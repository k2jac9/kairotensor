# Why the recovery pilots failed

This diagnosis does not change either frozen score or tune against held-out
outcomes. Both pilots remain zero successful held-out cases under the strict
contract. See the [v1](recovery-policy-v1-2026-11-14.md) and
[v2](recovery-policy-v2-2026-11-14.md) reports for source/run identities.

## Supported findings

1. **Formatting failed first.** All 40 replies across the two pilots were
   Markdown-fenced JSON. `validate_answer` parses the entire reply as a strict
   three-field object; a leading code fence therefore rejects it. V2's explicit
   no-fences instruction was present in every exact request, so this is not a
   missing-prompt/version switch. No output hit the 96-token cap.
2. **Formatting is not the only defect.** As a diagnosis only, parsing the inner
   object after removing an exact fence reveals 4/20 v1 replies and 10/20 v2
   replies matching their references. These are not official scores, and include
   replies that were late or belonged to different held-out inputs. On the same
   four calibration cases, the corresponding counts are 2/8 and 5/8. V2 has no
   count-string errors on its 17 consistent-evidence replies, but seven wrong
   status values. Three retry contexts deliberately contain conflicting evidence
   and have no admissible reference. Merely stripping fences cannot solve those
   semantic/contradiction or timing failures.
3. **The slow phase was generation, not Store overhead.** Total generation time
   rose from 159.59s to 1,299.09s while output tokens changed from 681 to 696.
   Observed throughput fell from 4.27 to 0.536 output tokens/s. Input tokens also
   rose from 2,297 to 4,545 with the longer prompt. Preparation remained small.
4. **Hardware was not controlled.** V1 reported EPYC 9V74 with AVX512/BF16 flags;
   v2 reported EPYC 7763 without those flags. Both requested bfloat16, two threads,
   torch 2.6.0+cpu and transformers 4.48.3. PyTorch's optimized CPU BF16 matrix
   paths are feature-gated, making a slower fallback plausible. This is a strong
   hypothesis, not proof that hardware alone caused the full 8.1x time increase.
5. **Retry admission is not a completion guarantee.** V2 initially replied at
   51.5–58.5s. Each retry was admitted while still below the 60s operation deadline,
   then finished at 124.1–137.4s. The implementation neither resets the deadline nor
   preempts a call after dispatch. It lacks a remaining-time estimate, so it can
   spend on a retry very unlikely to finish on time. Late usage is still charged.
6. **Feedback is weakly corrective.** Retry uses a fresh two-message request with
   generic `validation_error` instructions. It does not include the previous
   answer or field-specific findings. That gives the model less corrective
   information than an explicit error-directed retry.

The worker's chat-template and completion slicing follow Qwen's documented
inference flow. Adapter/worker source snapshots match across runs. There is no
observed request/template bug, but saved artifacts do not include rendered token
IDs or profiler/kernel dispatch traces, so exact tokenizer/kernel behavior cannot
be independently proven from these bundles alone.

## Cheapest next checks, in order

- **No inference:** add regression tests for explicit formatting/semantic error
  feedback, remaining-time retry admission, and runtime/hardware preflight. Keep
  uncertain-send handling and existing strict validation intact.
- **Small approved calibration benchmark:** on the same machine and same input,
  compare BF16 and FP32 with fixed threads; record CPU flags, warmup, rendered token
  IDs and kernel/profiler evidence. This separates dtype/hardware effects from
  changed prompts. Do not use a fresh heterogeneous hosted job as the comparator.
- **Calibration-only output experiment:** test constrained JSON generation or
  verified output construction against the unchanged validator, separately from
  semantic accuracy. Compare generic feedback with field-specific feedback.
  If these structured report calculations are used in production, deterministic
  validated code is a sensible baseline rather than requiring a model to compute
  equality and format numbers.
- Freeze any resulting settings, then create another fresh held-out set. Do not
  rescore old fenced output as success, lengthen the deadline after seeing failure,
  or pay for another full comparison before calibration resolves the blockers.

No additional paid run is authorized by this diagnosis. Ordinary offline tests
can proceed; subsequent inference requires an explicit bounded cost approval.

## Primary implementation references

- [Qwen chat inference](https://qwen.readthedocs.io/en/v2.5/inference/chat.html)
- [PyTorch CPU BF16 explanation](https://pytorch.org/blog/empowering-pytorch-on-intel-xeon-scalable-processors-with-bfloat16/)
- [PyTorch 2.6 BF16 matrix path](https://github.com/pytorch/pytorch/blob/v2.6.0/aten/src/ATen/native/mkldnn/Matmul.cpp)
- [PyTorch 2.6 CPU feature checks](https://github.com/pytorch/pytorch/blob/v2.6.0/aten/src/ATen/native/mkldnn/Utils.h)
- [PyTorch 2.6 BLAS fallback](https://github.com/pytorch/pytorch/blob/v2.6.0/aten/src/ATen/native/CPUBlas.cpp)

These sources explain mechanisms; only the saved pilot artifacts establish the
reported measurements, and neither source nor artifacts constitute a controlled
causal performance experiment.
