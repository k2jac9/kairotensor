CREATE TABLE branches (
    id INTEGER PRIMARY KEY,
    actor TEXT NOT NULL CHECK (actor <> ''),
    scope TEXT NOT NULL CHECK (scope <> ''),
    name TEXT NOT NULL CHECK (name <> ''),
    head INTEGER,
    revision INTEGER NOT NULL DEFAULT 0
        CHECK (typeof(revision) = 'integer' AND revision >= 0),
    UNIQUE (actor, scope, name),
    FOREIGN KEY (head, id) REFERENCES attempts (id, branch_id)
);

CREATE TABLE attempts (
    id INTEGER PRIMARY KEY,
    branch_id INTEGER NOT NULL REFERENCES branches (id),
    parent INTEGER,
    branch_revision INTEGER NOT NULL,
    view_epoch INTEGER NOT NULL,
    inherited_from INTEGER,
    UNIQUE (id, branch_id),
    FOREIGN KEY (parent, branch_id) REFERENCES attempts (id, branch_id),
    CHECK (inherited_from IS NULL OR (parent IS NOT NULL AND inherited_from = parent))
);

-- Repeated occurrences are valid inputs at distinct positions.
CREATE TABLE attempt_inputs (
    attempt_id INTEGER NOT NULL REFERENCES attempts (id),
    position INTEGER NOT NULL,
    evidence_id INTEGER NOT NULL REFERENCES evidence (id),
    PRIMARY KEY (attempt_id, position)
);
CREATE INDEX attempt_inputs_by_evidence ON attempt_inputs (evidence_id, attempt_id);

-- Capture the lookup basis, including missing IDs. Only admitted candidates
-- enter attempt_inputs. An actor/scope epoch change invalidates membership.
CREATE TABLE attempt_candidates (
    attempt_id INTEGER NOT NULL REFERENCES attempts (id),
    position INTEGER NOT NULL,
    requested_id INTEGER NOT NULL,
    PRIMARY KEY (attempt_id, position)
);

-- Absence of a delivery denotes pending work with unknown usage.
CREATE TABLE deliveries (
    attempt_id INTEGER PRIMARY KEY REFERENCES attempts (id),
    disposition TEXT NOT NULL CHECK (disposition IN ('accepted', 'stale_view', 'stale_branch')),
    output BLOB NOT NULL,
    usage INTEGER NOT NULL CHECK (typeof(usage) = 'integer' AND usage >= 0)
);
