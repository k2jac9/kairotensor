# Explain current eligibility

`Store::explain_current(actor, scope, branch)` returns metadata explaining the
same gate used by `read_current` and `begin_resume`. `Ready` means eligible under
the Store contract; it does not mean the saved statement is true. The embedding
runtime must authenticate the actor/scope and authorize diagnostic access.

| Snapshot | State | Proof | Reason |
| --- | --- | --- | --- |
| No branch or no head | `Empty` | None | `NoHead` |
| Head's captured epoch equals the current scope epoch | `Ready` | `CoarseEpoch` | None |
| Epoch changed; support/v1 verdict still matches its publication | `Ready` | `SupportV1` | None |
| Epoch changed; support/v1 verdict changed | `NeedsReview` | `SupportV1` | `SupportVerdictChanged` |
| Epoch changed; opaque or model head | `NeedsReview` | `CoarseEpoch` | `CoarseViewChanged` |

The explanation includes `head_id`, `branch_revision`, `captured_epoch`, and
`current_epoch`. `branch_revision` is absent only when the branch does not exist.
An empty branch has no head, captured epoch, or proof. With an unchanged epoch,
even a support head uses the coarse epoch proof. A changed support verdict is
ineligible in either direction: supported to unsupported or the reverse.
Missing witnesses do not prove the negation of the underlying proposition.

The evaluator verifies an accepted delivery exists without fetching its output.
A missing or nonaccepted delivery for a stored head is an error, including when
its epoch is stale. Database read errors propagate. Explanations contain no
output bytes, serialized request, occurrence payload, or free-form validator
reason, including in their `Debug` representation.

Each explanation is evaluated within one SQLite read transaction. Current reads
retain that transaction through loading the accepted output. Resume retains its
existing writer transaction through the shared gate, output loading, and capture
of the new attempt and lineage. A writer committing during a WAL read may leave
that pending read on the older coherent snapshot. The next read observes the new
state. Separate calls to `explain_current` and `read_current` may see different
snapshots; an explanation is never an authorization cache or permission to send.

Explanations do not change admission events, branch revisions, attempts,
historical dispositions, charges, or unresolved reservations. Historical
`Accepted` receipts remain historical even after current access is withdrawn.
Opaque and model heads retain coarse invalidation. A compatible model result
requires a new opted-in attempt and explicit `resolve_model_attempt`; explaining
it neither performs reuse nor dispatches work.

The eligibility tests cover every table row, both support-result directions,
actor/scope separation, empty branches, history and usage preservation, malformed
heads, and explicit model reuse. Internal test-only channel barriers pause the
actual read after gate evaluation, permit a second connection to commit, and
check immediately before output loading that the transaction remains active and
the same connection still sees the captured epoch, head, and branch revision.
They then compare the output with the captured head and check the next read.
There is no public race hook or new persisted format.
