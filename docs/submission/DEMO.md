# Optional four-minute presentation

This is a recording script. No presentation recording or live demonstration is
claimed. It can be recorded from the report and archived evidence without model
downloads or new inference.

## 0:00-0:40 - The problem

Show the report title and author.

"An agent reaches a conclusion from several observations. One observation is
later withdrawn. Can the application identify which saved results may still be
used? KairoTensor records the declared evidence for a computation and checks
whether its stored result remains eligible after a correction. It checks
eligibility, not truth."

## 0:40-1:25 - The mechanism

Show the README withdrawal example and selective-revision contract.

"Dependency tracking has a long history in truth-maintenance systems. Here it is
integrated with durable records at the model execution boundary. Each evidence
occurrence has an identity. A computation captures its selected
inputs and current branch. Settlement and later reads check that this context is
still applicable. Deterministic rules can name alternative groups of witnesses:
withdrawing one report need not discard a conclusion if another declared witness
group remains complete. Usage stays recorded even when work becomes unusable."

## 1:25-2:15 - What was measured

Show the atomic benchmark table, including the scan baseline.

"In one dense synthetic workload, atomic edits were 13.46 times faster than
per-branch edits by the median paired ratio. The lighter full scan was faster end
to end in every matched trial. The comparison supports batching an expensive
path; it does not establish that indexing beats a scan with equivalent history."

## 2:15-3:05 - The real model example

Show `docs/scoped-model-reuse.md` and its four-row result table.

"A real Qwen run reused a report judgment when only deployment policy changed.
Repair used one new call and 193 tokens; a clean rebuild used two calls and 404
tokens. Both returned the same checked plan. Replacing the report forced a new
judgment and a hold decision. This is one synthetic example, not an estimate of
typical savings. The earlier prompt failed its negative control and is retained."

## 3:05-3:45 - The failure matters

Show the recovery-v2 held-out results and audit receipt.

"A separate recovery evaluation made 20 actual Qwen calls. All responses violated
the strict JSON contract, and all three replayed policies had zero held-out
successes. Every retry missed the soft deadline. The ledger retained 5,241 tokens
of usage. A successful collection or audit is not a successful model evaluation."

## 3:45-4:00 - What comes next

"The next tests need representative correction streams, equally durable
baselines, concurrency and independent model replication. The reviewer package
contains the frozen evidence, reproduction commands and the limitations."
