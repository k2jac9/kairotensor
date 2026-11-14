# Kairotensor

Kairotensor is a Rust framework for versioned evidence memory. Its first component is a local SQLite store that records which observations an agent used and checks whether a saved result is still eligible to read or resume after evidence changes.

## Run the withdrawal example

With stable Rust, Cargo, and a C compiler for bundled SQLite, run these commands from the repository root.

```sh
cargo run --example withdrawal
cargo run --example selective_revision
cargo run --example corrected_handoff
cargo run --example scoped_model_reuse
cargo test --all-targets
```

The example counts 8 items on one shelf and 4 on another. It starts a second count, then withdraws the second shelf report. The previous total of 12 becomes unavailable for both current reads and resume. The late count still costs two additions. A fresh attempt over the same candidate list sees only the first shelf and publishes 8. The ledger records five additions across all three attempts.

## Bulk setup

The Rust store provides three ordered batch operations. Each call commits in one SQLite transaction. An error in any item rolls back every write in that call, including index updates and usage records.

| Operation | Use it when |
| --- | --- |
| `observe_and_admit_many(actor, scope, observations)` | Import new source/payload pairs and grant access to their occurrences. The returned IDs follow input order. Repeating the import creates new occurrences, even for identical bytes. |
| `admit_many(actor, scope, evidence_ids)` | Grant access to observations already in storage. Each effective grant advances the view epoch once. Repeated IDs and already-admitted IDs add no events. |
| `publish_support_many(actor, scope, rules)` | Register support rules and publish their initial verdicts together. Every declaration has its own accepted attempt, parent and charge. Repeating a branch name creates successive versions in input order. |

Evidence ingestion and rule publication are separate commit boundaries. A caller can reuse IDs from a successful import if rule registration fails. For an existing catalogue, bulk grants update witness counts immediately. Use `revise_support` for an affected head whose verdict needs a new publication. The selective revision example exercises bulk setup followed by an atomic withdrawal.

## Selective revision

The second example asks whether a test run has a reproduction record. An original report **and** its input manifest qualify together. A self-contained record from a second run also qualifies on its own. Withdrawing the original report preserves the accepted reproduction result through the second run. In the same transaction, the separate check that requires the original report publishes `unsupported`.

| Operation | Contract |
| --- | --- |
| `publish_support(actor, scope, branch, alternatives)` | Publish a fixed rule over observation IDs. Every ID in one group must be admitted. Any complete group suffices. |
| `set_admitted_and_revise(actor, scope, evidence, admitted)` | Apply one membership edit and publish changed support verdicts in one transaction. Return the changed branch names, accepted attempt IDs, and verdicts. |
| `support_consumers(actor, scope, evidence)` | List current support-rule branches that name this occurrence in this actor's scope. A listed branch may retain another witness. |
| `revise_support(actor, scope, branch)` | Return the same accepted attempt ID if its verdict still holds. Publish a new attempt if the verdict changed. |

The built-in rule checks **availability of declared witnesses**. The caller defines which observations qualify for the question. `unsupported` means every declared alternative lacks at least one admitted observation. Missing and withheld IDs have the same effect. The store never needs the payload bytes to answer this rule.

SQLite updates the affected clause counts in the same transaction as admission or withdrawal. Reads and resumes check the verdict against the current view. In-flight opaque work still goes through the original epoch check. Opening a v1 through v8 database migrates it to schema v9. Existing heads and usage records retain their identities.

Use `set_admitted_and_revise` when readers should see the membership update and all affected support heads in the same database snapshot. Any failure rolls back the entire edit. A witness that keeps the same verdict retains its head and cost. Repeating the desired membership while it is already current returns an empty change list. An intervening reversal makes a subsequent call a new edit. The original `set_admitted` and `revise_support` remain available for deferred revision.

The store charges one unit per premise checked during rule registration and one for a changed verdict published by revision. Reusing a head adds no delivery or charge. These logical units exclude index maintenance and read checks. The integration suite compares the incremental answers with fresh databases and an independent evaluator across every admission subset of four observations.

## Corrected handoff

The deployment planner example captures exact dispatch bytes and their source IDs, retires an accepted staging continuation, then asks for a canary plan. A clean rebuild provides a second answer against the same release checklist. Run with the deterministic adapter or the pinned local CPU model. [Example, commands and evaluation](docs/corrected-handoff.md)

## Reusing a model result

A caller may reuse a previously validated model response for a closed, versioned computation on a branch. For example, a deployment policy can change while an independent test-report judgment remains eligible. In a live Qwen fixture, repairing the plan used one call and 193 tokens; a clean rebuild used two calls and 404 tokens. Replacing the report forced a fresh judgment and the release was held. [Contract and live evaluation](docs/scoped-model-reuse.md)

The [live evaluation prompt catalogue](docs/evaluation-prompts.md) includes the original and revised instructions, exact user payloads, checkpoint history, and the request used at each step.

## Token preflight and budgets

The local Qwen worker can tokenize a frozen chat before generation and return its exact prompt count. Kairotensor can reserve that count plus the enforced output cap in a token budget. An affordable request uses the prepared inputs; an unaffordable request can discard them without running the model. On delivery, the ledger charges actual prompt and completion tokens and releases the unused reservation. Unresolved dispatched work retains its reservation.

The `token_cap_eval` example compares four output caps on synthetic reports and replays the archived 403/404-token boundary. [Experiment protocol](docs/token-cap-protocol.md) · [Results and reproduction commands](docs/token-cap-results.md)

## Evidence-backed deterministic verdict

The [deterministic verdict route](docs/deterministic-verdict.md) derives a complete
CI verdict from one admitted report without a model call. It refuses incomplete
candidate manifests and checks evidence freshness again at publication. Historical
commit receipts are separate from current read eligibility. Run
`cargo run --example deterministic_verdict` for the zero-model-call demonstration.

## Frozen semantic workload (offline)

The [incident-routing smoke workload](docs/incident-routing-workload.md) prepares
versioned evidence-grounded routing requests and audits recorded responses against
a strict frozen contract. Its 12 authored cases separate calibration from heldout
scoring. Preparation performs no inference; synthetic fixture scores are not model
performance measurements, and declared evidence eligibility is not Store authentication.
The [prepared-request bridge](docs/incident-prepared-bridge.md) connects these
prompts to the existing worker protocol with offline lifecycle tests and emits a
bounded, unexecuted calibration proposal. It does not grant Store send authority.
The [offline calibration controller](docs/incident-calibration-controller.md)
tests durable send claims, retained uncertain usage and independent process
termination against fixed fake workers. It has no live model launch path.
The [versioned Transformers backend](docs/incident-transformers-backend.md)
connects the bounded controller to pinned local-model APIs, with execution denied
by default. Its automated checks use fake dependencies only; real calibration
still requires provisioned assets and separately approved execution.
The [proposed GitHub/NixOS comparison](docs/calibration-host-matrix.md) lists
matching requirements, readiness gates and unapproved per-host bounds.
The [offline Adaption adapter](docs/incident-adaption-backend.md) exercises a
distinct hosted-response contract and synthetic accounting, without credentials,
network requests, or live execution authority.
The [one-request hosted runner](docs/incident-hosted-runner.md) adds fixed-origin
HTTPS code and a bounded child process, tested only with mocked HTTP and canned
receipts. Live use requires separate approval and a verified billing bound.

## Implemented contracts

The [durable recovery APIs and example](docs/budgeted-recovery.md) connect
validation failure to one retry or abstention. Recovery IDs, retry slots,
reservations and single-use send claims survive restart and serialize competing
workers. Only proven-unsent work can be cancelled. Run
`cargo run --example budgeted_recovery` for the deterministic fixture, or add
`--live` for the pinned Qwen worker. The fixture is not a live performance result.

The opt-in [deadline-aware recovery gate](docs/deadline-aware-recovery.md) binds
the original clock domain and deadline to a runtime/calibration profile, checks
remaining time after acquiring the writer lock, and refuses uncovered requests.
Its offline example uses synthetic timing, not measured inference performance:
`cargo run --example deadline_aware_recovery`. The [runtime send boundary](docs/recovery-runtime.md)
adds a concrete Linux boot clock and a final post-claim check before one backend
invocation, without treating uncertain sends as free or retryable. The
[offline end-to-end runner](docs/timed-recovery-runner.md) joins this path to the
existing fixture Adapter with durable audit files and fault scenarios; live mode
is explicitly refused.

The [recovery-policy evaluation protocol](docs/recovery-policy-protocol.md) defines
an offline paired comparison with explicit accounting and deadline checks. Run
`python3 scripts/recovery_policy_eval.py --fixture` for synthetic contract cases,
not model-performance measurements. The [Store-backed collector](docs/recovery-policy-runner.md)
adds a versioned corpus, raw traces and a durable ledger; live collection is opt-in.

| Operation | Behavior |
| --- | --- |
| `observe`, `set_admitted` | Record a distinct observation occurrence and change visibility for one actor and scope. Repeating an admission leaves the view epoch unchanged. |
| `begin_from_candidates` | Select admitted inputs from an ordered list and freeze their bytes, the view epoch and the expected branch head in one transaction. This also covers a selection that returns zero inputs. |
| `begin_attempt` | Require every explicit input to be admitted. Record the ordered IDs and start fresh, with no inherited output. |
| `read_current`, `begin_resume` | Check the accepted head against the current view before returning bytes. A resume records which accepted output it inherited. |
| `record_dispatch` | Freeze the exact serialized adapter request for a still-eligible pending attempt. The request stays linked to the ordered source occurrences. |
| `create_token_budget`, `token_budget_state` | Set a ceiling in input-plus-output tokens and read the amount spent and still reserved. |
| `reserve_dispatch` | Atomically reserve the measured input plus the configured output cap and authorize dispatch. A denial records the decision and consumes no capacity. |
| `create_recovery`, `begin_recovery_attempt` | Bind a stable operation key and allocate at most the original attempt plus one retry after invalid output. |
| `reserve_recovery_attempt`, `recovery_reservation` | Stage exact request/quote and read an unsent reservation for verified re-preparation. |
| `claim_recovery_dispatch`, `cancel_recovery_attempt` | Serialize send permission against cancellation; only one claimant can send, and only unclaimed work releases capacity. |
| `settle` | Validate the captured branch and view, record the delivery and usage, and advance the head only on acceptance. Identical retries return the first disposition. Conflicting retries fail. |
| `reject_invalid` | Record a dispatched response that failed validation, including the versioned validator, reason, exact returned bytes, and final usage. Keep the current branch head. |

The caller authenticates actors for every operation. For opaque computations the caller also validates outputs and measures usage. Settlement distinguishes acceptance, stale views, stale branch heads and validator rejection. A rejected answer also records whether its captured view and branch were current at settlement. Each branch belongs to one actor and scope. An effective admission or revocation in that scope conservatively invalidates an opaque head. A fresh computation can reuse raw evidence that is still admitted. Withdrawal governs future reads from this store. Previously delivered bytes may still exist in a caller.

## Reference and next milestone

[The v0 design](docs/memory-v0.md) describes the larger research program. The Python reference under `reference/` covers additional proposed behavior, including alternative derivations and exact categorical belief revision. Its original fixtures run with the Python standard library.

```sh
python3 -m unittest discover -s reference -p test_memory.py
python3 reference/belief_oracle.py
```

On the three-way benchmark, atomic revision reduced median edit time by 13.46× for 32 dense edits on 128 rules, compared with the per-branch API. For 128 sparse edits on 1,024 rules, the corresponding gain was 1.40×. The batched full scan still had the lowest total time in all 30 matched trials. On a subsequent matched run with 1,024 rules, bulk setup took a median 214 ms versus 3,690 ms for single-item setup. Both production paths retained per-occurrence admission events, branch lineage and logical charges. [Benchmark, measurements, and reproduction commands](docs/revision-benchmark.md)
