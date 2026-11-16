# Evidence reliability review guide

This integration preserves the 48 local commits through `c7501c4e9c29a5cd5432c44b4c075671bfa9ae84`, based on `20fab84a6baa525452191ecf80eeee339f0b5ac7`. The groups below are review boundaries in the existing history, not independent releases. Later fixes must be read with their earlier implementations.

## Review order

| Area | Compare range | Commits |
| --- | --- | ---: |
| Eligibility and atomic correction | `20fab84a..1509c38` | 6 |
| Store-backed incident preparation | `1509c38..0dd48f8` | 4 |
| SQLite lifetime fixes and decision replay | `0dd48f8..b921743` | 7 |
| Policy registry, validation and activation | `b921743..a621ad8` | 10 |
| Bounded offline diagnostics | `a621ad8..794f64e` | 2 |
| Runtime qualification and local launcher | `794f64e..0899eca` | 16 |
| Documentation and warning/log maintenance | `0899eca..c7501c4` | 3 |

Review the shared current-read transaction and absence of diagnostic payloads first; then batch admission ordering, rollback and expected-epoch conflicts. Incident preparation must render captured bytes and retain claim/publication checks. Recorded bundle replay must remain separate from execution and current access. Policy transitions must bind comparable evaluation evidence and active revision, preserve outstanding holds, and atomically revise admission. Offline ablations cannot authorize execution or narrow opaque dependencies.

The schema migration is exercised on disposable populated databases. This PR does not migrate a live owner's database. Runtime qualification and local launcher code have a separate trust boundary: local acceptance artifacts bind actual executables; the launcher is not a sandbox for arbitrary workloads.

## Verification evidence

The latest executable source before this guide is `62787673a61d3f126464a5087036194726d67ec6`; `c7501c4` changes documentation only. [Maintenance verification](verification-maintenance.md) records 362 Rust tests on each of three compiler versions, 376 Python script tests on each of three interpreters with actual fixture integrations, 18 reference and eight oracle checks, and seven affected offline examples. These local runs had no compiler warnings or skipped tests. Earlier failed checks and the unrecoverable superseded preflight display remain historical evidence.

The workflow defines five isolated combinations: stable, previous Rust, upcoming Rust, previous Python and upcoming Python. A configured matrix is not a passing hosted result. Consult the exact integration PR head and its Actions runs for hosted status; do not transfer a passing result from another commit.

## Research and review limits

All new semantic integration, replay, policy activation and ablation evidence here is offline or deterministic. It establishes tested contracts rather than improved model accuracy, independent execution authenticity, or typical savings. The sprint report in PR #19 retains its separately measured source. The hosted adapter in PR #18 is a separate change and is not included in this branch. Active external training is not a result of this PR.

## Preserved commit sequence

### Eligibility and atomic correction

- `8d4cce8` docs: review evidence reliability and plan bounded P0 work
- `9141d8f` build: freeze the tested dependency resolution
- `0d7f61c` feat: explain current eligibility through the shared read gate
- `c322e57` docs: define autonomous reliability milestones and decision boundaries
- `cfa8d3c` fix: reject malformed accepted output in eligibility gate
- `1509c38` feat: atomically revise admissions at an expected epoch

### Store-backed incident preparation

- `261a863` feat: add pure versioned incident protocol and exact-byte digests
- `ef42bcf` fix: bound incident protocol allocation before size refusal
- `94a6d43` feat: exercise incident preparation through Store timed gates
- `0dd48f8` test: verify Store incident byte parity in CI

### SQLite lifetime fixes and decision replay

- `3b45721` docs: bind replay safeguards and SQLite lifetime repair
- `062725f` fix: close SQLite audit and controller connections
- `8a519e1` docs: define deterministic decision bundle digest encoding
- `43556ae` feat: define bounded pure decision bundle replay
- `b5756e0` fix: enforce recovery bundle ledger invariants
- `37f25c4` feat: authorize decision exports within one Store snapshot
- `b921743` test: exercise decision replay across process boundaries

### Policy registry, validation and activation

- `04cb3b6` feat: validate complete comparable policy evaluation records
- `62f5646` fix: validate partial policy facts and legacy attempt timing
- `0923e13` docs: bind policy migration and activation integration scopes
- `2cc5f9d` feat: persist immutable scoped policy versions and evaluations
- `f1a0af0` fix: prevent rowid replacement of immutable policy records
- `33b3303` docs: bind activation consistency and diagnostic runtime boundaries
- `ee5beb6` feat: atomically activate evaluated policies
- `a8f8c4b` test: demonstrate policy activation and rollback without model calls
- `9329950` fix: bound policy fixture input before deserialization
- `a621ad8` docs: retain policy activation fixture packet

### Bounded offline diagnostics

- `31e0dd1` feat: plan and score bounded offline ablations
- `794f64e` fix: preserve unspecified-unit accounting observations

### Runtime qualification and local launcher

- `5bf3c75` Add source-bound isolated runtime matrix verifier and refusal tests
- `26045ce` Bind runtime receipts to the complete qualification manifest
- `353ff2f` Retain empty log hashes alongside nonempty qualification results
- `ab8e162` Normalize runtime receipt counts across JSON persistence
- `dd52954` Bind persisted runtime command and child interpreter provenance
- `fce095d` docs: record isolated runtime qualification and rollback evidence
- `d60159d` fix: isolate unavailable runtime lanes at the matrix CLI
- `7684b4f` docs: record isolated lane fix and five-runtime requalification
- `ae4aa08` Pin qualified project runtimes and add verified local launcher
- `64e0b1b` Bind native helper resolution and reject Cargo selector bypasses
- `9d94dbf` Record final runtime launcher qualification and activation handoff
- `e32c968` Reject inherited native flag families and target-specific selectors
- `1870b60` Record native boundary correction verification and retained warnings
- `d370b41` Clarify units of the retained native wrapper audit
- `5e9c76e` Bind rustdoc selection and version unknown-unit replay observations
- `0899eca` Document V2 selection and replay semantics with final source evidence

### Documentation and warning/log maintenance

- `7c7ca36` docs: remove unrelated integration references
- `6278767` Separate prepared adapter operations and preserve existing verification logs
- `c7501c4` Document warning cleanup and log preservation verification

