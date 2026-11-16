# A field guide to KairoTensor

[Project overview](../README.md) · [Paper](submission/kairotensor-report.pdf) · [Evidence index](submission/EVIDENCE.md)

The project studies a practical epistemic boundary: when a stored conclusion may still be used after its declared evidence changes. Read the paper for the measured research question; use the contract pages to understand the current implementation.

## Follow the evidence

| Stage | Question | Read |
| --- | --- | --- |
| **01 · Capture** | Which occurrences and exact bytes did this attempt receive? | [Store and execution guide](store-guide.md) · [Corrected handoff](corrected-handoff.md) |
| **02 · Qualify** | Is the head eligible now, and why? | [Current eligibility](current-eligibility.md) · [Atomic correction](atomic-correction.md) |
| **03 · Execute** | Can this request be sent within the declared resource and timing contract? | [Token preflight](token-cap-protocol.md) · [Budgeted recovery](budgeted-recovery.md) · [Runtime boundary](recovery-runtime.md) |
| **04 · Reuse or replay** | Can a prior result be reused, or a recorded request reconstructed? | [Scoped reuse](scoped-model-reuse.md) · [Decision replay](decision-replay.md) |
| **05 · Evaluate** | What changed in the decision, cost and failure behavior? | [Revision benchmark](revision-benchmark.md) · [Controlled correction pilot](controlled-correction-results.md) · [Policy promotion](policy-promotion.md) |

## Three kinds of evidence

**Measured research.** The sprint paper freezes source `20fab84`. Its [evidence index](submission/EVIDENCE.md) maps every principal result to samples, prompts or archived responses. The [later correction pilot](controlled-correction-pilot.md) has a separate source/model identity and is not silently added to the paper.

**Implemented contracts.** Rust tests and offline fixtures cover eligibility, atomic correction, the [Store-backed incident bridge](incident-store-bridge.md), replay and policy activation. A passing contract test establishes the tested behavior, not semantic model accuracy.

**Prepared experiments.** The [incident workload](incident-routing-workload.md), [Transformers backend](incident-transformers-backend.md), [Adaption contract](incident-adaption-backend.md) and [hosted runner](incident-hosted-runner.md) specify execution boundaries. Their offline tests do not establish live provider compatibility, billing reconciliation or comparative performance. Read each page's execution limits before running it.

## For reviewers

1. Read the [submission guide](submission/README.md) and the paper's contribution and limitations sections.
2. Follow a claim through the [evidence index](submission/EVIDENCE.md) to the pinned raw artifact.
3. Recompute archived summaries with the documented commands. Keep this distinct from re-executing or independently authenticating a model run.
4. Review [later reliability changes](reliability-review-guide.md) separately from the measured snapshot.

## For implementers

The [Store guide](store-guide.md) retains the detailed API and accounting contracts. [Deadline-aware recovery](deadline-aware-recovery.md), the [timed runner](timed-recovery-runner.md), [decision bundles](decision-replay.md), [policy transitions](policy-promotion.md) and [offline ablations](decision-ablation.md) describe distinct boundaries. None replaces current admission or grants an independent send permission.

The [original v0 design](memory-v0.md) records research direction. Its historical implementation-status passages are not an inventory of today's code. The Python reference covers additional proposed semantics; do not assume every reference behavior exists in the Rust Store.

For this workstation, consult the [runtime launcher](runtime-adoption.md) and [runtime qualification](runtime-adoption.md). For hosted checks, use the exact commit's [Actions results](https://github.com/k2jac9/kairotensor/actions), including integration stages and any reported skips.
