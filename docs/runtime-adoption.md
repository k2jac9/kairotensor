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
