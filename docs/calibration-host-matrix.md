# Proposed GitHub and NixOS calibration comparison

**Preparation only, not execution approval.** The proposed hosts are a standard
GitHub-hosted Ubuntu x64 runner and the local NixOS machine `matrix-nixos`.
Readiness is pending for both. No live workflow, provisioning recipe, or run is
provided by this document, and no spending ceiling has been approved.

Use the [prepared backend](incident-transformers-backend.md) and its existing
durable controller, rather than the historical report-task workflows. The
[eight-probe plan](incident-prepared-bridge.md#emit-an-unexecuted-calibration-proposal)
defines the requests; this document adds the host-comparison requirements.

## Same experiment on each host

- Use the same final reviewed Git commit, clean tracked source, source hashes,
  and plan digest. Do not compare a released commit with a modified worktree.
- Use `Qwen/Qwen2.5-1.5B-Instruct`, revision
  `989aa7980e4cf806f80c7fef2b1adb7bc71aa306`, on CPU in BF16. Model and tokenizer
  assets must match by hash; there is no silent FP32, GPU, or revision fallback.
- Keep the frozen [incident-routing/1 workload](../workloads/incident-routing-v1/freeze.json),
  prompt, and contract unchanged. Execute only calibration cases `cal-01` through
  `cal-04`: four initial requests, then four fixed malformed-feedback requests.
  These are independent probes, not retries of the preceding answers. No gold
  labels enter requests, and no held-out cases are executed.
- Allow at most **8 calls per host, 16 total**. Each quote permits at most
  **2,048 input tokens and 96 output tokens**, with **17,152 charged-plus-held
  token units per host**. There are no warmup calls, automatic retries, or resumes.
- Allow **20 minutes all-in per host**, including dependency/asset provisioning,
  readiness checks, loading, preparation, generation, validation, audit, and
  artifact preservation. A future outer launcher must start its watchdog before
  provisioning and give the controller only the remaining time, reserving time
  for reporting. The current controller does not bound setup performed before
  its invocation. Insufficient time means an incomplete run, not a fresh clock.

The backend requires these exact package versions:
`torch==2.6.0+cpu`, `transformers==4.48.3`, `tokenizers==0.21.0`,
`huggingface_hub==0.34.4`, `safetensors==0.5.3`, `accelerate==1.3.0`,
`jinja2==3.1.6`, and `numpy==1.26.4`.
Freeze the exact Python version, wheel files/hashes, and remaining dependencies
before execution. A cross-host Python/wheel lock and a pinned Nix environment
are **not yet supplied**; these version requirements alone do not establish
reproducibility.

## Host readiness gates

1. Record the actual OS/image, kernel, libc, architecture, CPU model/flags,
   affinity, available RAM/disk, Python/packages, and asset hashes. For GitHub,
   choose an explicit Ubuntu image label and retain its actual image version;
   runner resources vary by repository/runner category.
   [GitHub runner reference](https://docs.github.com/en/actions/reference/runners/github-hosted-runners)
2. Agree on matching thread counts and affinity allocation before running, or
   explicitly report the difference. The backend currently chooses
   `min(4, available affinity CPUs)`; that does not ensure equal parallelism.
   Record actual intra-op/inter-op threads, CPU topology, Torch backend settings,
   and competing host load. Do not attribute a timing difference to the OS alone.
3. On NixOS, use a project-scoped pinned FHS environment or narrowly scoped loader
   wrapper. Require an import/version/linkage preflight before model loading.
   nix-ld alone is insufficient for extensions loaded by nixpkgs Python, which
   does not use its loader variables. Do not change system configuration, enable
   global loader settings, or substitute a different Nixpkgs Torch build.
   [nix-ld maintainer guidance](https://github.com/nix-community/nix-ld#my-pythonnodejsrubyinterpreter-libraries-do-not-find-the-libraries-configured-by-nix-ld),
   [Nixpkgs FHS environment documentation](https://nixos.org/manual/nixpkgs/stable/)
4. Retain the Nix environment/lock and closure identity, or the Ubuntu environment
   identity, alongside import results and wheel hashes. Exercise the offline
   watchdog/parent-death tests inside the selected environment before trusting
   its process containment. A loader, memory, version, or containment failure
   stops the run; no inference is needed to establish these readiness checks.

## Approval, artifacts, and interpretation

For GitHub's standard Linux 2-core x64 runner, the published rate is
**$0.006/minute**, with each job rounded up to whole minutes. Twenty billed minutes
would be **$0.12 nominal compute**, plus any artifact storage and ordinary CI
costs. Account allowances are unknown. Verify the applicable rate and allowances,
then obtain an explicit spending ceiling before dispatch; this estimate and the
token limits are not an account spending cap.
[Runner pricing](https://docs.github.com/en/billing/reference/actions-runner-pricing),
[Actions billing](https://docs.github.com/en/billing/concepts/product-billing/github-actions)

Keep separate per-host artifact directories: readiness/provisioning logs and
timings, runtime and prepared-request observations, raw backend receipts, ledger,
supervisor seal, audit, and an artifact checksum manifest. Preserve partial runs
and unresolved holds. The backend remains local-cache-only; any required package
or model download belongs to separately approved, bounded provisioning.

Compare diagnostics only. Each host has one first-after-load initial probe, three
later initial probes, and four fixed-feedback probes. These do not establish
cold-feedback coverage, arbitrary-feedback coverage, general model quality, or
production latency guarantees. A complete ledger does not mean correct incident
answers. **Never reuse a latency profile across these hosts**, and do not turn
these observations automatically into production admission authority.
