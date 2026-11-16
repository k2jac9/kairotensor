# Policy activation

`Store::activate_policy` is a trusted management operation for one
`(actor, scope, purpose)`. The embedding runtime authenticates the caller and
supplies that scope. Imported artifacts and evaluation attestations cannot grant
management authority or permission to send model work. `active_policy` reads
metadata in one consistent SQLite snapshot; its result is not a dispatch claim.

Registration preserves immutable exact bytes and allocates an unadmitted evidence
occurrence. Bootstrap explicitly selects one registered baseline with no active
head, supplies the original frozen comparison-manifest bytes and SHA256, and
requires a nonempty reason and `expected_active_revision: None`. Strict manifest
validation checks its structure, scoped identity and exact compatible artifact.
The original bytes are hashed without normalization; the head and event retain
that digest. Bootstrap establishes a comparison baseline, with no assertion of
measured superiority. It does not require an evaluation or known accounting unit.
Synthetic manifests are permitted only for `fixture/recovery-policy`; bootstrap
refuses another synthetic purpose as `Incompatible`, without writes.

Promotion requires a positive expected revision, the requested candidate and a
scoped stored evaluation. The evaluation's baseline version and digest must equal
the current active head. Its original manifest bytes must match the bootstrapped
digest. Selection is recomputed from the original stored attestation: strictly
more successes, no increase in false refusals, unsafe acceptances, cost or deadline
misses. A separately stored baseline-only evaluation must fully validate as
complete `NoImprovement`, identify the same baseline on both sides with identical
rows, and exactly match the comparison's baseline projection, manifest, provenance
and evaluator. Unknown evaluator attestations remain unevaluated; neither their
storage nor a matching projection makes them sufficient baseline evidence.

Rollback requires a positive expected revision, a nonempty reason, a target that
was previously activated in this exact scope/purpose and a currently valid artifact
with the same compatibility contract. It needs no superiority evaluation. Selecting
the active target refuses as `NoChange`. Rollback advances revision like promotion.

All activation gates and writes run in one Immediate transaction. Before bootstrap,
the admitted registered-policy set in this exact actor/scope/purpose must be empty;
for later transitions it must consist of precisely the current head occurrence.
Externally withdrawing the head, admitting the target or admitting a third inactive
version causes `AdmissionMismatch`, without repair. Admissions in other purposes
are independent. Ordinary separately authorized admission corrections can repair
membership before a fresh activation request.

The transaction withdraws the old occurrence, admits the target, reconciles support
heads against the final combined view, updates the pointer and appends one immutable
event. An unchanged support verdict retains its old attempt and charge; each changed
verdict receives one new publication. Failure at any stage rolls back all changes.
The initial revision is 1; subsequent revisions are checked additions. Concurrent
bootstrap losers receive `AlreadyInitialized`; stale transition requests receive
`RevisionConflict`. Read fresh state before constructing another request.

Existing dispatch claims, deliveries, outstanding reservations, original deadlines
and attempt dispositions remain untouched. The scope epoch changes, so captured
opaque and timed work must pass the existing freshness gates before dispatch or
publication. An uncertain previously claimed send keeps its hold across activation
and reopen. Different purposes sharing a scope can invalidate opaque work coarsely.

Malformed management request syntax produces an error. Valid requests that fail a
semantic activation gate return `Refused`; underlying persisted corruption and
SQLite failures remain errors. Refusals and errors never partially activate.

Positive fixtures demonstrate synthetic registry mechanics only. They are not
observed policy superiority, live model results, production release acceptance or
a migration of an owner's live database. Existing acceptance gates remain required.


## Offline typed fixture

Run the separate fixture with no model or provider calls. On the owner workstation,
use:

```sh
artifact_root=/home/neo/Projects/kairotensor-artifacts
mkdir -p "$artifact_root"
packet="$(mktemp -d "$artifact_root/policy-activation-XXXXXXXX")"
python3 scripts/recovery_policy_eval.py --activation-fixture > "$packet/fixture.json"
kairotensor-cargo --budget-gib 2 run --locked --offline --example policy_activation -- \
  --fixture "$packet/fixture.json" --out "$packet/run"
python3 scripts/recovery_policy_eval.py --archived-v1-refusal > "$packet/archived-v1-refusal.json"
```

The unique packet retains the input, SQLite database, prepared requests, result
and archived-refusal evidence under the project artifact root. The CLI's run
directory is new inside the packet. The example refuses an existing run directory
instead of opening an owner database. Other hosts should choose their project's
retained artifact directory and use its required admitted Cargo invocation. CI
runs it explicitly alongside the existing offline steps. These are configured
checks; remote CI execution is separate.

The `policy-activation-fixture/1` envelope contains a complete deliberately
synthetic `PolicyEvaluation`, generated with the existing D1 codec and evaluator
identity. The example registers known `always_stop` and `affordable_retry`
artifacts, bootstraps the baseline, records a complete identical-row baseline-only
evaluation and a qualifying paired evaluation, promotes, then rolls back to the
previous occurrence at revision 3. Synthetic row counters are attestations for
registry mechanics: they are **not feasible named-policy replay or measured
superiority**. Their frozen semantic labels never enter execution requests.

Execution uses a separate `policy-report-fixture/1` manifest: exactly one `Policy`
occurrence in position 0, followed by `Report` occurrences. It binds the active
version, revision, exact evidence occurrence ID, artifact digest, compatibility
contract and original baseline manifest digest, plus each ordered input occurrence
and payload hash. All occurrences pass through ordinary Store recovery capture.
Preparation refuses filtering, reordering, role/hash mismatch, unsupported contract
or an identical-byte policy acquired as a different occurrence. It validates the
captured policy artifact against a fresh active selection immediately before
preparation. Only the captured Reports become `ci_reports`; policy bytes are never
parsed as a Report or inserted into that payload.

The immutable request includes this manifest, a computation digest, the fixed
protocol feedback and `fixture-byte-runtime/1` identity with a byte quote and
output cap. Normal Store claim and settlement freshness gates still decide
permission to dispatch and publish after preparation. The demonstration shows a
policy switch fencing an old prepared claim, an old duplicate `Accepted` receipt
labelled `historicalAccepted` while the current head needs review, and an unresolved
claimed attempt retaining its exact hold through promotion, rollback and database
reopen. Unsent work may be cancelled; an uncertain claimed send cannot be reclaimed
or cancelled. Rollback advances revision and does not make old work current.

`--archived-v1-refusal` reads the retained actual v1 ZIP's `trace.json`, preserves
its archive/trace hashes, and invokes the unchanged explicit v2 converter without
supplying invented binding facts. It returns `Incomplete`: v1 fields alone do not
establish frozen answerability, eligibility, units or observed references. This
is a refusal at the conversion boundary, so no insertable `PolicyEvaluation` or
Store promotion request is manufactured. Frozen-v1 default modes, archived schemas
1/2, output and the Report-only collector remain unchanged.

The typed policy fixture has its own role and validation contract. Existing
`incident/1` decision export/replay and diagnostic support do not cover it; no
incident validator binding or unsupported replay claim is attached to this demo.
