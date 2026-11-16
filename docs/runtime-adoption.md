# Runtime adoption and reproducible baseline

Checked on 2026-11-15. This change freezes a locally verified dependency
resolution and requires Cargo to honor it in the existing CI commands. Runtime
migration, hosted CI execution, release acceptance and rollback qualification
remain separate gates.

## Source and dependency identity

- Repository: `https://github.com/k2jac9/kairotensor.git`; branch:
  `feat/evidence-reliability-20261115`.
- Source tested: `8d4cce8dbab72b769c1bcad263210a65538c4fee`, with the added
  `Cargo.lock` and locked CI commands. Production Rust, manifests, examples and
  tests are unchanged by this task.
- Lock format: 4; SHA-256:
  `0298c2ecbad412b31f56d8c712bbea76c1b4cb0e055fdaee009bf967b170a820`.
- The lock is byte-identical to the retained `Cargo.verification.lock` tested on
  `20fab84a6baa525452191ecf80eeee339f0b5ac7`. There are no additions, removals or
  version changes relative to that resolution. The source repository had no
  tracked lock, so the historical CI transitive resolution is not established.
- Existing constraints remain unchanged: `rusqlite =0.32.1` with bundled SQLite,
  Linux `libc =0.2.177`, `serde 1` with derive, and `serde_json 1`. The selected
  serde and serde_json versions are `1.0.229` and `1.0.154` respectively. Newer
  versions of exact-pinned dependencies are outside this reproducibility change.

The complete 32 external package/version/license inventory is retained in
`dependency-review.json` in the evidence packet below, using locked Cargo
metadata. Declared licenses include MIT, Apache-2.0, BSD-2-Clause, Unlicense and
Unicode-3.0 combinations. Every external package declares a license; the root
unpublished crate has no manifest license declaration. This inspection covers
manifest declarations and the full resolved graph, including optional packages
not compiled on this host; it is not a vulnerability or legal audit.

## Exact executed environment

The local host was Linux `x86_64-unknown-linux-gnu`, NixOS `26.5.0`, kernel `7.3.4`.
The storage wrapper's existing directory label `nixos26.05-x86_64` is a cache key,
not an independently measured OS version.

| Component | Executed identity | Executable SHA-256 |
| --- | --- | --- |
| Rust | `1.99.1`, commit `58cf8f9c35b355199727627d07ae34a4e2846d67`, LLVM `23.1.1` | `c45c3ff06bc89e67f851c575aa2eb8624e71e83bd5a4bba7378147c95c702909` |
| Cargo | `1.99.1`, commit `5f94df4789f005f9a352888e8355ffc645b7ed0e` | `73d1b427364c2cf348812d53860ab93b87831d94831cdfeb7d6a237a9c477330` |
| Python | `3.15.2`, built 2026-10-25, GCC `16.2.0` | `7f7f9b35ee0ca35d3dbd073bd34e37e1364fae5570ab027f20dfbcb5e6787001` |

Rust and Cargo resolved through rustup to
`/home/neo/.rustup/toolchains/1.99.1-x86_64-unknown-linux-gnu/bin/`.
Python resolved to
`/nix/store/w3ps4vf470m69d9l5bcbbipay9insx32-python3-3.15.2/bin/python3.15`.
These are source-bound local qualification identities, not proof of a durable
project runtime pin or an operational deployment baseline.

## Fresh upstream selections and execution gaps

The owner policy requires current stable, upcoming and previous compatibility
lanes. Fresh upstream selections below are research inputs only; no runtime was
installed, upgraded or promoted by this task.

| Role | Rust selection as of 2026-11-15 | Python selection as of 2026-11-15 | Execution status |
| --- | --- | --- | --- |
| Local qualification baseline | `1.99.1` with exact executable identity above | `3.15.2` with exact executable identity above | Existing local CI workload passed, as detailed below. Accepted live baseline is not established. |
| Latest stable target | `1.100.0 (0ef1bc386 2026-11-09)`, distribution dated `2026-11-12` | `3.15.2` | Rust unexecuted. Python executed locally; hosted runner identity and complete cross-runtime matrix remain unestablished. |
| Latest published upcoming preview | `1.102.0-nightly (5f4effd57 2026-11-13)`, distribution dated `2026-11-15`; beta `1.101.0-beta.2 (63931fb6b 2026-11-12)`, dated `2026-11-14`, also recorded | `3.16.0a2`; artifact digest not established | Available upstream; neither runtime preview lane was executed or promoted. |
| Previous stable compatibility | `1.99.1`, newest published patch of the previous minor line; ongoing upstream support status not established | `3.14.10`, supported bugfix line | Rust local workload passed using the same baseline run. Python lane unexecuted; artifact digest not established. |
| Separately promised older consumers | Not established | Not established | Consumer inventory and any additional required checks remain owned gaps. |
| Exact accepted rollback set | Not established | Not established | Historical receipts retained; no rollback recovery exercise claimed. |

The retained Rust manifests pin the selected `x86_64-unknown-linux-gnu` full
`.tar.xz` distributions to these SHA-256 values:

- Stable `1.100.0`: `98ee6bec240e1cfe944f17e7092412dc5ef6e008484bb668c6936d35a7d15acd`.
- Beta `1.101.0-beta.2`: `86eb70ada7f97ce39df670733e92a942047fbbee5fc39b81f6f2fa0643651adc`.
- Nightly `2026-11-15`: `2b7c1b434dc3bed75aa47b1bd05ec2d24edf01bbc1a3ffa2d9a4895962658aef`.

These are upstream manifest declarations; the toolchain archives were not
downloaded or independently hash-verified. The additional retained `1.98.0`
manifest was an exploratory lookup and is not the previous-stable lane selection.

Primary sources checked: [Rust release announcements](https://blog.rust-lang.org/releases/),
[Rust stable manifest](https://static.rust-lang.org/dist/channel-rust-stable.toml),
[Rust beta manifest](https://static.rust-lang.org/dist/channel-rust-beta.toml),
[Rust nightly manifest](https://static.rust-lang.org/dist/channel-rust-nightly.toml),
[Python downloads and supported lines](https://www.python.org/downloads/), and
[Python 3.16.0a2](https://www.python.org/downloads/release/python-3160a2/).
Retained manifest bytes, retrieval times and manifest hashes are in
`rust-*.toml` and `upstream-rust.json`. Recheck upstream weekly and before any
migration decision; next adoption review is due 2026-11-22.

## Verification receipt and CI mapping

Evidence packet:
`/home/neo/Projects/kairotensor-artifacts/reliability-implementation-20261115/task-a0/`.
The machine-readable summary is `verification-receipt.json`; each command and log
path is retained in `ci-checks.json` and `rust-tests.json`.

Local `kairotensor-storage preflight --budget-gib 2 --output-dir <packet>` passed.
Cargo commands used `kairotensor-cargo --budget-gib 2` with
`KAIROTENSOR_REQUIRE_LINUX_CLOCK=1`. Output was keyed by checkout, source, runtime,
OS/architecture label and lock hash. The explicitly authorized project storage
exception was active at verification time, retaining a 20 GiB emergency floor
until 2026-11-17 18:04:38 UTC. Historical reservations and receipts were preserved.

| Existing `contracts` CI workload | Local result |
| --- | --- |
| `cargo test --locked --all-targets` | 185 passed across 24 test binaries; 0 failures, 0 ignored |
| All 12 existing `cargo run --locked` invocations | Passed, including invalid replay, Store-backed trace collection and timed fixture Adapter |
| Python reference contracts | 18 passed; oracle 8 passed |
| Full `scripts/test_*.py` discovery | 203 scheduled, 175 passed, 28 skipped because `KAIROTENSOR_RECOVERY_RUNNER` was absent in that initial CI-equivalent step |
| Compiled-runner discovery of `test_recovery_policy_runner.py` and `test_audit_recovery_bundle.py` | 15 and 19 passed respectively, 0 skipped; these exercise all 28 initially skipped tests plus 6 already runnable checks |
| Archived output caps, recovery policy fixture and collected trace scoring | Passed |
| Calibration, routing preparation, preflight, offline controller and fake Transformers fixtures | Passed; zero new model calls or provider spending |

All 25 example/Python CI commands following the Rust suite exited zero. The
compiled-runner environment used the wrapper's exact keyed executable path in
place of CI's default `target/debug/examples/recovery_policy_eval`. No validation
was weakened. Existing Rust dead-code warnings and two Python unclosed SQLite
connection `ResourceWarning`s remain visible in the logs.

The baseline missing-lock refusal and original offline-cache failure remain
unchanged under
`/home/neo/Projects/kairotensor-artifacts/reliability-review-20261115/` as
`rust-baseline.*` and `rust-resolved-baseline.*`; `historical-receipts.json`
records their identities. A new offline full-metadata inspection also exposed
uncached optional `syn 2.0.119`; the retained `cargo-metadata.stderr.log` records
that failure. Locked metadata with network access then succeeded without changing
the resolution. A committed lock enables reproducible resolution, not a promise
that every optional dependency is available offline.

`.github/workflows/check.yml` retains its existing `contracts` job and gates,
adding only `--locked` to its 13 Cargo invocations (one test and twelve runs).
It still uses `ubuntu-latest` with unpinned runner-provided Rust/Python. This task
performed local verification only; no fresh hosted CI run is claimed. Latest
stable and preview jobs, immutable runner/runtime pins, supported consumers,
service dependencies, accepted rollback artifacts, promotion procedure, isolated
rollback exercise, observation window, retirement date and owner acceptance are
not established. The project technical owner owns these adoption gaps, with
independent review required before promotion. Adding `--locked` does not close
them or retire historical pins or rollback assets.

## G1 isolated qualification — 2026-11-16

The preceding A0 record is historical and remains unchanged. This section binds
new local offline checks to their actual source; it does not activate a runtime
selection, claim hosted CI, qualify model workloads or establish a live service.

Accepted entry/rollback source S0 is
`794f64e5f9702987cd2502b86776b28f8a8d97c5`. The unchanged current lock SHA-256 is
`72282fc2776662e44b2e7b4003c2c582211296d663fdaecbd5b768dc09bc63aa`.
The initial verifier source S1 was
`26045ce65a3d1b495f50635da28d961ef71058bb`. Its previous-Rust attempt remains
Failed: all application commands it launched exited zero, but the new verifier
incorrectly rejected an empty successful controller stderr log. Root authorized
a narrow correction: recursively hash every artifact file, permit empty logs
beside at least one nonempty directory result, and continue rejecting missing or
all-empty artifacts and explicitly required empty result files. Regression tests
were added before the correction (`353ff2f2673b59df8402bf8dd3e10e4735a40470`).
Subsequent retained revisions corrected JSON tuple/list receipt persistence
(`ab8e162d335eabd11006b4a9d8bd9f3482dbdbb6`) and rejected contradictory
recorded cwd, runtime environment, runner and child-Python provenance. The final
common executable source is `dd52954fff73484c7fe7dafc474ac706af69c59d`, with 42
verifier regressions. Earlier receipts remain historical; the documentation-only
successor does not relabel them.

The packet is
`/home/neo/Projects/kairotensor-artifacts/reliability-implementation-20261115/task-g1/`.
`task-1-report.md` inventories every executed command and retained failure.
`required-command-inventory.json`, its independent root review, and the final
source manifest bind the unchanged 40-command workload. This includes all of
`check.yml`, compiled recovery/decision/policy runners, all-target locked Rust,
reference/oracle suites, all Python suites with explicit runner paths, offline
examples/audits/activation fixtures, E1 planning/recorded scoring and the new
verifier unit suite. Focused suites repeat subsets of full discovery; their
counts are reported separately and are not added as distinct tests.

These checks establish internally consistent local attestations, not
cryptographic issuer authentication or proof of host execution. The verifier
accepts an explicit `kairotensor-runtime-matrix/1` manifest. Its
`source` binds a committed revision, every tracked-file digest and the lock.
Five `lanes` bind roles, registered checkout paths, absolute actual Rust/Cargo/
Python executables, expected version output, SHA-256 and archive/closure
provenance. Wrapper/guard/recipe/native-library and external E1 input identities
are explicit. Mandatory `host_path` freezes ordered nonempty absolute PATH
directories. Command cwd and selected environment are checked; runner identities
come from the retained compiler-artifact log; structured parent/child Python
real paths and exact versions, raw probe output/exit and alias targets are
verified. `commands` must equal the complete built-in inventory. The verifier
never installs a runtime or substitutes a PATH runtime; it creates verified
private aliases for child Rust/Python selection. Missing prerequisites are
Unexecuted, started failures remain Failed, and a Pass requires every required
command/result, nonzero test counts, no skipped/ignored/filtered tests, exact
source before/after, unchanged identities and independently re-readable logs
and artifacts. Receipts bind the complete manifest, not only a reported status.
Fake executable tests exercise refusals and command identity only; they are not
runtime qualification evidence.

Use `python3 scripts/verify_runtime_matrix.py --manifest <manifest.json>
--output <new-evidence-directory>` for a new qualification run, or add
`--lane <role> --verify-receipt <receipt.json>` to validate an existing lane's
bound artifacts. The Python used to invoke the verifier is operational; each
workload interpreter is selected by the manifest. These are qualification
interfaces, not G2's future ordinary-development launcher or accepted selection.

### Runtime and source evidence

Intake refreshed primary Rust stable/nightly manifests and official release
records, plus Python's supported-release list and latest previous/preview
redirects. Selections were Rust 1.100.0; nightly-2026-11-16 / 1.102.0-nightly
(commit `5f4effd57`, replacing the prior dated Nov15 research selection); previous
1.99.1; Python 3.15.2, 3.16.0a2 and 3.14.10. Ongoing support for the previous
Rust line remains unknown. Raw bytes, response times and digests are retained
under `upstream-intake/`, with exact component URLs/digests in
`selected-upstream-identities.json`. Download declarations are distinguished from
verified archives and actual launched binaries.

The measured host is NixOS 26.05 (Yarara), build
`26.05.20261108.ac588ef`, Linux x86_64. The wrapper's `nixos26.05-x86_64`
output component is a static label, not a fresh host measurement. Actual binary,
ELF loader, library, standard-library import and native GCC identities are
retained in runtime inspection receipts. Compiler identities use separate
registered checkouts/keys. Python-varied reuse requires unchanged compiler,
source, lock, native dependencies and profile, with every test process rerun.
All Cargo operations used the approved `kairotensor-cargo --budget-gib 2` route,
zero debug symbols and disabled incremental compilation. Host defaults, existing
runtime slots, source contracts, dependency pins and CI were preserved.

| Role | Actual Rust | Actual Python | Final source result |
| --- | --- | --- | --- |
| stable | `rustc 1.100.0 (0ef1bc386 2026-11-09)` | `Python 3.15.2` | Pass: 40/40 commands; CLI revalidation exit 0 |
| rust-upcoming | `rustc 1.102.0-nightly (5f4effd57 2026-11-13)` | `Python 3.15.2` | Pass: 40/40 commands; CLI revalidation exit 0 |
| rust-previous | `rustc 1.99.1 (58cf8f9c3 2026-10-13)` | `Python 3.15.2` | Pass: 40/40 commands; CLI revalidation exit 0 |
| python-upcoming | `rustc 1.100.0 (0ef1bc386 2026-11-09)` | `Python 3.16.0a2` | Pass: 40/40 commands; CLI revalidation exit 0 |
| python-previous | `rustc 1.100.0 (0ef1bc386 2026-11-09)` | `Python 3.14.10` | Pass: 40/40 commands; CLI revalidation exit 0 |

All five rows bind the original immutable `matrix-qualified.json` and their
`qualified-v2/matrix/<role>/receipt.json`. Every row ran 357 Rust tests, 18
reference tests, 8 oracle tests and 335 full Python tests (293 existing plus
42 verifier tests), with zero skips/ignored/filtered tests. Focused counts
24/42/15/19/7 repeat subsets. `final-matrix-rollup.json` binds all five receipt
SHAs, source/lock and unchanged-runtime equality; `all-final-command-results.json`
contains every exact argv/environment/cwd/exit/log/artifact/count record. All
200 final command executions exited zero. Preview Python uses Astral
python-build-standalone release 20261110, a third-party CPython build, and passed
actual loader/standard-library checks without patching. No hosted CI is claimed.

### Executed recovery and retention

The isolated registered `g1-rollback-s0` checkout built S0's original locked
library with retained Rust 1.99.1 and Python 3.15.2. An explicitly authorized,
artifact-only `rollback_probe.rs` created one accepted current branch and a
separate claimed unresolved recovery through actual Store APIs. The closed
database was copied to backup and restored, then reopened twice through Store.
Both checks returned the exact accepted bytes, spent 7/reserved 12, the same
unresolved attempt, no second claim and `cancel=false`; no backend was invoked.
Original, backup and restored final bytes remained identical. The fixture digest
is `b32b32f06e8f8fe78211560a3ad76c16aae4d8b55060513323fd7533441a8e29`.
This proves the bounded local fixture restore, not reversal of external effects.

The full old Rust sysroot was copied under the unique G1 runtime prefix and all
210 file hashes matched. Eight explicit Nix GC roots retain Python and native
closures, with closure metadata and loader identities. Operational receipts are
under `/home/neo/.local/state/kairotensor/runtime/g1-20261116-s0794f64e/`, with
provenance copies in the packet. No retirement date or deletion is authorized.
The first probe compile's incorrect dependency search path and all source/lane/
acquisition failures remain retained; later success does not overwrite them.

### Owned gaps and next review

G2 owns portable pins, normal project launch selection, CI changes, independent
acceptance and any local accepted-selection activation. No such activation
occurred in G1. Seven-day observation dates begin only at an actual activation;
retirement requires a separate owner decision. Root is the technical owner
acting under Alejandro's project authority; the next weekly source-freshness and
gap review is due 2026-11-23, and freshness must be checked again before promotion.

The six other workflows (`benchmark`, `handoff`, `recovery-calibration`,
`recovery-policy`, `registration-cache`, `token-caps`), historical explicit ML
Python 3.11 environments, hosted execution, model/provider consumers and any
separately promised older consumers remain unqualified by this core matrix.
A future root Rust toolchain pin can affect workflows that invoke rustup-managed
Cargo; each workflow still needs its own runtime/consumer evidence. No model
inference, provider calls, dependency bulk update, storage exception extension,
cleanup, old-runtime removal or live database work occurred. Inherited Rust
warnings and bounded metadata/tooling failures remain in the logs.

One operational evidence defect is retained in `controller-log-collision.json`: an
older controller progress log and standalone preflight display were overwritten
while adapting an operational script. The progress log was reconstructed into a
new path and matched its previously retained SHA exactly. The old standalone
preflight display bytes could not be recovered; its original exit/SHA and all
inner command/admission receipts remain. The final matrix uses a fresh exclusively
created packet with direct process exits and complete fresh admission evidence.
The missing historical display is not a passing receipt or silently reconstructed.

## G1 review fix 1 — unavailable lane isolation, 2026-11-16

The preceding G1 records remain historical. Independent review found that a
missing archive or checkout in an unselected lane could abort the CLI before the
selected available lane received any result. Executable successor `d60159d6bda1e36ceecace4125f05059c07cbb50`
separates shared/structural contracts from each selected lane's availability.
Only requested roles run. Missing lane prerequisites emit Unexecuted receipts;
independent available roles continue. Dependent Python rows emit Unexecuted when
the same-manifest stable receipt is missing, failed or cannot be validated.
Selected-lane digest mismatches are Failed; malformed shared or role contracts
still refuse.

Eight additional public-CLI regressions bring the verifier suite to 50 tests.
They use actual subprocesses, tiny fixture tools and a real Python parent/child;
the reachable workload intentionally exits 23, so these refusal/isolation tests
cannot be confused with runtime qualification. RED and GREEN logs are retained.
The approved 40-command workload and acceptance predicates are unchanged.

The new packet is `fix-1/` within the G1 artifact directory above.
`manifest.json`, `source-freeze.json`, `execution/process-receipts.json`,
`final-matrix-rollup.json` and `all-command-results.json` bind these fresh runs:

| Role | Rust | Python | Commands | Receipt CLI exit |
| --- | --- | --- | --- | --- |
| stable | `rustc 1.100.0 (0ef1bc386 2026-11-09)` | `Python 3.15.2` | 40/40 Pass | 0 |
| rust-upcoming | `rustc 1.102.0-nightly (5f4effd57 2026-11-13)` | `Python 3.15.2` | 40/40 Pass | 0 |
| rust-previous | `rustc 1.99.1 (58cf8f9c3 2026-10-13)` | `Python 3.15.2` | 40/40 Pass | 0 |
| python-upcoming | `rustc 1.100.0 (0ef1bc386 2026-11-09)` | `Python 3.16.0a2` | 40/40 Pass | 0 |
| python-previous | `rustc 1.100.0 (0ef1bc386 2026-11-09)` | `Python 3.14.10` | 40/40 Pass | 0 |

Each row ran 357 Rust tests, 18 reference tests, 8 oracle tests and 343 full
Python tests (293 existing plus 50 verifier tests), with zero skipped, ignored or
filtered tests. Focused suites repeat subsets. All 200 workload commands, all
five separate persisted-receipt CLI validations and the outer controller exited
zero. Three new registered frozen worktrees bind the same source/lock; Python
rows reuse only the unchanged stable compiler key and rerun every test process.
The documentation-only successor does not relabel these source-bound receipts.

`provenance-role-mapping.json` proves equality of the entire prior 78-file
provenance/native identity union. Runtime-specific archives and libraries now
belong to their consuming roles; 27 host/compiler native files remain shared.
This prevents accidental cross-lane availability requirements without omitting
an identity. Mapping follows the actual selected tool installation prefixes and
is bounded to this frozen fixture workload, not discovery of arbitrary future
native dependencies. No runtime, dependency, guard, CI or accepted-selection
change occurred, and no download or repeat restore was needed. The retained S0
restore evidence and eight GC roots/closures were rechecked unchanged.

The lost historical standalone preflight display remains a preservation defect:
its original SHA-256 is
`8795475d9b4184453f7c0653f9a594ab4690a4113e986f000aa35b69d7f14f86`.
Its bytes have not been recovered. Fix-1 uses exclusively created operation
paths and fresh admission receipts; it makes no claim to reconstruct that loss.
Independent scoped review remains required. Other adoption gaps and G2's
activation/observation responsibilities above are unchanged.

## G2 project runtime selection (2026-11-16)

The portable defaults are Rust `1.100.0` (`minimal` profile: Rust/Cargo/std;
no optional rustfmt/clippy component) and Python `3.15.2`. They follow accepted
G1 qualification at executable source
`d60159d6bda1e36ceecace4125f05059c07cbb50`, documented by successor
`7684b4fd4148145f882a1e2e3fc3e2f2833d559e`. G2's final source and results are
recorded below after execution. G1's other-role full suites remain S1 evidence.
No local activation or hosted execution is implied by these files.

Use the durable project launcher from a project checkout:

```sh
python3 scripts/project_runtime.py identity
python3 scripts/project_runtime.py python -m unittest discover -s reference -p test_memory.py
python3 scripts/project_runtime.py --budget-gib 2 cargo test --locked --all-targets
```

The bootstrap Python only reads and validates selection metadata. Actual workload
Python, child `python3`, Rust and Cargo use the verified selected binaries. The
host Python command and `.python-version` alone do not establish child selection.
The host Rust commands are rustup proxies: actual checks with auto-install disabled
returned exit 1, because `1.100.0` is absent in the host rustup home. The qualified
copy exists separately under project development storage. Use the launcher; do not
change host defaults or allow a proxy to download a substitute. Bootstrap requires
Python 3.11 or newer (stdlib `hashlib.file_digest`).

The default pointer is
`~/.local/state/kairotensor/runtime/accepted-selection.json` (or the equivalent
`XDG_STATE_HOME` path). It is root-owned operational acceptance state, not a file
committed with workstation paths into portable CI. Before independent review use
`--selection /home/neo/.local/state/kairotensor/runtime/candidates/g2-20261116-r1/stable.json`.
Missing selection/tool, changed hash/version, unexpected inherited runtime override,
unbound Cargo config or incompatible qualification binding fails closed with exit
2. A refused storage admission propagates its actual exit. No automatic install,
download, fallback, host update or runtime retirement is performed.

The local JSON schema is `kairotensor-project-runtime/1`. Required fields:

- `role`: the exact role in the qualified five-role manifest.
- `tools`: `rustc`, `cargo`, `python`, each with absolute `path`, SHA-256
  `sha256`, exact stdout `version`, and `version_args` array. These must match
  that role's immutable qualification identities.
- `host_tools`: exact resolved absolute paths and hashes for `cc`, `gcc`, `c++`,
  `ar`, `ld`, `git`, `sh`, `bash`, `dirname`, `tr`, `sha256sum`, `cut`, `mkdir`,
  and `mktemp`. Resolution must agree with the qualification manifest's frozen
  `host_path`; `cc`/`gcc` must also match its native-file inventory. Private aliases
  bind these helpers, and Git source inspection uses that same verified environment.
  Ambient PATH additions cannot select replacement native or Git commands.
- `qualification_manifest`, `qualification_receipt`, `independent_review`,
  `acceptance`: each an absolute `path` and `sha256`. The receipt must be Pass
  for the manifest's source and role; the acceptance binds that source and review.
  The manifest additionally binds selected native/closure provenance, approved
  Cargo wrapper, storage guard and recipe; those files are rechecked before launch.

The G1 acceptance is `task-g1/fix-1/root-fix1-acceptance.json` in the retained
implementation packet. A selection can identify accepted G1 runtimes during G2
candidate testing; it does not declare G2 independently accepted. Root controls
activation after G2 review. These are local trusted-owner artifacts: content hashes
bind bytes, not issuer authentication. The launcher is an invocation boundary, not
a security sandbox for arbitrary Python, Cargo build scripts or executable code.

Each launch creates a private alias directory and receipt under
`~/.local/state/kairotensor/runtime/invocations/`. It records actual resolved paths,
versions/digests, parent/child Python, selected profile, wrapper, current commit,
tracked-file hashes, dirty/untracked status, current Cargo.lock, command and exit.
The inherited environment is not logged. Keep credentials out of command arguments.
Editable source is allowed and always marked `qualification: not assessed`;
it never inherits the frozen source's successful workload. Requested Cargo commands
route through the approved `kairotensor-cargo --budget-gib N` wrapper; debug symbols
and incremental compilation are disabled. The launcher accepts Cargo build/test/run/check/metadata/locate-project/version
commands. Install, direct rustc/rustdoc forwarding, runtime/output/profile overrides
(including `-r` and clustered short flags), and Cargo configs require a separate
reviewed binding. Use long option forms; only `-q`, `-v`, `-vv`, and `-V` shorthand
is accepted. Native helper and Git environment overrides also refuse. The wrapper handles fresh
storage admission; refusal blocks heavy work. Its existing exception expires
2026-11-17 18:04:38 UTC and must not be extended by this launcher.

`check.yml` has five independently varied roles: stable `1.100.0 / 3.15.2`, Rust
upcoming `nightly-2026-11-16 / 3.15.2`, Rust previous and retained baseline
`1.99.1 / 3.15.2`, Python upcoming `1.100.0 / 3.16.0a2`, and Python previous
`1.100.0 / 3.14.10`. The setup-python preview selector is exactly
`3.16.0-alpha.2`, as advertised by its build manifest; observed Python must report
`3.16.0a2`. Each row explicitly sets `RUSTUP_TOOLCHAIN` to override the root pin,
checks Rust release/commit and Cargo version, and checks parent/child Python.
The previous-Rust pair also covers the distinct accepted development baseline;
no redundant sixth row is needed. Failures remain visible (`fail-fast: false`,
no `continue-on-error`). Original checkout identity and all command gates remain.

Setup actions are pinned to verified upstream source: setup-python v7.1.0 at
`6d4fd22b20465ccc0f7354476588e8df7c9ca0e0` and rust-toolchain at
`df2a88e9fe4098bad09cb42c3dec7187f15861fc`. Primary metadata was retained on
2026-11-16 in `runtime-actions-preflight-20261116/` and
`runtime-python-ci-preflight-20261116/`; runtime freshness was reconfirmed by root
before G2 intake. `ubuntu-latest` is mutable; these pins do not prove hosted
binaries equal the locally qualified Nix/standalone artifacts. Hosted CI is
unexecuted; this task neither pushes nor triggers it.

Six separate workflows remain owned adoption gaps: benchmark, handoff,
recovery-calibration, recovery-policy, registration-cache and token-caps. Their
files and old receipts are unchanged. Future Cargo commands inside a candidate
checkout can now resolve the root Rust pin; the registration-cache workflow runs
from its parent with `--manifest-path`, so candidate/control Rust selection needs
explicit follow-up rather than assuming the nested pin selects it. The four ML
measurement workflows (handoff, recovery-calibration, recovery-policy, token-caps)
retain Python `3.11` and have no real-model qualification under this core matrix.
Accepted checkout v4.2.2 is intentionally retained; adopting latest v7.0.1 is a
separate owned gap. Previous Rust upstream support and the complete separately
promised consumer inventory remain not established. No model, deployment,
shared publication or bulk dependency change is covered here.

Root, acting under Alejandro's autonomous project authority, owns review and
activation. Retain any prior pointer, independently review G2, then atomically
replace `accepted-selection.json` with the reviewed candidate bytes. Run launcher
identity/child-Python verification and the candidate-built disposable Store
current-read/hold probe. Record activation time and an exact review date seven
days later; freshness and the six workflow/ML/consumer gaps are reviewed then and
weekly. Until activation, next review is due 2026-11-23. No retirement date is
authorized. Preserve old binaries/sysroots, Nix roots, source, locks and failed
receipts. If checks fail, retain failure evidence and restore the exact previous
selection pointer; when no prior pointer existed, remove only the new activation
pointer as a root-controlled reversal. For source/data recovery use the retained
S0 source `794f64e5f9702987cd2502b86776b28f8a8d97c5`, exact Rust `1.99.1` /
Python `3.15.2` closure and G1's exercised rollback procedure; switching a pointer
does not undo data mutations. The G2 probe uses a fresh disposable database and
never mutates the original rollback fixture.


### Final G2 local verification and source mapping (2026-11-16)

Executable/configuration source is `64e0b1b4a45857f419ac9539760018e3890d9aa3`. It supersedes
initial S2 `ae4aa0865912ac5f991a08ddfe379e5a180f7bfa`: root identified that ambient
PATH could shadow native/Git helpers. Regression tests reproduced that problem;
the successor binds verified helper aliases from qualification's frozen host PATH
and also rejects Cargo shorthand/forwarded selector bypasses. Both sources and
all their evidence are retained. The final docs-only successor changes only this
file and is mapped in the packet's `docs-only-transition.json`; no old receipt is
relabelled as evidence for new executable code.

Final evidence is retained under
`/home/neo/Projects/kairotensor-artifacts/reliability-implementation-20261115/task-g2/successor-r1/`.
`verification-rollup.json` binds the complete source, actual command records,
logs, candidate selections and probe identities. The stable manifest and CLI
receipt revalidation passed with the unchanged 40-command G1 workload, all exits
zero: 357 Rust tests, 18 reference tests, 8 oracle tests, 361 full-discovery Python
tests, and explicit suites of 24/50/15/19/7 tests, with zero skips. Counts across
those overlapping commands total 859 executions; they are not 859 unique tests.
Five selected runtime pairs each passed an actual identity/child-Python check and
18 focused launcher tests (90 additional test executions). Other full role suites
remain attributed to accepted G1 source; no final-source five-role full matrix or
hosted execution is claimed.

CI YAML was parsed, its shell and Python probe syntax checked, five independent
roles verified, and all 16 original command steps plus checkout pin compared
unchanged. Actionlint was unavailable locally and was not downloaded. Actual
hosted action execution remains open. All 196 protected repository files, four
owner rule files and bound intake inputs still matched; the retained rollback
DBs, eight Nix roots/closures and host defaults were verified unchanged.

The accepted-pointer candidate is `/home/neo/.local/state/kairotensor/runtime/candidates/g2-20261116-r1/stable.json`,
SHA-256 `aaa7438718dbed6e6f9b7ac50dd7fbb02317278c18a8f093a431d488ba998321`. It is prepared for independent review and root
activation; `accepted-selection.json` remained absent at worker closeout.
The final candidate-built activation binary is `/home/neo/Projects/kairotensor-artifacts/reliability-implementation-20261115/task-g2/successor-r1/activation-probe/activation-probe`,
SHA-256 `275b94eef822d82bbfcfa3be1b2e7b0d40a6fb49d737a2ad59cde918e6b8c666`. Its source is the intake-authorized exact
copy of G1's probe. Library build used the candidate launcher and approved Cargo
wrapper; standalone compile used separate fresh storage admission. Disposable
create/reopen checks passed exact current bytes, spent=7, reserved=12, unresolved
attempt=2, claim=None, cancel=false, and zero backend calls. This is preactivation
candidate health; root must run the post-activation check on a new database.

The initial probe compile failed (exit 1, E0463): its old harness assumed
`debug/deps`, while Cargo 1.100 emitted per-package `debug/build/*/*/out`
dependency directories. The failed receipt remains under `activation-probe/`.
An artifact-only correction used actual Cargo-emitted paths; both its retained
S2 retry and the final successor probe passed. No application source, dependency,
storage guard or acceptance test was changed to clear the failure.

### G2 independent-review corrections (2026-11-16, fix round 1)

Independent review I1 reproduced an inherited native flag outside the declared
runtime boundary: the selected Nix gcc wrapper consumed
`NIX_CFLAGS_COMPILE_x86_64_unknown_linux_gnu` while its executable hash stayed
unchanged. Acceptance and activation remained held. The correction rejects
native input families before creating execution aliases, including arbitrary
Nix target/role suffixes for compile/before/link flags, C++/GNAT flags, hardening,
wrapper role/initialization controls, linker selection, rpath/build-ID/purity and
response-file controls. It also rejects the cc crate's bare, HOST_/TARGET_, raw
and normalized target-specific compiler/archiver/tool/flag selectors, native
search-path controls and the bundled SQLite configuration inputs. Empty native
selectors are refused too because some explicitly disable defaults. No target
triple is hardcoded into the refusal policy. Selection schema and accepted
G1/storage code are unchanged; this remains an invocation boundary rather than
a sandbox for arbitrary workload code. Exact audited native source identities,
pre-fix preprocessing reproduction and RED/GREEN results are in `task-g2/fix-1/`.

Review M1 also found retained compiler warnings that the prior summary omitted:
17 warning lines, including repeated diagnostics and Cargo-generated summaries,
cover three existing dead-code categories in the protected shared examples:
unused `Prepared`, unused `PreparedReply`, and unused associated methods.
Passing exits do not mean warning-free builds. These warnings remain visible in
the raw Rust suite and handoff logs. Application cleanup is explicitly deferred
to the integrated review owner; G2 neither changes protected application code
nor suppresses warnings or weakens acceptance gates.


### G2 fix-round-1 source-bound verification

Corrected executable source `e32c96814bbf0e841b9bacda7b5edc241e72fbf4` supersedes the earlier
`64e0b1b` proposal; earlier sources and receipts remain historical. A subsequent
docs-only commit changes only this file and is mapped in
`task-g2/fix-1/docs-only-transition.json`. Independent re-review and root activation
remain pending; no accepted pointer was written.

`task-g2/fix-1/verification-rollup.json` binds a fresh unchanged stable
40-command workload (all exits 0, zero skips), with Rust 357, reference 18,
oracle 8, full Python discovery 365, and explicit suites 24/50/15/19/7.
These overlapping suites total 863 test executions. Five actual selected-runtime
identity probes and five focused 22-test runs also passed (110 executions).
The source-bound final negative control refused the exact reviewed suffixed
Nix flag with exit 2 before launching the selected compiler. The new regression
coverage checks 258 native-input cases, including the end-to-end observed case;
220 subcases failed in the retained pre-fix RED and all 22 tests passed GREEN.
The audited selected shell wrappers expose 50 distinct native-variable-name/file pairs
in the inspected files, all classified by the new refusal boundary; this is
an audit of those selected wrappers, not a claim about arbitrary external code.

Fresh candidate library and standalone probe builds used unchanged admitted
wrappers. The final candidate binary is `/home/neo/Projects/kairotensor-artifacts/reliability-implementation-20261115/task-g2/fix-1/activation-probe/activation-probe`,
SHA-256 `275b94eef822d82bbfcfa3be1b2e7b0d40a6fb49d737a2ad59cde918e6b8c666`. Its current-read/hold create/reopen checks
passed on a new disposable database, with zero backend calls. This replaces the
earlier probe proposal for root's post-activation check. The accepted-runtime
candidate selection remains byte-identical at
`/home/neo/.local/state/kairotensor/runtime/candidates/g2-20261116-r1/stable.json`;
root must bind the corrected executable source and this binary in its own
reviewed activation receipt.

The fresh warning audit (`task-g2/fix-1/compiler-warnings.json`) confirms the
same 17 lines, including repetitions and generated summaries, in the Rust suite
and corrected/scoped handoff stderr. The three protected-code dead-code
categories remain deferred to integrated review. All 199 files protected by the
fix intake, four owner policies, review/context identities and candidate selection
bytes were checked unchanged. Hosted CI, other full S2 roles, real-model/ML
consumers and activation remain unexecuted, as before.

### Final integrated-review successor: selection V2 and replay report V2

The final correction uses executable source
`5e9c76e0b95fa2559fc594523fd9739263577a1b` (S3). The subsequent documentation-only
successor is mapped in `final-fix/docs-only-transition.json`; it changes only
`docs/runtime-adoption.md`, `docs/decision-bundle-v1.md`, and
`docs/decision-replay.md`. Historical G1/G2 qualification records, old source,
selections and accepted pointer remain intact. This section supersedes the earlier
runtime proposal for final acceptance; activation remains a root action after
independent scoped review.

`kairotensor-project-runtime/2` retains the exact G1-qualified rustc, Cargo,
Python, native/helper, review and acceptance identities and requires exactly one
additional tool: the selected rustc path's sibling `rustdoc`. Its path and SHA-256
are bound, its arguments are fixed to `--version`, and its version must match the
release/commit/date derived from the qualified rustc's first version line. The
launcher verifies the actual rustdoc hash and version, puts it on the private
PATH and sets `RUSTDOC`, alongside `RUSTC`. Invocation receipts include its actual
identity and explicitly identify it as supplementary selection-V2 evidence;
G1 never tested or recorded rustdoc. Missing or additional runtime tool keys,
wrong sibling/hash/version/arguments, and inherited `RUSTDOC`, `RUSTDOCFLAGS`,
`CARGO_ENCODED_RUSTDOCFLAGS` or `CARGO_BUILD_RUSTDOC` (including empty values) refuse.
Existing Cargo config/selector and native environment refusals remain in force.

Five new candidate selections reside under
`/home/neo/.local/state/kairotensor/runtime/candidates/final-fix-20261116/`.
The original V1 accepted pointer is deliberately unchanged during review; the V2
launcher explicitly refuses it. Use `--selection` with a new V2 candidate for
qualification. Retained old source/checkouts and their V1 launcher continue to
use the old pointer. There is no silent fallback, installation or workstation
runtime-default change. Root must bind reviewed S3 evidence and the candidate
selection before changing that pointer, then test a fresh database.

The replay correction is independently versioned as `decision-replay-report/2`.
Known compatible units retain numeric totals; unknown units become ordered
per-attempt observations without addition. DecisionBundleV1 bytes, integrity,
per-attempt accounting, ledger writes and historical dispositions remain intact.
The two integrated-review input bundles are preserved and replayed by the new
source-bound binary under `final-fix/replay-probes/`. This is offline reconstruction
and contract checking, with no model calls or new semantic-quality claim.

The project Cargo documentation-test invocation passed with **zero doctests**:
that is tooling coverage, not an invented test. A separate, exact artifact source
(`final-fix/rustdoc_fixture.rs`) ran **one real passing doctest** through the V2
selected rustdoc with selected Rustc. Supplementary source, executable identities,
commands and results are retained in `final-fix/doc-probes/` and
`final-fix/rustdoc-fixture-final/`. The one-test artifact was repeated with
compiler temporary output explicitly declared under its evidence directory; both
passing results are retained, not counted as two distinct tests. All five qualified
runtime pairs also execute
actual V2 identity/child-Python probes and the focused 28-test launcher suite.

The new Store health probe is built from S3 and the exact approved probe source;
`final-fix/activation-probe/build-identities.json` binds compiler, library,
dependencies, source and binary. Its fresh disposable create/reopen observations
retain exact current bytes, spent 7, reserved 12 and unresolved attempt 2, with no
second claim/cancel and zero backend calls. They are candidate checks; root's
post-review activation must use another fresh database.

Two supplementary storage jobs initially refused because their retention lists
included existing regular files and the storage inspector requires directories.
Their manifests and failed outputs remain in `doc-probes/` and `activation-probe/`.
New manifests retain enclosing output directories under the same 0.05 GiB bounds;
exact file hashes remain separately bound. The admitted retry passed. No storage
guard, acceptance gate, application source or historical receipt was changed.

M1 remains a minor shared-example warning cleanup follow-up; protected adapter
code and warnings are retained. Hosted CI, real-model/ML consumers, external report
consumer inventory, seven-day observation and shared publication remain outside
this candidate's evidence. The prior lost preflight display remains an explicit
historical preservation defect. This work neither extends the storage exception
(expiry `2026-11-17T18:04:38Z`) nor authorizes reservation release or asset retirement.

#### S3 full runtime matrix

`final-fix/verification-rollup.json` binds all five final-S3 runtime pairs to the
unchanged 40-command workload and passing independent receipt revalidation:
**200 commands**, with **362 Rust, 371 full Python, 18 reference and 8 oracle**
checks per role, zero skips/ignored/filtered checks. Explicit overlapping suites
remain 24/50/15/19/7; the five full roles total 4370 test executions including
those overlaps. The five separate focused 28-test launcher suites add 140
executions. These are actual reruns at S3, not relabelled historical G1/G2 runs.

Three fresh registered Rust checkouts distinguish source/compiler/native keys.
Python-varied rows rerun against the exactly bound stable Rust key. All use the
original storage/Cargo wrappers with 2 GiB admission per fresh key; supplemental
probes retain their separate 0.05 GiB jobs. The warning inventory contains 17
lines per role (85 including repeated diagnostics and summaries across the full
matrix), all retained in `final-fix/compiler-warnings.json`. Passing matrix and
candidate health still require independent review and root activation.
