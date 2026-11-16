# Controlled local correction pilot

This is a separate experimental harness, **offline-qualified only until a LIVE
receipt exists**. It exercises real Rust Store capture, scoped model reuse,
withdrawal, typed alternative support and fresh recomputation. Its fixed data and
prompts are copied from `scoped_model_reuse`; no dataset expansion, incident
held-out evaluation, training or change to existing Store/validator behavior is
included. It is separate from the main submission pinned to `20fab84` and from
the fixture-only incident Store bridge.

The model candidate is cached `ornith-1.5:9b`, Q4_K_M, on Ollama 0.40.16. The
adapter fixes its selected variant digest and the supervisor pins the cached
container image and full model/config/projector/variant content hashes. This is
not the historical Qwen experiment or a claim of equal model behavior. No new
runtime upgrade is selected. The ML runtime compatibility matrix remains
unexecuted; core Rust/Python qualification does not cover it.

## Paired schedule

A new scratch Store registers BEFORE policy, two distinct GREEN observations A
and B, and an unadmitted RED replacement. Three real generations would produce
independent A and B report verdicts and the initial A-based release plan. The
closed Store checkpoint is copied into independent scenario stores. Full
recomputation uses a separate fresh Store without any prior model output.

| Condition | Store change | Repair calls | Full recomputation calls |
| --- | --- | ---: | ---: |
| Report correction | Withdraw A/B, admit RED | 2 | 2 |
| Unrelated policy change | BEFORE to AFTER; A/B remain admitted | 1 | 2 |
| Withdrawal | Withdraw A/B; no replacement | 0 | 0 |
| Alternative support | Withdraw A; B remains admitted | 1 | 2 |

The entire successful path has 13 generations including the shared three-call
bootstrap. The two skipped report calls are Store-authorized reuse hits with
original attempt lineage. The surviving B result can only use B's independently
validated origin, never A's withdrawn result. `A OR B` is checked against an
independent admission-set oracle; that deterministic support flag is not model
output and does not itself authorize opaque model reuse.

Repair/fresh order alternates by condition. Both tasks use the same model,
decoding parameters and source selection rule. The second arm's exact request
bytes must equal the first arm's corresponding bytes **before dispatch**. Both
checked structured outputs must agree. Actual planner inputs come from the
actual validated model verdict, never an expected-answer fixture. Frozen prompt
examples are unchanged. The existing scoped-example validator rejects duplicate
keys, unexpected fields and inconsistent values; it retains that example's
acceptance of an outer JSON Markdown fence, so this is not a claim of raw-JSON
compliance.

A rejected bootstrap stops execution and records all remaining slots and
conditions as unexecuted. Transport uncertainty also stops execution. There is
no retry, warmup, resampling, checkpoint resume or fallback model. A withdrawal
without surviving evidence causes infrastructure refusal before generation; it
is not a model's semantic abstention.

## Execution and authority

The public Python CLI supports `freeze`, `fixture`, and read-only `audit`. It has
no live option. The Rust `--bridge` mode opens no network connection; it emits
fixed requests on stdout and waits for measured replies on stdin.

A trusted root caller must construct `LiveApproval` separately from artifact
contents, binding the exact source commit, executable SHA256, configuration
SHA256 and independent-review SHA256, then call `run_live`. A JSON field cannot
grant execution. This object is a caller attestation, not an unforgeable
capability against arbitrary same-user code. Source must be clean and match the
reviewed freeze; runtime, binary and full cached assets are checked again.

Example offline qualification, using fresh project storage admission:

```sh
python3 scripts/project_runtime.py --budget-gib 2 cargo test --offline --locked \
  --example controlled_correction_pilot
python3 scripts/project_runtime.py python -m unittest discover -s scripts \
  -p test_controlled_correction_pilot.py
python3 scripts/project_runtime.py --budget-gib 2 cargo build --offline --locked \
  --example controlled_correction_pilot --message-format=json
```

For compiled IPC tests, set `KAIROTENSOR_PILOT_TEST_BINARY` to that exact built
example while running the Python suite. This test-only variable enables three
zero-model integration tests; without it, that class is explicitly skipped.
The public fixture command invokes the binary's deterministic adapter and labels
its accounting units synthetic:

```sh
python3 scripts/project_runtime.py python scripts/run_controlled_correction_pilot.py \
  fixture --binary /absolute/built/example --out /absolute/new/artifact-directory
python3 scripts/project_runtime.py python scripts/run_controlled_correction_pilot.py \
  freeze --binary /absolute/built/example --out /absolute/new/execution-freeze.json
```

Freeze only checks source/runtime/asset identities and does not start a model.
After review, root's trusted invocation is:

```python
from pathlib import Path
from run_controlled_correction_pilot import LiveApproval, run_live, sha
config = Path('/absolute/execution-freeze.json')
# These values come from root's independent review, not from an untrusted manifest.
approval = LiveApproval(reviewed_commit, reviewed_binary_sha256,
                        sha(config), reviewed_review_sha256)
result = run_live(config, Path('/absolute/new/live-directory'), approval=approval)
```

Invoke this through the accepted project Python and a fresh `workstation-storage`
(or still-valid scoped project guard) `preflight` plus `run` reservation. The
source freeze is not storage admission. Suggested artifact growth is <=32 MiB
with a 1 GiB reservation for the pilot/runtime layer; Cargo has a separate 2 GiB
reservation. A failed admission blocks the run. Preserve all failed receipts.
No dependency install, model pull, training job, provider key or shared-runtime
change is allowed.

## Runtime and bounds

The supervisor creates a uniquely named owned container from the cached image
with `--pull=never`, a read-only root and model mount, explicit writable `/tmp`
and `/root/.ollama` tmpfs, 2 CPU quota, inherited affinity (no fixed cpuset), 12 GiB RAM, no GPU
devices, one loaded model and one parallel request. The supported network backend is `pasta:--no-map-gw`. The endpoint is only
`127.0.0.1:11445`; the existing shared service is untouched. Local cloud mode is
disabled and the adapter exposes neither pull nor arbitrary endpoint/model
selection. Loopback publication and this API restriction do not establish a
network-egress sandbox; the container network still has ordinary outbound
connectivity. No credentials are mounted or read.

Readiness captures `/api/version`, `/api/tags`, `/api/show`, `/api/ps` and owned
container identity. It requires an empty loaded-model list initially. The cached
GGUF template explicitly supports disabling thinking. Every reply is followed by
`/api/ps`, requiring the selected model digest, `size_vram=0`, and observed
context length at most 2,048. Unexpected runtime identity stops execution after
retaining the already returned response/usage.

Fixed request settings are seed 0, temperature 0, top_k 1, top_p 1, 96 requested
output tokens, context 2,048, two threads, num_gpu 0, think false, truncate false,
shift false and nonstreaming output. No answer grammar or oracle is supplied.
The exact installed-version [request API source](https://raw.githubusercontent.com/ollama/ollama/v0.40.16/api/types.go)
exposes the runtime fields, while [Ollama's chat API](https://docs.ollama.com/api/chat)
documents usage and duration fields. Definitions and metadata alone do not prove
the actual loaded model honored every setting; receipts and runtime observations
are necessary and mismatches fail closed.

Limits are 13 durable generation claims, one in flight, 120 seconds of total elapsed time per HTTP
request (including slow streaming bodies), and one original 1,800-second BOOTTIME window starting before identity
verification/startup/load. No new work begins in the last 60 seconds. An
independent watchdog process observes the original absolute deadline and parent
pipe loss and kills only the named owned runtime. Conmon's container timeout starts at container startup and is a runtime-duration
backstop; the independent BOOTTIME watcher enforces the original absolute deadline. The Rust, HTTP transport and Podman creation children bind the expected parent PID
before fork and use parent-death SIGKILL. Normal cleanup kills/reaps owned process
groups. On parent loss during creation, the independent watcher kills the bound
startup group and reconciles a late container through the original deadline;
absence without observed stopped state is unresolved cleanup, never success. Deadline overrun is explicitly incomplete. Kernel hangs,
failed container-control commands and exact hard-real-time kill are not promised.
Stopped owned containers are retained for later explicit retirement.

**Token-envelope narrowing authorized by root:** Ollama chat has no trusted
pre-dispatch tokenizer quote here. Therefore 2,048 input tokens per request,
2,144 combined quote units per claim, and 27,872 aggregate quote units are
configured/observed accounting envelopes, **not proven hard token budgets or
provider billing bounds**. Every returned count is retained without clamping,
including excess. Excess, missing/faulty usage, unexpected thinking, nonfinal
responses and ambiguous transport stop the run. Unknown claims retain the
administrative 2,144-unit hold, with actual additional liability explicitly
unbounded/unknown. There is no automatic release or redispatch. Model-provider
spend is zero for this local path; workstation CPU/electricity are unpriced.

## Evidence and interpretation

The supervisor persists each claim before HTTP I/O. A separate transport subprocess
streams response chunks to the parent, which independently syncs them before parsing.
An absolute deadline kills/reaps that owned transport process, preserving partial
raw bytes. Readable counts in an uncertain response remain observed usage while
its hold stays unresolved. Known partial usage remains an observation and
does not reconcile unknown final liability. Records include immutable claims,
raw receipts, usage reconciliation, runtime readiness and per-call observations,
watchdog/cleanup results, Store SQLite files/events and final outcome scheduling.
Audit is read-only and refuses corrupt accounting rather than inventing totals.
Simultaneous storage loss cannot guarantee a receipt that existed only in memory;
a prior durable claim stays unresolved.

Rust Store charges for typed support checks use their existing deterministic
units; never sum those with provider token counts as though the entire Store
ledger were homogeneous. The external pilot ledger reports provider-observed
model usage separately. Invalid semantic/model replies remain charged and cannot
publish a head. A complete ledger does not establish a correct model answer.

Report marginal repair and full-recompute model usage separately (expected valid
path calls 4 versus 6), followed by the actual shared 3-call bootstrap and full
13-call aggregate. Attribute the same shared prefix to each hypothetical
lifecycle comparison without double-charging it in actual-run totals. Include
cold/load time, preparation, failed/partial/unexecuted outcomes and original
deadline. One correlated synthetic case per condition cannot establish a latency
distribution, production benefit, semantic generalization, model necessity,
incident LIVE Store integration or held-out efficacy. Arithmetic report verdicts
still have the existing deterministic product route.


## Qualification operational notes

Root's no-generation cached-runtime probe succeeded with `pasta:--no-map-gw`,
CPU quota 2 without a fixed cpuset, and the read-only/tmpfs settings above.
Version, selected digest, empty loaded-model state and template metadata were
verified, then that owned runtime was stopped. Earlier slirp4netns and cpuset
attempts were refused by the installed Podman/cgroup setup and retained as failed
receipts. These probes did not load or execute the model; CPU/token behavior
remains subject to actual run observations.

During implementation, a host `rustfmt` command unexpectedly resolved through
rustup, which announced three component downloads and auto-installed a missing
host Rust 1.100.0 toolchain before reporting rustfmt unavailable. The formatting
command did not run. This was outside the planned no-download scope; an incident
receipt is retained and no cleanup was performed. No model download occurred.
The accepted project runtime remains separately pinned and is rechecked before
handoff; this side effect must not be described as a download-free overall task.
