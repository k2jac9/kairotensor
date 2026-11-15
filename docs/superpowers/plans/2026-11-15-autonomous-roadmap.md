# KairoTensor autonomous execution map

Authority: on 15 November 2026 the owner authorized autonomous implementation,
research, documented decisions and continued execution. The owner explicitly
excluded any Rocket connection. Existing available credits may be used for
bounded, useful experiments; this is not authority for top-ups, subscription
changes or spending beyond verified existing balances.

This map supersedes the earlier review-only execution restriction. Historical
reviews and receipts remain unchanged. Checkboxes in the component plans describe
future work; completion is recorded with commits and evidence in the ledger.

## Execution order

| Stage | Executable plan | Dependency / deliverable |
| --- | --- | --- |
| A | [P0](2026-11-15-evidence-reliability-p0.md) | Qualified lock, shared eligibility explanation, atomic batch correction. |
| B | [Incident bridge](2026-11-15-incident-store-bridge.md) | After A; captured context, exact fixture requests, existing timed execution gates. |
| C | [Decision replay](2026-11-15-decision-replay.md) | After B; authorized export, honest missing-data outcomes, pure replay. |
| D | [Policy activation](2026-11-15-policy-activation.md) | After A-C; comparable offline evaluation and transactional activation. |
| E1 | [Offline diagnostics](2026-11-15-offline-diagnostics.md) | After C; bounded plans and recorded-outcome scoring. |
| E2 | Same diagnostics plan, conditional task | Portable interchange only when a real second consumer requires it. No Rocket integration. |
| F | Bounded experiments | After relevant offline gates; separate run manifests and exact credit bounds. |
| G | [Runtime qualification](2026-11-15-runtime-qualification.md) | After integrated core; qualify isolated current/preview/previous lanes and exact project pins. |

Read the [continuation specification](../specs/2026-11-15-reliability-continuation.md)
with each component plan. It resolves details absent from the original proposal.
Architecture stays Rust/SQLite first; Python remains an offline fixture/scoring
layer. No new service, distributed authority or programmable policy language.

## Autonomous operating contract

1. Reconcile source and literal allowed files before each task. Use one code
   implementer at a time and an independent reviewer at each completed task.
   The coordinator may write planning documents concurrently in disjoint files.
2. Record BASE/HEAD, task identity, reviewed spec and allowed files before work.
   Preserve a failing test, implement, run relevant checks, review, fix and commit.
   Never alter a worker's grader/permissions to hide a failure. The coordinator
   documents any necessary successor scope before dispatching it.
3. Resolve routine ambiguity from source, primary research and this specification.
   Record `Ruling: decision — evidence/reason — consequence if wrong` in the
   ledger. Continue without routine owner check-ins.
4. Reuse existing storage admission. The project exception expires on
   17 November 2026 at 18:04:38 UTC. Its expiry is not permission to extend it,
   discard evidence or release another workload's reservations. Refused heavy
   work leaves read-only review and lightweight planning available.
5. At every integration boundary run the locked all-target Rust suite and the
   existing Python suites. Run fixture examples with explicit artifact output
   paths. Repeat broad checks only after changes or unresolved concerns.
6. Retain failed tests, mutations, evaluations, source/runtime/dependency
   identities and cost receipts in `kairotensor-artifacts`. Never overwrite an
   archived research result with a newer one.
7. Finish local implementation, commits and review autonomously. No shared-branch
   push, PR merge, deployment, publication or physical disk modification is implied.
   Defer those external actions while continuing authorized local work.

## Experiment discipline

Before spending, inspect supported account/model/balance tools and prior-run
receipts. An absent local receipt does not establish that another runner did
not send. A connected training tool does not prove hosted inference access.
Freeze a hypothesis, source, request set, model/runtime, validators, scorer,
split, deadline, call cap, measured balance and all-category cost ceiling.
Keep reserves for ambiguous sends; do not retry an unknown operation blindly.

Choose experiments by expected information value, not by the amount of credit
available. Initial priorities: a bounded hosted connectivity/accounting smoke,
then a paired Store-backed semantic fixture using a fresh final holdout if
earlier data influenced design. Do not silently compare hosted timings with
local Qwen hardware or convert fixture byte counts into model-token savings.
If access, complete billing bounds or prior-send state cannot be established,
record the limitation and advance the other implementation tasks.

## Completion means

A-E1 have passing source-bound tests, independent review dispositions and
documentation; runtime-policy gaps and experimental unknowns remain explicit.
E2 is deferred until its actual-consumer condition is met. No fake consumer is
invented to make a checklist complete. F reports measured results, including
negative/inconclusive outcomes; available credit is not a scientific result.
G closes runtime gaps that can be qualified locally and retains clear unavailable
or failed lanes; configured hosted CI is not a claim that hosted CI ran.
