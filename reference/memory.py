"""Finite SQLite contract model for the Rust-first design, NOT a production library.

README / scope: logical actor/scope filtering is not authorization or a security
boundary. Callers can impersonate actors; database-file access bypasses this API.
Historical diagnostics return IDs/status, never raw evidence. Stored context is
not exposed by a historical raw-serving API. Hashes identify exact bytes, not
truth or semantic equivalence. Direct occurrence-only support cannot form claim
cycles, but has no inference engine, freshness policy, contradiction handling,
or nested claims. HAS_SUPPORT is not a truth judgment.

Branch heads are opaque compare-and-swap tokens, not served/inherited app state.
Caller-supplied context bytes are retained exactly but are NOT proven clean by
checking its declared evidence IDs. This model omits the full design's
post-withdrawal needs-review / read-resume gate and checkpoint recertification;
it does not demonstrate end-to-end withdrawal from already committed state.

No model/tool execution, retrieval, tensor backend, MCAP, budgets, reservations,
clock, timestamps, network, migrations, garbage collection, or fault injection.
Costs are caller-supplied nonnegative integer units, not verified provider bills.
Validators are caller-supplied outcomes, not executed or authenticated here.
Only short database work occurs within BEGIN IMMEDIATE transactions. Unit tests
demonstrate selected semantics and reopen behavior, not crash/fault safety.
"""

from contextlib import contextmanager
import hashlib
import json
import sqlite3
import uuid


class Conflict(ValueError):
    """A previously used identity was delivered with different contents."""


def _label(value):
    if not isinstance(value, str) or not value.strip():
        raise ValueError("a nonempty string is required")
    return value


def _bytes(value):
    if not isinstance(value, bytes):
        raise ValueError("payload must be bytes")
    return value


def _ids(values):
    if not isinstance(values, (list, tuple)):
        raise ValueError("an ordered list or tuple of evidence IDs is required")
    return [_label(value) for value in values]


def _json(value):
    return json.dumps(value, ensure_ascii=False, separators=(",", ":")).encode()


def _id(prefix):
    return prefix + "_" + uuid.uuid4().hex


def object_ref(kind, version, payload):
    """SHA-256 over four independently length-prefixed fields; lengths are u64 BE."""
    _label(kind)
    _bytes(payload)
    if type(version) is not int or version < 1:
        raise ValueError("positive integer version required")
    fields = (b"evidence-memory-object", kind.encode(), str(version).encode(), payload)
    preimage = b"".join(len(field).to_bytes(8, "big") + field for field in fields)
    return "sha256:" + hashlib.sha256(preimage).hexdigest()


class Memory:
    """One connection per instance; use separate instances for concurrent callers."""

    def __init__(self, path):
        self._db = sqlite3.connect(path, isolation_level=None, timeout=5.0)
        self._db.row_factory = sqlite3.Row
        self._db.execute("PRAGMA foreign_keys=ON")
        if self._db.execute("PRAGMA user_version").fetchone()[0] not in (0, 1):
            self.close()
            raise ValueError("unsupported schema version; no migration is provided")
        self._db.executescript("""
        CREATE TABLE IF NOT EXISTS objects(
            ref TEXT PRIMARY KEY, kind TEXT NOT NULL, version INTEGER NOT NULL,
            payload BLOB NOT NULL);
        CREATE TABLE IF NOT EXISTS evidence(
            id TEXT PRIMARY KEY, object_ref TEXT NOT NULL REFERENCES objects(ref),
            source TEXT NOT NULL);
        CREATE TABLE IF NOT EXISTS views(
            actor TEXT, scope TEXT, epoch INTEGER NOT NULL CHECK(epoch>=0),
            PRIMARY KEY(actor,scope));
        CREATE TABLE IF NOT EXISTS admitted(
            actor TEXT, scope TEXT, evidence_id TEXT REFERENCES evidence(id),
            PRIMARY KEY(actor,scope,evidence_id),
            FOREIGN KEY(actor,scope) REFERENCES views(actor,scope));
        CREATE TABLE IF NOT EXISTS admission_events(
            seq INTEGER PRIMARY KEY AUTOINCREMENT, actor TEXT NOT NULL,
            scope TEXT NOT NULL, evidence_id TEXT NOT NULL REFERENCES evidence(id),
            action TEXT NOT NULL CHECK(action IN ('ADMIT','REVOKE')), epoch INTEGER NOT NULL,
            UNIQUE(actor,scope,epoch));
        CREATE TABLE IF NOT EXISTS snapshots(
            id TEXT PRIMARY KEY, actor TEXT NOT NULL, scope TEXT NOT NULL,
            epoch INTEGER NOT NULL, manifest_ref TEXT NOT NULL REFERENCES objects(ref));
        CREATE TABLE IF NOT EXISTS derivations(
            id TEXT PRIMARY KEY, rule_id TEXT NOT NULL,
            alternatives_ref TEXT NOT NULL REFERENCES objects(ref));
        CREATE TABLE IF NOT EXISTS branches(
            name TEXT PRIMARY KEY, head TEXT NOT NULL REFERENCES objects(ref),
            revision INTEGER NOT NULL CHECK(revision>=0));
        CREATE TABLE IF NOT EXISTS branch_events(
            branch TEXT REFERENCES branches(name), revision INTEGER,
            head TEXT NOT NULL REFERENCES objects(ref), reason TEXT NOT NULL,
            PRIMARY KEY(branch,revision));
        CREATE TABLE IF NOT EXISTS attempts(
            id TEXT PRIMARY KEY, branch TEXT NOT NULL REFERENCES branches(name),
            branch_head TEXT NOT NULL REFERENCES objects(ref), branch_revision INTEGER NOT NULL,
            actor TEXT NOT NULL, scope TEXT NOT NULL, view_epoch INTEGER NOT NULL,
            snapshot_id TEXT NOT NULL REFERENCES snapshots(id),
            context_ref TEXT NOT NULL REFERENCES objects(ref),
            inputs_ref TEXT NOT NULL REFERENCES objects(ref));
        CREATE TABLE IF NOT EXISTS ledger(
            usage_id TEXT PRIMARY KEY, attempt_id TEXT NOT NULL UNIQUE REFERENCES attempts(id),
            cost_units INTEGER NOT NULL CHECK(cost_units>=0));
        CREATE TABLE IF NOT EXISTS settlements(
            attempt_id TEXT PRIMARY KEY REFERENCES attempts(id),
            result_ref TEXT NOT NULL REFERENCES objects(ref),
            validator_ok INTEGER NOT NULL CHECK(validator_ok IN (0,1)),
            usage_id TEXT NOT NULL UNIQUE REFERENCES ledger(usage_id),
            disposition TEXT NOT NULL CHECK(disposition IN ('ACCEPTED','STALE','REJECTED')),
            reason TEXT NOT NULL);
        CREATE TABLE IF NOT EXISTS logs(
            id TEXT PRIMARY KEY, object_ref TEXT NOT NULL REFERENCES objects(ref));
        PRAGMA user_version=1;
        """)
        # Defense against accidental SQL edits; not protection from database owners.
        for table in ("objects", "evidence", "admission_events", "snapshots", "derivations",
                      "branch_events", "attempts", "ledger", "settlements", "logs"):
            for operation in ("UPDATE", "DELETE"):
                self._db.execute(f"""CREATE TRIGGER IF NOT EXISTS {table}_{operation}_guard
                    BEFORE {operation} ON {table} BEGIN
                    SELECT RAISE(ABORT,'immutable record'); END""")

    def close(self):
        self._db.close()

    @contextmanager
    def _transaction(self):
        self._db.execute("BEGIN IMMEDIATE")
        try:
            yield
            self._db.execute("COMMIT")
        except BaseException:
            if self._db.in_transaction:
                self._db.execute("ROLLBACK")
            raise

    def _one(self, sql, parameters):
        row = self._db.execute(sql, parameters).fetchone()
        if row is None:
            raise KeyError(parameters)
        return dict(row)

    def _put(self, kind, payload):
        ref = object_ref(kind, 1, payload)
        old = self._db.execute("SELECT kind,version,payload FROM objects WHERE ref=?", (ref,)).fetchone()
        if old is not None:
            if tuple(old) != (kind, 1, payload):
                raise Conflict("hash collision or corrupted object")
        else:
            self._db.execute("INSERT INTO objects VALUES(?,?,?,?)", (ref, kind, 1, payload))
        return ref

    def _object_json(self, ref):
        return json.loads(self._one("SELECT payload FROM objects WHERE ref=?", (ref,))["payload"])

    def store_evidence(self, payload, source="unspecified"):
        """Store a new occurrence, even when the content already exists. Never admit it."""
        _bytes(payload)
        _label(source)
        evidence_id = _id("ev")
        with self._transaction():
            ref = self._put("evidence", payload)
            self._db.execute("INSERT INTO evidence VALUES(?,?,?)", (evidence_id, ref, source))
        return evidence_id

    def evidence_info(self, evidence_id):
        """Metadata only; object references do not confer current read access."""
        return self._one("SELECT * FROM evidence WHERE id=?", (_label(evidence_id),))

    def view_epoch(self, actor, scope):
        row = self._db.execute("SELECT epoch FROM views WHERE actor=? AND scope=?",
                               (_label(actor), _label(scope))).fetchone()
        return 0 if row is None else row[0]

    def _admitted_ids(self, actor, scope):
        return [row[0] for row in self._db.execute(
            "SELECT evidence_id FROM admitted WHERE actor=? AND scope=? ORDER BY evidence_id",
            (actor, scope))]

    def _change_admission(self, actor, scope, evidence_id, active):
        _label(actor), _label(scope), _label(evidence_id)
        with self._transaction():
            self.evidence_info(evidence_id)  # Validate before any insertion.
            epoch = self.view_epoch(actor, scope)
            present = self._db.execute(
                "SELECT 1 FROM admitted WHERE actor=? AND scope=? AND evidence_id=?",
                (actor, scope, evidence_id)).fetchone() is not None
            if present == active:
                return epoch
            self._db.execute("INSERT OR IGNORE INTO views VALUES(?,?,0)", (actor, scope))
            epoch += 1
            self._db.execute("UPDATE views SET epoch=? WHERE actor=? AND scope=?", (epoch, actor, scope))
            if active:
                self._db.execute("INSERT INTO admitted VALUES(?,?,?)", (actor, scope, evidence_id))
            else:
                self._db.execute("DELETE FROM admitted WHERE actor=? AND scope=? AND evidence_id=?",
                                 (actor, scope, evidence_id))
            self._db.execute("INSERT INTO admission_events(actor,scope,evidence_id,action,epoch) VALUES(?,?,?,?,?)",
                             (actor, scope, evidence_id, "ADMIT" if active else "REVOKE", epoch))
        return epoch

    def admit(self, actor, scope, evidence_id):
        return self._change_admission(actor, scope, evidence_id, True)

    def revoke(self, actor, scope, evidence_id):
        return self._change_admission(actor, scope, evidence_id, False)

    def admission_history(self, actor, scope):
        return [dict(row) for row in self._db.execute(
            "SELECT * FROM admission_events WHERE actor=? AND scope=? ORDER BY epoch",
            (_label(actor), _label(scope)))]

    def get_evidence(self, actor, scope, evidence_id):
        """Current admission and bytes are read in ONE statement, not separate checks."""
        row = self._db.execute("""SELECT o.payload FROM admitted a
            JOIN evidence e ON e.id=a.evidence_id JOIN objects o ON o.ref=e.object_ref
            WHERE a.actor=? AND a.scope=? AND a.evidence_id=?""",
            (_label(actor), _label(scope), _label(evidence_id))).fetchone()
        if row is None:
            raise PermissionError("evidence is not currently admitted to this view")
        return row[0]

    def _snapshot(self, actor, scope):
        snapshot_id = _id("snapshot")
        manifest = self._put("snapshot-manifest", _json(self._admitted_ids(actor, scope)))
        self._db.execute("INSERT INTO snapshots VALUES(?,?,?,?,?)",
                         (snapshot_id, actor, scope, self.view_epoch(actor, scope), manifest))
        return snapshot_id

    def capture_snapshot(self, actor, scope):
        _label(actor), _label(scope)
        with self._transaction():
            return self._snapshot(actor, scope)

    def snapshot_info(self, snapshot_id):
        """Diagnostic manifest; this is not an authority to retrieve historical bytes."""
        snapshot = self._one("SELECT * FROM snapshots WHERE id=?", (_label(snapshot_id),))
        snapshot["evidence_ids"] = self._object_json(snapshot["manifest_ref"])
        return snapshot

    def derive(self, rule_id, alternatives):
        """Record OR-of-AND support over occurrence IDs; no nested claims or empty proof."""
        _label(rule_id)
        if not isinstance(alternatives, (list, tuple)) or not alternatives:
            raise ValueError("at least one nonempty alternative is required")
        clauses = [_ids(clause) for clause in alternatives]
        if any(not clause for clause in clauses):
            raise ValueError("empty support clauses are forbidden")
        derivation_id = _id("derivation")
        with self._transaction():
            for evidence_id in {item for clause in clauses for item in clause}:
                self.evidence_info(evidence_id)
            ref = self._put("support-alternatives", _json(clauses))
            self._db.execute("INSERT INTO derivations VALUES(?,?,?)", (derivation_id, rule_id, ref))
        return derivation_id

    def _support(self, derivation_id, admitted_ids):
        derivation = self._one("SELECT * FROM derivations WHERE id=?", (_label(derivation_id),))
        clauses = self._object_json(derivation["alternatives_ref"])
        admitted = set(admitted_ids)
        active = [i for i, clause in enumerate(clauses) if all(item in admitted for item in clause)]
        return {"derivation_id": derivation_id, "rule_id": derivation["rule_id"],
                "status": "HAS_SUPPORT" if active else "NO_ACTIVE_SUPPORT",
                "active_alternatives": active}

    def current_support(self, derivation_id, actor, scope):
        _label(actor), _label(scope)
        return self._support(derivation_id, self._admitted_ids(actor, scope))

    def historical_support(self, derivation_id, snapshot_id):
        """As-of diagnostic only: evaluates membership, never grants raw-data access."""
        snapshot = self.snapshot_info(snapshot_id)
        result = self._support(derivation_id, snapshot["evidence_ids"])
        return dict(result, snapshot_id=snapshot_id, view_epoch=snapshot["epoch"])

    def create_branch(self, name, initial_state=b""):
        _label(name), _bytes(initial_state)
        with self._transaction():
            if self._db.execute("SELECT 1 FROM branches WHERE name=?", (name,)).fetchone():
                raise Conflict("branch already exists")
            head = self._put("branch-state", initial_state)
            self._db.execute("INSERT INTO branches VALUES(?,?,0)", (name, head))
            self._db.execute("INSERT INTO branch_events VALUES(?,0,?,'CREATE')", (name, head))
        return self.branch_state(name)

    def branch_state(self, name):
        return self._one("SELECT head,revision FROM branches WHERE name=?", (_label(name),))

    def _advance(self, name, head, reason):
        revision = self.branch_state(name)["revision"] + 1
        self._db.execute("UPDATE branches SET head=?,revision=? WHERE name=?", (head, revision, name))
        self._db.execute("INSERT INTO branch_events VALUES(?,?,?,?)", (name, revision, head, reason))

    def restore_branch(self, name, previous_head):
        """Restore a head previously held by this branch, always increasing revision."""
        _label(name), _label(previous_head)
        with self._transaction():
            self.branch_state(name)
            if not self._db.execute("SELECT 1 FROM branch_events WHERE branch=? AND head=?",
                                    (name, previous_head)).fetchone():
                raise ValueError("restore target was not a head of this branch")
            self._advance(name, previous_head, "RESTORE")
        return self.branch_state(name)

    def begin_attempt(self, branch, actor, scope, evidence_ids, context):
        """Capture the optimistic fence and the exact supplied bytes; execute elsewhere."""
        _label(branch), _label(actor), _label(scope), _bytes(context)
        inputs = _ids(evidence_ids)
        attempt_id = _id("attempt")
        with self._transaction():
            state = self.branch_state(branch)
            admitted = set(self._admitted_ids(actor, scope))
            for evidence_id in inputs:
                self.evidence_info(evidence_id)
                if evidence_id not in admitted:
                    raise PermissionError("attempt input is not currently admitted")
            epoch = self.view_epoch(actor, scope)
            snapshot = self._snapshot(actor, scope)
            context_ref = self._put("input-context", context)
            inputs_ref = self._put("input-evidence-order", _json(inputs))
            self._db.execute("INSERT INTO attempts VALUES(?,?,?,?,?,?,?,?,?,?)",
                             (attempt_id, branch, state["head"], state["revision"], actor,
                              scope, epoch, snapshot, context_ref, inputs_ref))
        return attempt_id

    def attempt_info(self, attempt_id):
        """References and metadata only; never serve historical context by its ID."""
        attempt = self._one("SELECT * FROM attempts WHERE id=?", (_label(attempt_id),))
        attempt["input_evidence_ids"] = self._object_json(attempt["inputs_ref"])
        return attempt

    def _settlement(self, attempt_id):
        row = self._db.execute("""SELECT s.*,l.cost_units FROM settlements s
            JOIN ledger l USING(usage_id) WHERE s.attempt_id=?""", (attempt_id,)).fetchone()
        return None if row is None else dict(row)

    def settle(self, attempt_id, result, validator_ok, usage_id, cost_units):
        """Atomically settle once and optionally promote; stale/rejected work still costs."""
        _label(attempt_id), _bytes(result), _label(usage_id)
        if type(validator_ok) is not bool:
            raise ValueError("validator_ok must be a boolean")
        if type(cost_units) is not int or not 0 <= cost_units < 2**63:
            raise ValueError("cost must be a nonnegative signed-64-bit integer in caller-defined units")
        result_ref = object_ref("attempt-result", 1, result)
        with self._transaction():
            attempt = self._one("SELECT * FROM attempts WHERE id=?", (attempt_id,))
            existing = self._settlement(attempt_id)
            if existing is not None:
                received = (result_ref, int(validator_ok), usage_id, cost_units)
                recorded = tuple(existing[key] for key in ("result_ref", "validator_ok", "usage_id", "cost_units"))
                if received != recorded:
                    raise Conflict("conflicting duplicate settlement")
                return existing
            if self._db.execute("SELECT 1 FROM ledger WHERE usage_id=?", (usage_id,)).fetchone():
                raise Conflict("usage ID already belongs to another attempt")
            branch = self.branch_state(attempt["branch"])
            if (branch["head"], branch["revision"]) != (attempt["branch_head"], attempt["branch_revision"]):
                disposition, reason = "STALE", "branch_changed"
            elif self.view_epoch(attempt["actor"], attempt["scope"]) != attempt["view_epoch"]:
                disposition, reason = "STALE", "view_changed"
            elif not validator_ok:
                disposition, reason = "REJECTED", "validator_rejected"
            else:
                disposition, reason = "ACCEPTED", "accepted"
            # All external input and duplicate checks precede the first write.
            self._put("attempt-result", result)
            self._db.execute("INSERT INTO ledger VALUES(?,?,?)", (usage_id, attempt_id, cost_units))
            self._db.execute("INSERT INTO settlements VALUES(?,?,?,?,?,?)",
                             (attempt_id, result_ref, int(validator_ok), usage_id, disposition, reason))
            if disposition == "ACCEPTED":
                self._advance(attempt["branch"], result_ref, "PROMOTE")
            return self._settlement(attempt_id)

    def usage_total(self):
        # Python integer summation also avoids SQLite SUM overflowing across bills.
        return sum(row[0] for row in self._db.execute("SELECT cost_units FROM ledger"))

    def log_event(self, payload):
        _bytes(payload)
        log_id = _id("log")
        with self._transaction():
            ref = self._put("log", payload)
            self._db.execute("INSERT INTO logs VALUES(?,?)", (log_id, ref))
        return log_id
