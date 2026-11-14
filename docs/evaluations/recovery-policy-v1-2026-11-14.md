# Recovery policy v1 pilot: measured failure, not a policy win

Frozen source: `d72dcab9b12f431de309ab00487eb006ac2acddc`.
[Run 43568726459](https://github.com/k2jac9/kairotensor/actions/runs/43568726459)
completed on 2026-11-14. The
[raw artifact](https://github.com/k2jac9/kairotensor/actions/runs/43568726459/artifacts/14102554902)
is retained for 30 days. Its ZIP SHA256 is
`067b7b00a8ecf725b11e900eb62ea81ec72c63616e994b3e62ddae4eafec57fa`.

The collector made 20 live Qwen calls, charged 2,978 tokens (2,297 input,
681 output), and left zero outstanding holds. All replies failed the unchanged
strict JSON/reference contract. All six held-out cases failed under all three
policies. Policy figures are paired replays of shared measured trajectories,
not separately executed policy arms.

| Held-out policy | Calls | Charged tokens | Successful cases |
| --- | ---: | ---: | ---: |
| always_stop | 6 | 791 | 0/6 |
| affordable_retry | 12 | 1,829 | 0/6 |
| evidence_gated_retry | 10 | 1,465 | 0/6 |

The pilot cannot establish a recovery benefit. No retrospective fence removal,
reference change or rescoring is part of this result.

## Calibration-only diagnosis for the next version

Only the four calibration cases were examined for the follow-up prompt change.
All eight calibration replies were fenced; three used the literal string
`passed/total`, two marked 11/12 as pass, and one represented tests as an object.
All eight ended at EOS below the 96-token cap. This motivates explicit raw-JSON,
string-type, numeric formatting and equality instructions, not a looser validator
or larger cap. These observations are hypotheses about prompt clarity, not proof
that new wording improves the model.

## Cost

The live job lasted 358 seconds: six rounded Linux minutes at $0.006/minute,
nominally $0.036. Two associated CI jobs each rounded to one minute, making
$0.048 nominal compute for that push. The 40,165-byte artifact would add roughly
$0.0000092 gross storage for its 30-day retention at $0.25/GiB-month. Included
allowances may reduce charges; actual account billing was unavailable. These
are list-price estimates, not a confirmed invoice or a claim of free execution.

Sources: [runner pricing](https://docs.github.com/en/billing/reference/actions-runner-pricing)
and [Actions billing](https://docs.github.com/en/billing/concepts/product-billing/github-actions).
No second live run is authorized by this record.
