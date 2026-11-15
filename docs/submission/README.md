# KairoTensor submission guide

**KairoTensor: Evidence-Grounded Revision for Agent Memory**

Alejandro Castellanos, Independent Research

Apart Research AI x Epistemics Research Sprint, 13-15 November 2026

Recommended track: **Track 2: Trust Infrastructure**

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
after that snapshot. Unmerged PR #18 is excluded from the reported system.

## Review

- [Submission report (PDF)](kairotensor-report.pdf) and [editable manuscript](report.md).
- [Evidence index](EVIDENCE.md): findings, raw artifacts, source identities and limits.
- [Reproduction guide](REPRODUCE.md): offline summaries and consistency checks.
- [Four-minute presentation script](DEMO.md): an optional presentation, not a recording.

The accompanying reviewer ZIP contains the six-page report, exact source archive,
these guides, verification receipts and SHA-256 manifest. The source archive has
162 entries and SHA-256
`e5928a4daf5b9d68cfbec0f58afdfd53d65f4202e3f1c5d54a4284baa1c0b821`.
It preserves the measured snapshot rather than incorporating later documentation.

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

Archived benchmark summaries were recomputed. The frozen recovery ZIP passed its
read-only internal consistency audit, including 20 recorded calls, 5,241 charged
tokens and zero unresolved reservations. Its bytes were unchanged after auditing.
No new inference was run in preparing this package. A successful audit is not
independent authentication of model or hardware execution.

The newer incident-routing calibration remains unexecuted. The current CI does
not implement the complete stable/preview/previous-stable compatibility matrix;
no such matrix is claimed here. Historical model/runtime pins are retained for
reproduction, not presented as current release recommendations.
