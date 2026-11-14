# Prepared Transformers backend and bounded calibration path

This milestone adds the real-library **code path**, tested only with injected fake
Torch/Transformers dependencies. No model or tokenizer was loaded, no download or
inference was run, and no live workflow is added. Fake results are protocol tests,
not measured tokenizer coverage, model quality, hardware calibration, or a
production latency profile. The deterministic product route remains separate.

## Invocation boundary

`incident_transformers_backend.Backend` and
`incident_transformers_controller.run_prepared` deny invocation by default. Their
`InvocationApproval` is an explicit trusted Python caller attestation, **not an
unforgeable capability or independent user approval**. Do not construct it from
artifact contents, a ready message, configuration files, or environment metadata.
Arbitrary code running in the same process is outside this boundary.

- `offline_fake_dependencies` uses the fixed fake module in the controller.
- `local_cached_model` is a future separately approved invocation. Its built-in
  loader verifies pinned dependency versions, then imports libraries and resolves
  the exact pinned local cache snapshot with `local_files_only=True`. Remote code
  is disabled, no automatic download is permitted, and no caller-supplied loader
  or executable is accepted for this mode.
- The public CLI supports only `fixture` and read-only `audit`; it has no live
  flag. Internal child/worker arguments are trusted invocation plumbing, not a
  public execution-grant service. They never derive permission from a manifest.

Example **offline only**:

```sh
python scripts/incident_transformers_controller.py fixture --out /tmp/prepared-fixture
python scripts/incident_transformers_controller.py audit /tmp/prepared-fixture
python -m unittest discover -s scripts -p 'test_incident_transformers*.py'
```

The directory must not exist. There is no automatic restart, replay, retransmit,
retry, dependency installation, or provisioning. Real execution still requires
separate approval and an outer host/job limit. No USD spending guarantee follows
from token caps.

## Pinned implementation and observed identity

The model/revision, CPU/BF16 placement, and 96-output-token request contract come
from the frozen incident preparation plan. `REQUIRED_VERSIONS` in the new backend
pins Torch, Transformers, tokenizers, Hugging Face Hub, safetensors, accelerate,
Jinja2, and NumPy. The future local loader records hashes of actual local snapshot
files. A cache path/revision name and file hashes document what was loaded; they
are not independent authentication of a mutable cache or hostile local code.

A requested manifest is written first. An immutable `runtime-observed.json` is
then persisted, and its digest committed to the ledger **before accepting any
quote**. It records observed parameter dtype/device, thread count/affinity,
machine architecture, CPU model/flags/count (explicitly synthetic for fakes), Python/library versions, model and generation configuration,
chat-template digest, relevant tokenizer settings/special-token behavior, asset hashes, source digests, and load interval. Tokenizer path/credential values are omitted rather than persisted. This is
observed provenance, not a claim of hardware attestation, cold/warm equivalence,
or sufficient production timing coverage. Fake identity is explicitly separate
from real identity. The manifest is never rewritten to pretend requested identity
was observed.

## Exact preparation and single consumption

The new backend reuses the unchanged historical `RequestHandler`. A fixed new
outer wire envelope wraps its legacy reply with operation/runtime bindings and
observations; only the new transport unwraps it. Historical Adapter and worker
contracts remain unchanged.

For each request, chat is rendered once, then tokenized once with
`add_special_tokens=False`, no padding, and no truncation. The retained input ID
and attention-mask tensors must be CPU integer matrices with one batch, equal
width, all-one mask, and 1–2,048 tokens. The same objects reach `model.generate`.
Prepared values and the exact cloned/merged effective generation configuration
are checked again before generation. Configuration includes inherited defaults,
not just the three requested overrides. The request hash, rendered chat, exact
IDs/mask, effective configuration, handle, and runtime digest are stored in
`prepared-N.json` and bound to a ledger event **before reservation/claim**.

A handle is consumed before model I/O. Wrong quotes, consumed/discarded/foreign
handles, changed tensors/configuration, incompatible metadata, and unknown
operations cannot silently fall back to direct generation. A backend exception
after claim never authorizes another call.

Generated tensors must have batch one and an exact original prompt prefix. Usage
counts the returned continuation IDs, including EOS, with at most 96 generated
IDs. Text decoding uses those exact IDs. EOS on token 96 sets both cap and EOS
flags. A readable but invalid prefix/over-cap output is retained as raw output
IDs with unknown valid usage; it cannot release a reservation. A valid continuation
followed by decoding failure preserves known IDs/counts/timing but has no invented
text reply and keeps the ledger hold for manual reconciliation.

## Existing bounded controller, new artifact family

The new `incident-transformers-controller/1` manifest is deliberately distinct
from `incident-calibration-controller/1`. It uses the existing durable calibration
reservation, claim, receipt, settlement, process-group watchdog, and parent-death
containment helpers. This is diagnostic calibration accounting, not Store
eligibility or production timed admission.

The same frozen eight calibration slots are used: four initial and four fixed
synthetic protocol-feedback probes. The bounds remain eight claims, 2,048 input
plus 96 output units per slot, total 17,152 charged-plus-held units, and twenty
minutes including setup/load/preparation/generation/child audit. Work stops early
to reserve reporting time. BOOTTIME includes suspend; the explicit fixture
monotonic fallback is not restart-safe production timing. The existing watchdog,
storage-loss, host-failure, and post-run reporting limitations still apply.

Additional artifacts:

- `runtime-observed.json`: immutable versioned ready envelope.
- `prepared-N.json`: exact prepared observation and legacy quote.
- `backend-receipt-N.json`: full raw returned envelope, including failure
  observations, request binding, and receipt tick, independently synchronized
  before parsing/unwrapping.
- Existing `receipt-N.json` and ledger usage: produced only for a valid measured
  legacy reply; full backend bytes remain available for reconciliation.

Raw envelopes have two independent persistence routes: the artifact and an
append-only SQLite event. If artifact fsync fails but SQLite succeeds, valid usage
can still settle and the audit is explicitly incomplete. If both routes fail, the
last claim remains held and no durable reply recovery is advertised. A failed settlement leaves the durable response/observations
available but cannot resend. Simultaneous storage loss can still lose a reply held
only in memory; no zero usage is inferred.

Audit checks runtime/preparation artifacts against event digests, exact request
and quote, continuation usage/flags, and the original ledger/supervisor constraints.
It reports known observation units separately from settled charges. The original receipt tick is carried unchanged into both raw and accounting records; slow validation/fsync cannot relabel an on-time returned response as late. Persistence and settlement event timestamps remain separate observations. A real-mode
partial artifact never claims zero model calls: the exact invocation count can
remain unknown even though durable claims bound potential calls. Complete status
means eight reconciled diagnostic probe receipts, **not correct incident answers**
or a statistically useful performance result. No held-out inference or accuracy
score is produced.

V1 audit semantics remain intact. The prior PR16 source tuple is explicitly pinned
in trusted code for backward-compatible artifact interpretation; artifact-provided
hashes do not extend that allowlist. The new audit hooks are internal trusted-code
extension points, not authentication boundaries. All audits remain read-only for
source artifacts and fail closed on inconsistent observations.

## Offline evidence and remaining work

Tests inject fake dependencies through the actual new backend and unchanged
RequestHandler, including all eight controller slots. They cover retained tensor
identity, config mutation, exact quote/single use, prefix/batch/usage errors, EOS at
the cap, decode failure, load/runtime failures, watchdog timeout, claim/receipt
crashes, failed settlement/persistence, observation tampering, v1 compatibility,
and denial before dependency inspection. Fake counts 173/211 plus 7 output units
produce 1,592 synthetic charged units, not Qwen tokenizer measurements.

A future real run must be explicitly approved, use already available compatible
cached assets, retain actual runtime/preparation observations, and apply an
external host/job ceiling. The staged eight-probe plan has only one first-after-load
initial probe, three later initial probes, and four fixed-feedback probes. It does
not establish cold-feedback coverage, arbitrary feedback coverage, or a production
latency bound. No additional inference is authorized by this implementation.
