# KairoTensor submission guide

**KairoTensor: Evidence-Grounded Revision for Agent Memory**

Alejandro Castellanos, Independent Research

Apart Research AI x Epistemics Research Sprint, 13-15 November 2026

Recommended track: **Track 2: Trust Infrastructure**

For the integrated repository, separate correction-pilot supplement and delivery
status checked on 16 November, see [Current status](CURRENT_STATUS.md). The paper
and historical verification receipts below retain their original source boundary.

KairoTensor records the evidence supplied to an agent computation and checks
whether a saved result remains eligible after a correction or withdrawal. The
report evaluates atomic revision, scoped reuse and a failed recovery experiment.
It does not claim source authentication, tamper evidence, or general improvement
in real-world decisions.

## Provenance

The author confirmed on 15 November 2026 that no KairoTensor-specific ideas, code
or experiments existed before the sprint. This is an author declaration. Git
timestamps alone are not proof of when work began. Dependencies and prior work
are separately attributed in manifests and the report.

The measured main-branch snapshot is
`20fab84a6baa525452191ecf80eeee339f0b5ac7`. Historical experiments use the source
identities in [EVIDENCE.md](EVIDENCE.md). These submission documents were prepared
after that snapshot. Later reliability commits, the hosted adapter introduced by
PR #18, the separate correction pilot and AutoScientist runs are excluded from
the reported results. PR #18 has since merged; that integration does not change
which experiments the paper reports.

## Review

- [Submission report (PDF)](kairotensor-report.pdf) and [editable manuscript](report.md).
- [Evidence index](EVIDENCE.md): findings, raw artifacts, source identities and limits.
- [Reproduction guide](REPRODUCE.md): offline summaries and consistency checks.
- [Four-minute presentation script](DEMO.md): an optional presentation, not a recording.

The accompanying reviewer ZIP contains the seven-page report, exact source archive,
these guides, verification receipts and SHA-256 manifest. The source archive has
162 entries and SHA-256
`e5928a4daf5b9d68cfbec0f58afdfd53d65f4202e3f1c5d54a4284baa1c0b821`.
It preserves the measured snapshot rather than incorporating later documentation.

The 16 November finalization adds Doyle's truth maintenance system and de Kleer's
assumption-based TMS to the related work. The contribution is the durable
integration of declared evidence dependence at the model execution boundary;
dependency tracking itself is established prior work. Measured outcomes are
unchanged. The report retains a 192-word abstract, five pages through code and
data, one reference page, and one page with the ethics appendix and LLM statement.

New receipts are in [verification/finalization-20261116](verification/finalization-20261116/README.md).
The original 15 November receipts remain unchanged and describe the earlier
six-page export. They are historical records, not checks of the final PDF.

The repository was private when the package was prepared. The owner plans to
make it public; this preparation does not change visibility or submit an entry.
Reviewers need the ZIP or repository access to inspect the evidence.

## Verification scope

On 15 November, local verification passed 18 reference tests and 8 belief-oracle
tests. The scripts suite reported 203 tests with 28 skips: 175 executed without
failure. It emitted resource warnings about unclosed SQLite connections; the
warnings are retained in the reviewer receipts. Skipped checks are not passes.
Rust execution is supported by historical exact-source CI
[43622988515](https://github.com/k2jac9/kairotensor/actions/runs/43622988515),
not a fresh local Rust run during packaging.

Finalization on 16 November repeated the offline commands from a fresh reviewer
package extraction under Python 3.15.2: the same 201 executed tests passed and
28 script tests skipped. Two unclosed-SQLite-connection resource warnings remain
in the new stderr receipt. Both benchmark summaries matched the historical
outputs byte for byte, and the frozen recovery audit again passed with the same
recorded failures. This was an evidence/package check, not fresh model execution.

Archived benchmark summaries were recomputed. The frozen recovery ZIP passed its
read-only internal consistency audit, including 20 recorded calls, 5,241 charged
tokens and zero unresolved reservations. Its bytes were unchanged after auditing.
No new inference was run in preparing this package. A successful audit is not
independent authentication of model or hardware execution.

The incident-routing model-quality evaluation remains unexecuted. The measured
snapshot's CI predates the complete stable/upcoming/previous compatibility matrix.
The integrated Rust/Python matrix subsequently passed at `109798d`; its exact
source and coverage are recorded in [Current status](CURRENT_STATUS.md). Those
checks do not qualify the separate ML runtime or establish model performance.
Historical model/runtime pins are retained for reproduction, not presented as
current release recommendations.
