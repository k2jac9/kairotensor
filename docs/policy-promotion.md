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
