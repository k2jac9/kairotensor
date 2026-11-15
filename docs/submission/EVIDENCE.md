# Evidence index

Paths below are relative to the measured source root. The accompanying source
archive is an exact snapshot of `20fab84a6baa525452191ecf80eeee339f0b5ac7`.
CI collection success and model-task success are separate outcomes.

| Report claim | Raw evidence and protocol | Result and limit |
| --- | --- | --- |
| Atomic revision reduces the expensive per-branch edit path | `docs/benchmarks/atomic-revision-2026-11-13.csv`; `docs/revision-benchmark.md` | 90 rows, 30 matched triplets. Dense 128-rule/32-edit cell: 1,972.508 vs 146.650 ms median edit time; median paired ratio 13.460, range 13.244-13.563, six trials. Full scan wins total time in all 30 trials but preserves less history. |
| Bulk setup reduces setup cost | `docs/benchmarks/bulk-setup-2026-11-13.csv`; `docs/revision-benchmark.md` | 1,024-rule fixture: 3,690.273 vs 214.389 ms median setup; paired ratio 16.617, range 14.886-34.549. Scan wins median total in all five cells and 29/30 individual trials. Single-writer synthetic traces. |
| Scoped reuse saves calls in one controlled correction | `docs/evaluations/scoped-reuse-qwen-2026-11-13.json`; `docs/scoped-model-reuse.md`; `docs/evaluation-prompts.md` | Repair: one call, 193 tokens. Fresh rebuild: two calls, 404 tokens. Identical checked canary plans. Report replacement forces new judgment and hold. Seven actual generations across all four routes; one observation per route. |
| Earlier scoped-reuse prompt failed | `docs/evaluations/scoped-reuse-first-qwen-2026-11-13.json`; `docs/scoped-model-reuse.md` | Model labeled an 11/12 report as passing; validator rejected it. The historical attempt remained unsettled. A later zero-call replay records rejection; it does not rewrite the original trace. |
| Frozen recovery did not succeed | `docs/evaluations/recovery-policy-v2-2026-11-14.zip`; matching `.md` | Ten synthetic cases, 20 recorded Qwen calls, 5,241 charged tokens. All replies fenced and invalid under strict JSON contract. Each policy scores 0/6 held-out successes; policies replay shared trajectories. All retry receipts miss the soft deadline. |
| Current main has automated contract coverage | `.github/workflows/check.yml`; `reference/test_memory.py`; `reference/belief_oracle.py`; `scripts/test_*.py`; Rust integration tests | Historical exact-main CI plus local Python receipts. New incident calibration is unexecuted; fixture tests are not live-model evidence. |

## Source and execution identities

| Evidence | Historical source / execution |
| --- | --- |
| Atomic revision | `5d699c44ebfec97bae016a2f8965cc1d9765ffa3`; [run 43476626433](https://github.com/k2jac9/kairotensor/actions/runs/43476626433) |
| Bulk setup | `10e3a57475246f77c8eaa5a05469f1fe79361cfb`; [run 43478880682](https://github.com/k2jac9/kairotensor/actions/runs/43478880682) |
| Revised scoped reuse | [run 43491028189](https://github.com/k2jac9/kairotensor/actions/runs/43491028189); exact requests and lineage retained in the JSON trace |
| Failed initial reuse | [run 43490325882](https://github.com/k2jac9/kairotensor/actions/runs/43490325882) |
| Recovery v2 | `11234573896b68699efa00df63e88553f8d8e99a`; [run 43571699931](https://github.com/k2jac9/kairotensor/actions/runs/43571699931) |
| Main contract CI | `20fab84a6baa525452191ecf80eeee339f0b5ac7`; [run 43622988515](https://github.com/k2jac9/kairotensor/actions/runs/43622988515) |

Both cited live-model experiments use Qwen/Qwen2.5-1.5B-Instruct, model and
tokenizer revision `989aa7980e4cf806f80c7fef2b1adb7bc71aa306`, CPU with two
threads, PyTorch `2.6.0+cpu` and Transformers `4.48.3`. Consult each execution's
manifest for dtype and hardware. Changed hosts and prompts prevent treating
different experiments as a controlled latency comparison.

The recovery ZIP SHA-256 is
`74e8895335f304be0db1d8c705b1e448cb0588dd1b1ccff7296e4a2ff6f8af0d`.
It contains raw responses, exact requests, environment records, policy summaries
and a SQLite ledger. The audit checks consistency between them; it does not prove
that the producer's hardware, model or timestamp attestations are authentic.

The claimed evidence boundary is application-declared dependence and eligibility.
It is not proof of truth, complete causal attribution, source authenticity or
tamper resistance. All reported tasks and workloads are synthetic.
