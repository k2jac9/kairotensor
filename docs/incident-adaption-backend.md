# Offline Adaption hosted-response adapter

This milestone implements a **hosted request/receipt contract and synthetic
accounting tests**, not a live Adaption integration. There is no HTTP client,
credential lookup, account query, model call, or paid operation. Invocation is
denied unless the caller supplies the explicit offline marker and the fixed
fixture transport. `live_admissible` remains false.
`OfflineApproval` is a fake-only caller marker, not user permission or a security
capability.

The subsequent [one-request hosted runner](incident-hosted-runner.md) supplies
separate HTTPS and process-control code, tested offline only. The contract and
synthetic ledger described here remain unchanged; they cannot launch that runner
or supply live authority.

This is separate from the [local Transformers backend](incident-transformers-backend.md).
Freezing an HTTP request body does not retain the provider's tokenized inputs,
measure its exact input count, pin a model deployment revision, or enforce its
billing. The local worker's token reservation and hardware profile cannot be
reused as hosted guarantees.

## Offline commands and scope

From the repository root:

```sh
python scripts/incident_hosted_controller.py fixture --out /tmp/incident-hosted-fixture
python scripts/incident_hosted_controller.py audit /tmp/incident-hosted-fixture
python -m unittest discover -s scripts -p test_incident_adaption_backend.py
python -m unittest discover -s scripts -p test_incident_hosted_controller.py
```

The destination must not exist. The fixture command currently runs **one canned
`cal-01` response** as a contract smoke test. Its placeholder model ID is not an
available or selected Adaption model. It performs zero model calls; synthetic
usage and estimated charges are not measurements of provider performance or cost.
There is no live flag or arbitrary transport option. A ledger permits at most
four claimed initial requests; the smoke command uses only one.

The renderer supports the four frozen calibration cases in
[incident-routing/1](incident-routing-workload.md). The proposed hosted comparison
is eight probes: four initial requests, then four fixed protocol-feedback
requests. That comparison is not implemented by this one-case smoke command or
approved for execution. Held-out evaluation is separate future work; no held-out
results, accuracy claims, or production latency profile are produced here.

## Request and response contract

`incident_adaption_backend.render_case` emits canonical UTF-8 request bytes,
their hash, and workload/evidence provenance for `POST /api/v1/responses`. It
requires an explicit model ID, refuses stack IDs, and renders only the frozen
prompt and selected calibration evidence. Gold labels and case-routing metadata
do not enter the HTTP body. Missing, stale, or refused evidence causes refusal,
not silent filtering. These eligibility declarations remain fixture/replay
attestations, not authenticated Store admission.

Requests set `max_output_tokens=96`, `temperature=0`, `stream=false`,
`background=false`, `tools=[]`, `tool_choice="none"`, `agentic_memory=false`, and
`store=false`, with no prior-response linkage. These are requested controls, not
proof of provider behavior. The public [Responses creation reference](https://docs.adaptionlabs.ai/api/resources/responses/methods/create)
documents the endpoint and fields; this adapter does not assume OpenAI-compatible
features beyond its reviewed subset.

`parse_response` keeps response attribution, completion state, output protocol
validity, and reported usage separate. It distinguishes refusals, errors,
unfinished output, unexpected tools, malformed envelopes, and invalid model JSON.
Assistant text parts must agree with the aggregate text; reasoning is not an
answer. The unchanged incident validator checks format and known IDs, **not
semantic correctness against gold**. A well-formed wrong answer can be
protocol-valid, and an invalid answer can still have accountable usage.

Usage counts must be nonnegative integers with a consistent total. Missing,
partial, contradictory, or over-cap counts remain visible and are never repaired
or replaced with zero. Cached/reasoning detail is retained separately rather than
added to totals. Provider-reported counts are not independently measured tokens
or a verified invoice.

## Durable fixture accounting

`incident_hosted_controller.HostedLedger` provides operation creation,
reservation, claim, dispatch, receipt recording, reconciliation, cancellation,
and read-only audit. Operation keys bind immutable request bytes, provenance,
model, and quote. A SQLite transaction commits the single-use claim **before**
the fixture transport is invoked. Duplicate dispatches cannot resend that claim,
including after reopening the ledger.

`OfflinePriceBound` uses integer **synthetic micro-USD** rates, fees, and maximum
holds. These are test inputs, not Adaption prices or an enforceable spending
limit. Bound, terminal receipts with complete valid usage can settle a synthetic
estimate once, including invalid answers or late receipts. Each hold must cover
the quote's worst-case synthetic cost. Missing or uncertain
usage retains the full hold and prevents further dispatch while unresolved.
Usage above the synthetic bound is preserved and flagged, not clipped or charged
as zero. A missing receipt clock is recorded as unknown, not a fabricated time.
Only planned or reserved, unclaimed work can be cancelled locally. Cancellation
of that ledger state is not cancellation of remote work.

Artifacts are `manifest.json`, `ledger.sqlite`, and `receipt-<operation-id>.json`.
Receipts preserve bounded raw body bytes, HTTP status, request binding, receipt
tick, and allowlisted response headers. The receipt file and SQLite are separate
persistence routes before parsing/reconciliation. If both fail, a response held
only in memory may be lost; the last claim remains uncertain. No credential
configuration or authorization headers are implemented. Audits rebuild frozen
requests, check source/manifest bindings, and recompute parsing and estimates
from durable receipts. They report estimated spending, held bounds, parsed usage,
and issues; `observed_provider_cost` remains null. These are internal-consistency
checks, not authenticated provider provenance or tamper-proof storage. An audit
result is not a live run result, a correctness score, or a bill.
`audit_complete` means the consistency checks succeeded; operations may still
have unresolved holds.

The fixture clock checks admission against a stored window. This milestone does
not provide a remote cancellation mechanism or a live hard-deadline controller.
A client timeout, disconnect, or local process kill would not prove that a
provider stopped computing or charging. The documented
[response deletion endpoint](https://docs.adaptionlabs.ai/api/resources/responses/methods/delete)
deletes stored responses and does not cancel in-progress work. No automatic
retries, redirects, retrieval, or retransmission are implemented.

## Blockers before any live run

- Verify account access to a selected hosted model through the account's model
  catalogue. Public examples are not proof of availability to this account.
  [Models list reference](https://docs.adaptionlabs.ai/api/resources/models/methods/list)
- Verify credits, applicable pricing, credit-to-currency conversion, and all
  billable input/output/reasoning categories. The public catalogue lists credit
  rates, but no account balance, effective price, or safe spending ceiling has
  been established here. [Hosted model catalogue](https://docs.adaptionlabs.ai/inference-stacks/models/)
- Establish a defensible pre-send input and reasoning/output billing bound. The
  reviewed API reference does not establish an exact input-count preflight,
  idempotent-send contract, or provider-enforced strict JSON-schema output for
  this adapter. Request hashes and `max_output_tokens` alone do not fill those
  gaps.
- Obtain explicit approval for the selected service/model, transmitted data,
  call count, total currency limit, and timeout/uncertainty handling. Supply
  credentials through an approved secure mechanism only after that decision.
- Review the separate real transport and bounded controller for the approved run.
  Freeze the final source/request configuration, preserve partial receipts, and
  never treat a local timeout as permission to release a hold or resend.

Current tests exercise the offline contract. They do not establish live API
compatibility, service availability, remote completion guarantees, or model
efficacy. Prior frozen workloads, local workers, and archived results are unchanged.
