# KairoTensor: Evidence-Grounded Revision for Agent Memory

Author: Alejandro Castellanos; affiliation: Independent Research

## Abstract

Agents often retain conclusions after the evidence underlying them changes. We study a local evidence store that records the occurrences supplied to an agent computation and checks whether a saved result remains eligible after a withdrawal or correction. KairoTensor uses SQLite transactions to capture evidence views, branch heads, exact dispatch bytes and usage. For deterministic support rules, it also tracks alternative sets of witnesses and atomically revises affected verdicts. In a synthetic, single-writer benchmark, atomic revision reduced median edit time by 13.46 times for 32 dense edits across 128 rules relative to a per-branch implementation; a lighter full-scan baseline was faster end to end in all 30 matched trials. In a single synthetic live-model handoff, reuse of an unchanged report judgment reduced correction-route model usage from 404 to 193 tokens; replacing the report triggered a fresh judgment. These observations are narrow: the task was synthetic, timings were not a latency distribution, and a separate frozen recovery evaluation yielded zero successful held-out cases under its strict contract. The prototype demonstrates a testable mechanism for managing evidence changes and accounting for work, not a general claim that agent decisions become more accurate or trustworthy.

## 1. Introduction

An agent may make a decision from a set of observations and later receive a correction. Saving the answer alone loses a practical question: which evidence did the computation actually receive, and is the result still eligible when a source is withdrawn? Reusing an obsolete continuation can carry a superseded claim into subsequent actions even when the new evidence is available.

KairoTensor investigates a narrower, operational question: can a local store keep a durable record of an agent computation's declared evidence and refuse stale reads or continuations, while permitting explicitly bounded reuse? The prototype provides an occurrence-level evidence ledger, revision-checked branches, and a deterministic alternative-witness mechanism. It does not authenticate evidence or infer what a model internally attended to. The caller must authenticate actors, supply all relevant dependencies, validate outputs, and correctly report usage.

Alejandro Castellanos confirms that no KairoTensor-specific ideas, code or experiments predate the sprint. The repository history begins on 13 November 2026, and the measured revision and model experiments were committed on 13-14 November. This provenance statement is the author's declaration; commit timestamps alone cannot establish when work began. This report covers the measured main-branch system and excludes the unmerged hosted-integration branch.

## 2. Related work

W3C PROV models entities, activities and agents for exchanging provenance [1]. C2PA describes signed claims about media provenance and authenticity [2]. KairoTensor instead focuses on application-level read and continuation eligibility after a local evidence-view change; its SQLite history is not a signed or tamper-evident provenance standard. These approaches are complementary and no compatibility or comparative evaluation is claimed. Its alternative-witness rule resembles dependency tracking: each declared group requires all its evidence, and any complete group suffices. These are availability checks over caller-declared rules, not verification that the evidence supports a real-world claim.

## 3. Methods

### Store and revision

The Rust store gives each observation occurrence an identity and tracks admission for an actor and scope. Beginning an attempt freezes the selected bytes, ordered occurrence IDs, view epoch and expected branch head. Dispatch can record exact adapter request bytes. Settlement checks the captured view and branch before advancing the head and records usage even when a delivery is stale. Current reads and resumes recheck eligibility. A withdrawal cannot revoke bytes already delivered to an external caller.

For deterministic support, each rule contains alternatives of conjunctive evidence groups. SQLite maintains counts for affected clauses and publishes changed verdicts in the same transaction as a membership edit. An unchanged verdict can retain its head. An independent evaluator checks the incremental implementation across all admission subsets of four observations [3]. The implementation assumes the caller's rule and witness assignments are appropriate; an admitted witness is not proof of truth.

### Experiments and baselines

The revision benchmark uses file-backed SQLite, synthetic rules with two three-premise alternatives, and identical ordered edits. It compares per-branch indexed revision, atomic indexed revision with equivalent publication history, and a batched full scan with a lighter history contract. Each timed path is checked against an independent evaluator; separate replays check persisted verdicts after every edit. Five fixed workload cells use six matched repetitions each, with all execution order permutations represented. The full scan is a useful performance baseline but does not persist the same attempt, parent, binding, delivery and usage history [4].

The model reuse example separates a test-report judgment from a deployment planner. A policy correction changes the planner's input but not the report judgment's closed, versioned inputs. Reuse requires the same request bytes, candidate list, contract and relevant admissions; changing the report is a negative control. The live example uses Qwen2.5-1.5B-Instruct, model and tokenizer revision 989aa7980e4cf806f80c7fef2b1adb7bc71aa306, CPU, two threads, PyTorch 2.6.0+cpu and Transformers 4.48.3. Route timings begin after evidence-view preparation and model loading. One revised-prompt run compares repair and fresh rebuild; it is not a randomized population study [5].

## 4. Results

In the atomic revision experiment, 128 rules with 32 dense edits took a median 1,972.5 ms for per-branch edits versus 146.7 ms for atomic edits. The median paired speedup was 13.46 times, range 13.24–13.56 across six trials. For 1,024 rules and 128 sparse edits, it was 1.40 times, range 1.34–1.68. Atomic edits improved edit time in all 30 matched comparisons. The lighter full scan had the lowest total time in every matched trial; for the dense cell its median total was 27.8 ms versus 369.1 ms for atomic indexed revision. The baseline's reduced historical contract prevents attributing the difference to indexing alone [4].

In a later bulk setup experiment, the 1,024-rule fixture's median setup time was 3,690 ms using individual production calls versus 214 ms using bulk calls. The median paired setup ratio was 16.62, range 14.89–34.55, across six trials. The scan retained the lowest median total in all five cells and finished first in 29 of 30 matched trials [4]. These are fixed synthetic, single-writer traces on hosted machines, not a general break-even result.

In the revised-prompt model example, repair reused one report judgment, made one new call and used 145 input plus 48 output tokens. A clean rebuild made two calls and used 326 input plus 78 output tokens. Both produced the same checked canary plan; the observed savings were 211 of 404 tokens. Replacing the report forced a fresh judgment and a hold decision. The earlier prompt had produced an incorrect passing status for an 11/12 report, which the validator rejected; the prompt was revised before the reported successful run. Each route is one observation, not evidence of typical savings or semantic reliability [5].

A separate frozen recovery-policy evaluation illustrates the remaining model risk. Ten synthetic cases generated 20 Qwen calls. All responses included Markdown fences and failed the unchanged strict JSON contract. On the six held-out cases, the stop, affordable-retry and evidence-gated-retry policies each had zero successes. These policy outcomes are paired replays of shared trajectories, not independent policy arms. All retry receipts arrived after the 60-second operation deadline; the deadline was soft and did not interrupt dispatched calls [6].

## 5. Discussion and limitations

The implementation makes evidence dependence explicit at an application boundary and preserves the cost of work that becomes unusable. The measurements show that atomic batching can improve a costly per-branch path without establishing that indexed revision beats a lighter scan. The model example shows that a carefully scoped cached judgment can be reused in one controlled correction, not that agent decisions improve in the wild. The strict recovery failures show how validation, model formatting and time budgets can dominate the usefulness of such a mechanism.

Future work should evaluate real correction streams and user decisions, test concurrency and adversarial callers, compare equally durable baselines, and run preregistered, independently measured model evaluations. The frozen incident-routing workload and bounded controller on main are offline preparations; they do not establish model performance. A reviewer package contains the pinned source snapshot, evidence index and offline verification commands. Repository and workflow links require access while the repository remains private.

## 6. Conclusion

Versioned evidence and conservative checks can prevent this prototype from serving certain stale stored results. A small live-model example demonstrates a possible reuse benefit; benchmark results and failed recovery trials also show the costs and limits. Trustworthy deployment requires authenticated inputs, sound dependency declarations, independent validation, and evidence from representative settings.

## Code and data

Measured main-branch snapshot: 20fab84a6baa525452191ecf80eeee339f0b5ac7, https://github.com/k2jac9/kairotensor. Raw benchmark CSVs, prompts, model traces and evaluation protocols are in docs/benchmarks and docs/evaluations. Historical CI executions use their recorded source commits, not the current main commit. The accompanying reviewer package preserves the source archive and its checksum; exact experimental sources and commands are documented in [4-6].

## References

[1] Moreau, L. and Missier, P. (eds.), 2013. PROV-DM: The PROV Data Model. W3C Recommendation. https://www.w3.org/TR/prov-dm/

[2] Coalition for Content Provenance and Authenticity, 2026. C2PA Technical Specification 2.4. https://spec.c2pa.org/specifications/specifications/2.4/specs/C2PA_Specification.html

[3] KairoTensor, 2026. Selective revision implementation and tests. https://github.com/k2jac9/kairotensor

[4] KairoTensor, 2026. Benchmarking selective revision, raw samples and run identities. https://github.com/k2jac9/kairotensor/blob/main/docs/revision-benchmark.md

[5] KairoTensor, 2026. Scoped model-result reuse, prompts and traces. https://github.com/k2jac9/kairotensor/blob/main/docs/scoped-model-reuse.md

[6] KairoTensor, 2026. Recovery policy v2: measured contract and deadline failures. [Protocol and raw bundle.](https://github.com/k2jac9/kairotensor/blob/main/docs/evaluations/recovery-policy-v2-2026-11-14.md)

## Appendix: Limitations and Dual-Use / Ethical Considerations

An operator could use admission controls to suppress inconvenient evidence, present a selective record as complete, or convey unwarranted authority through a provenance-like interface. The store does not establish source authenticity, prevent tampering by a privileged operator, detect omitted dependencies, guarantee semantic correctness, or erase copies already delivered. Deployment would need access control, independent auditing, transparent inclusion and exclusion policies, and protection for stored sensitive information. These are recommendations, not implemented protections. The reported examples use synthetic reports and deployment data; this statement does not establish the privacy properties of future applications.

## LLM usage statement

AI assistance was used to help develop code and prepare this report. The author is responsible for the claims and final submission. Automated checks recomputed benchmark summaries and reconciled the archived recovery records with their SQLite ledger; these checks establish internal consistency, not independent replication of model execution.
