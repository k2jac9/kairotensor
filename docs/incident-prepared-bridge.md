# Incident prepared-request bridge (offline only)

This milestone connects the frozen incident request renderer to the existing
worker's **prepare / generate / discard protocol** using an injected transport.
Tests run the actual, unchanged `handoff_model.RequestHandler` with a strict fake
tokenizer and fake generator. They prove retained tensor identity, exact reported
quotes and single handle consumption. They do not load Qwen, measure real tokens,
validate model quality or exercise Rust Store admission.

The historical worker and Rust Adapter are unchanged. Their worker envelope uses
`transformers-cpu/v1`, Qwen/Qwen2.5-1.5B-Instruct at revision
`989aa7980e4cf806f80c7fef2b1adb7bc71aa306`, BF16, the generation chat-template flag,
no sampling, one beam and a 96-token output cap.

## Interfaces and trust boundaries

`scripts/incident_preflight.py` provides:

- `build_request(case_id, invalid_output=None)`: reloads the frozen workload and
  renders a calibration case into the complete canonical worker envelope. It
  supports only `cal-01` through `cal-04`. Feedback appends the prior invalid text
  as an assistant message and a public format-only user instruction.
- `validate_envelope(request)`: checks exact envelope fields, pinned model,
  revision, dtype, decoding and initial/feedback message roles. It deliberately
  does not prove that arbitrary supplied message contents came from the frozen
  corpus. Use `build_request` for that preparation path.
- `PreparedBridge(transport)`: owns one exclusive worker transport on its creating
  thread. The injected callable exchanges JSON-like command/response objects.
  This module contains no subprocess, networking, model launch or automatic retry.
- `prepare`, `generate`, `discard`: return or consume a locally bound immutable
  ticket containing exact request bytes/digest, worker handle, input count and cap.

The client verifies strict integer counts (not booleans), input up to 2048, cap96,
completion flags and measured input equality. Output includes generated tokens
including EOS; EOS with zero tokens is invalid. A valid last-token EOS may also
hit the cap. The fake tests' 173-token shape is a test double, not a Qwen token
count or usable calibration observation.

The quote and worker identity remain transport attestations. A request digest
shows which bytes the client passed; it does not authenticate a worker, its
weights/tokenizer, runtime, hardware or reported usage. The bridge does not read
Store evidence, reserve budgets, check durable deadlines or grant send authority.
The complete timed Store/controller integration is a separate milestone.

## One outstanding ticket, repeat only after known completion

Calls from another thread fail before transport; sharing the same worker across
multiple bridge objects is unsupported. The client is explicitly single-thread,
not a concurrent connection pool. Reentrant callbacks cannot start a second
operation while an exchange is in progress.

`prepare` is allowed initially or after a confirmed generation/discard. One client
can therefore run the eight sequential proposed probes on one persistent worker.
Every new preparation gets its own ticket. Old, copied, foreign, mutated, consumed
or discarded tickets cannot authorize generation or discard. Tests also mutate a
ticket through Python's low-level escape hatch and verify its stored snapshot is
still authoritative, including during a callback.

The client marks an operation uncertain **before** invoking transport. The worker
independently consumes a generation handle before its generation callback. A lost
prepare reply, failed generation, worker restart/lost handle, unknown discard
acknowledgment or malformed measured response leaves the client uncertain. It has
no automatic reconnect, reset, retry, reclaim or fallback to legacy direct
requests. The old worker may still hold a preparation after an ambiguous prepare;
that is not permission to guess its handle or send again.

Confirmed discard permits another preparation but does not establish warm model
readiness. A completed generation establishes only what the worker attests; a
future controller must record and validate the actual readiness context.

`BridgeError.reply` preserves returned malformed data for reconciliation/audit.
Unknown transport outcomes have no fabricated measurements. An invalid reply
never turns into zero usage or permission to resend. No Store hold is released by
this helper because it does not own Store accounting.

## Format feedback is not a gold-label oracle

Feedback is allowed only when the incident workload's public protocol validator
rejects the previous output. A well-formed semantically wrong answer remains
protocol-valid and cannot trigger this feedback builder. Gold labels never enter
requests, feedback, preparation, admission or worker generation.

The proposed calibration feedback seed is the exact synthetic invalid object
`{"decision":"route"}`. It lacks required fields. It is a **fixed calibration
probe**, not an observed initial model response or an example selected using gold.
Its exact bytes, feedback instruction and completed request hashes are included
in the plan. Measuring this short form would not cover arbitrary invalid outputs
or longer future feedback requests.

## Emit an unexecuted calibration proposal

```
python scripts/incident_preflight.py --out /tmp/incident-preflight-plan.json
```

The output file must not exist. `--live` and other execution flags are rejected.
This CLI renders JSON only; no tokenizer, model or transport is invoked.
The plan has `execution_authorized=false`, `new_model_calls=0`,
`profile_status=unmeasured`, per-slot `measurement_planned=true` and
`measurement_status=not_executed`. Input token counts are null until actually
prepared. The plan digest covers all fields except itself; a worker source digest
and frozen workload digest identify the local source artifacts, not authenticated
execution provenance.

The staged proposal is:

1. Four initial requests in calibration order on one freshly loaded worker.
2. Four fixed protocol-feedback requests on the same worker.
3. No hidden warmup, retries or held-out cases.

Bounds: at most **8 generation calls**, **2048 exact prepared input tokens** per
call, **96 output tokens** per call, **17,152 total quoted token units**, and
**20 minutes total elapsed time**, including model/tokenizer load, preparation,
queue, generation, validation and audit. These are proposed bounds, not permission
to execute. Missing or exceeded bounds require stopping without automatically
retrying uncertain work. The controller must stop if the preceding generation
has no known completed receipt; uncertainty cannot silently establish warm state.

Only the first initial call is cold. Proposed observations are cold initial n=1,
warm initial n=3, warm fixed-feedback n=4. There is **no cold-feedback coverage**,
no arbitrary-feedback coverage and no broad production latency profile. Such
small groups cannot justify percentile claims or hard completion guarantees.

Before an explicitly approved execution, a separate bounded controller must
capture model/tokenizer revisions, dependency versions, exact chat-template and
tokenizer configuration, worker source identity, full effective generation
configuration including EOS/PAD/defaults, dtype, CPU model/flags, affinity, actual
thread count, clock domain, readiness and exact per-request token counts. The
historical worker chooses up to four threads from available affinity; do not assume
a two-thread profile. Measure load/preparation separately from generation through
validation, while charging all of them to the overall elapsed limit.

An external watchdog is required for an actual all-in deadline: a check before a
native model call cannot preempt a hung generation. Any resulting uncertainty must
retain conservative accounting. No such live controller or execution is enabled
by this preparation milestone, and old report-task calibration is not reused.

### Possible later runner cost, not an execution request

If a separately approved run uses GitHub's standard Linux 2-core x64 runner, the
current [published runner rate](https://docs.github.com/en/billing/reference/actions-runner-pricing)
is $0.006 per minute, with each job rounded up to whole minutes. Twenty billed
minutes would therefore be approximately $0.12 of nominal compute before account
allowances, plus any artifact storage and ordinary CI costs. This is neither an
invoice nor a hard account spending cap, and no live workflow is added here.

A 20-minute all-in job must reserve time for setup and final audit/upload and stop
model activity early enough to finish those steps. Resetting the controller clock
after setup would violate the all-in limit. The actual runner's two-core affinity
and observed thread configuration must enter the new profile; do not reuse a
profile measured on other hardware.
