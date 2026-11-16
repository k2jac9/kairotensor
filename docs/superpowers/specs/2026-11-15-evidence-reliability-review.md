# KairoTensor evidence reliability: verified review and design decisions

Date: 15 November 2026, America/Toronto.
Scope: **review and planning only**. No proposed production feature is implemented
by this delivery. The owner separately authorized a 48-hour storage exception.

## 1. Source and recent work

Remote `main` remains `20fab84a6baa525452191ecf80eeee339f0b5ac7`.
The original checkout is clean on `docs/epistemics-submission-20261115`,
`99b6fe24177e56b450c543c95723961fed440d4d`. Its changes are the submission report,
provenance disclosure, evidence index and reproduction material prepared in this
conversation. Preserve that branch, its PDF and archived experimental evidence.

- [PR #19](https://github.com/k2jac9/kairotensor/pull/19) is draft, open and unmerged;
  both final-head CI checks passed. It changes documentation only.
- [PR #18](https://github.com/k2jac9/kairotensor/pull/18) is draft, open and unmerged
  at `a8bcc705e4047765b34f6152aa303a6d40c5c34d`. Its hosted transport and accounting
  code has offline/mock verification, not confirmed provider execution.
- The supplied ChatGPT conversation link returned no readable content. The full
  plan pasted by the owner is the design input for this review.
- Review receipts are retained outside the checkout under
  `/home/neo/Projects/kairotensor-artifacts/reliability-review-20261115/`.

The planning worktree is based directly on main. Neither draft PR is silently
merged or treated as implemented main-branch functionality.

## 2. What exists, and what remains proposed

| Area | Verified source | Current boundary / remaining work |
| --- | --- | --- |
| Current read and resume | `src/attempts.rs:85`, `:316`, `:342` | Both already hold a database transaction around eligibility and accepted output. There is no `explain_current`. Extract and share this gate; do not claim a demonstrated existing revocation race. |
| Support eligibility | `src/support.rs:41-64` | A surviving support/v1 verdict can remain current across an epoch change. Its boolean helper loses the distinction between a non-support head and a changed support verdict. |
| Atomic correction | `src/support.rs:304`; `src/lib.rs:138` | One membership plus affected publications is atomic; bulk grants exist. Mixed withdraw/admit correction with expected-epoch compare-and-swap is absent. |
| Model reuse | `src/model_reuse.rs`; `tests/model_reuse.rs:55` | Explicit closed-computation resolution can reuse one sampled response. Ordinary current reads still use coarse invalidation for model heads. |
| Recovery and timing | `src/recovery.rs:84`; `src/timed_recovery.rs:256`; `src/recovery_runtime.rs:69` | Timed capture is complete relative to its requested list. Claim, post-claim clock check and settlement exist. This does not validate a semantic source mapping or prove that the declared list is complete. |
| Incident workload | `scripts/incident_routing.py:184`; `docs/incident-routing-workload.md` | Frozen rendering and protocol/scoring separation exist. Eligibility flags are declared fixture state; an actual Rust Store-backed incident bridge is absent. |
| General replay / policy registry | Existing recovery audit and Python policy replay | No `DecisionBundleV1`, diagnostic authorizer, general replay API or Store policy registry was found. These remain separate future designs. |
| Dependency identity | `Cargo.toml`; `.github/workflows/check.yml` | No tracked `Cargo.lock` exists. The current CI uses Cargo without `--locked`; a source SHA alone does not identify its resolved dependencies. |

## 3. Changes to the supplied plan

### First establish a reproducible baseline

A fresh `cargo test --locked --offline --all-targets` failed because the baseline
has no lockfile. A subsequent offline resolution produced a retained candidate
lock, then failed because `ahash 0.8.12` was unavailable locally. These failures are
preserved. Any successful later run uses the newly recorded resolution; it is not
an assertion about the dependency identity of historical CI.

Before feature changes, qualify and commit a lockfile and make CI enforce it.
Record the tested Rust identity. Maintain a separate, explicit runtime-policy
adoption record: latest stable, upcoming preview and previous supported stable
must be freshly identified, pinned and tested before claiming that matrix.
Neither an observed local compiler version nor old passing CI establishes it.
Do not combine unreviewed dependency upgrades with the gate refactor.

### Explain the gate that actually serves bytes

The payload-free evaluator must serve `read_current`, `explain_current`, and
`begin_resume`. Include `src/support.rs` in the allowed files so the evaluator can
distinguish non-support heads from changed support verdicts without inventing
another authority. Keep current snapshot semantics: a read established before a
revocation may finish using that earlier coherent snapshot. A snapshot that
already contains the revocation must not return the revoked result.

Independent public reads can observe different commits. Do not write a race test
that assumes two separate calls are one snapshot. The existing raw-SQL test in
`tests/atomic_revision.rs:220` does not exercise the production read path. Add a
controlled test-only barrier in the real gate-to-payload path and test both
revocation and concurrent head replacement there.

The final independent review also identified a false-confidence risk in the
proposed mutation test: immutable historical bytes fetched by a saved head ID
can look correct even after splitting transactions. The revised plan separately
asserts transaction liveness and the retained epoch/head snapshot at the payload
boundary, then checks observable metadata/output consistency.

### Batch only effective edits, and publish the final state

Do not implement a batch by looping `set_admitted_and_revise`: that would publish
intermediate verdicts and charge for them. Use one Immediate transaction and
evaluate the union of consumers after applying all effective edits.

An unchanged requested membership must not repair an already-stale consumer as
an incidental side effect. Existing no-op behavior deliberately avoids that
(`tests/atomic_revision.rs:164-175`). A stale expected epoch conflicts even for
empty or all-no-op batches. Duplicate IDs are invalid. This remains optimistic
concurrency, not an idempotent operation-key API.

Proposed additional bound: at most **1,024 edits per batch**. This is a new API
limit for review, not an existing measured capacity result. Specify malformed
request and epoch-conflict precedence in the execution plan and test it.

### Make the semantic bridge a separate next milestone

The following points must be frozen before its implementation WorkOrder:

1. Map `(workload_version, case_id, external_evidence_id)` to one Store occurrence.
   Evidence IDs such as `e01` repeat across cases. Reject omissions, duplicates,
   aliases, empty mappings, unknown fields and version mismatches.
2. Capture the question, owner roster, policy/context and report text that can
   change the computation, or bind immutable configuration in its versioned
   contract. Render from `CapturedInputs`; do not substitute corpus payloads or
   caller-declared current flags.
3. Preserve exact Python renderer bytes for the frozen v1 fixture. Pin a reviewed
   SHA-256 dependency in `Cargo.toml` and `Cargo.lock` here, before bundle export
   needs it. Do not write cryptography by hand.
4. Use strict typed parsing that rejects duplicate JSON keys and unknown fields.
   Match the frozen protocol's depth limit of 64. Generic `serde_json::Value`
   alone loses duplicate keys. Keep semantic labels out of request construction,
   admission, runtime validation and retry feedback; well-formed wrong answers
   are not magically detected by protocol validation.
5. Reuse `RecoveryStart`, `RecoveryPreparation`, reservation, timed claim and
   runtime settlement. `Existing(id)` is not a new send opportunity. The public
   reservation reader exposes reserved request/count/cap, not arbitrary captured
   input reconstruction. Initially refuse ambiguous restart states; richer
   reconstruction needs its own reviewed interface.
6. Revocation before claim refuses dispatch. Revocation after claim may be too
   late to prevent invocation: refuse stale publication and retain incurred
   usage/ambiguous holds. A timer cannot revoke a send that already happened.
7. Fixtures may invoke canned adapters. They must issue zero provider/model
   calls and must not be reported as model-quality measurements.

## 4. Prioritized delivery boundaries

| Boundary | Deliverable | Exit gate |
| --- | --- | --- |
| A0 | Reproducible baseline and visible runtime-policy gaps | Committed tested lock; CI refuses unreviewed resolution; exact source/runtime/lock receipt. |
| A1 | Payload-free current eligibility explanation | Shared read/resume evaluator; parity and controlled race tests; no migration. |
| A2 | Expected-epoch atomic membership correction | Final-state equivalence, rollback, one-winner concurrency, unchanged accounting and no-op behavior; no migration. |
| B | Store-backed incident fixture | Strict case-scoped mappings, captured bytes, byte-identical rendering, label-independent validation and existing timed gates. Separate implementation plan. |
| C | Authorized decision export and deterministic replay | Freeze bundle schema, explicit error type, authorizer contract, redaction boundary and versioned validator registry first; no execution during replay. |
| D | Evaluated policy activation | Freeze counters/denominators, authority, supported artifacts, exact comparison manifest and next free schema version; bootstrap/promote/rollback have separate gates. |
| E | Portable envelope and bounded diagnostics | Only after a named second consumer needs it. No live ablation execution implied. |

Do not put C-E in the same execution batch as A. A policy registry must not
manufacture send authority; a stored evaluation remains an attestation. Preserve
unknown usage and every uncertain hold. Rollback advances policy history and
requires current compatibility/authority, not proof of superiority. A repeatedly
used holdout becomes selection data; a final comparison needs a fresh lockbox.

The original references to MemoryDelta, RecallTrace, Precedent, Bodega,
NeverTwice and Countersign lack repository URLs, commit identities and license
records in the pasted text. Their implementations were not independently
verified in this review. Obtain those identities before copying code or claiming
a source-pinned comparison. The architectural ideas do not require copying them.

## 5. Adaption remains a separate experiment

The owner authorized one synthetic DeepSeek V4 Flash smoke request, at most one
Adaption credit / US$1, with no retry, top-up or subscription change. That
authorization is retained for a future execution pass; this pass is review only.

Fresh observations: PR #18 is unchanged and unmerged; `ADAPTION_API_KEY` is not
present in this process environment. A bounded filename inventory of the local
repository and its artifact directory found no hosted/Adaption smoke receipts.
That does not prove another runner never sent a request or that no charge exists.
The connected Adaption tools expose training/dataset operations, not proof of
this account's inference model access or all-category request pricing.

Before any launch, reconcile possible prior claims/requests across runners and
provider records, verify exact model access, an all-in one-request billing bound,
credit eligibility, secure credentials and request identity. Keep the full
monetary hold through unknown remote completion. An absent local receipt or a
timeout is not permission to resend. Hosted timing/accounting is not equivalent
to the local Qwen experiment.

## 6. Verification and operational constraints

Fresh review checks passed: **185 Rust tests across 24 test binaries**, with no
failures or ignored tests, plus 18 reference, 8 belief-oracle and 88 incident
Python tests (**114 Python tests**). The Rust command was
`kairotensor-cargo --budget-gib 2 test --locked --all-targets`, using
`rustc 1.99.1 (58cf8f9c3 2026-10-13)` and the generated verification lock with
SHA-256 `0298c2ecbad412b31f56d8c712bbea76c1b4cb0e055fdaee009bf967b170a820`.
The lock, `rust-qualified-baseline.json`, and complete logs are retained in the
review artifact directory. This qualifies that newly recorded resolution; it
does not identify historical CI dependencies, execute the entire CI workflow,
or establish the requested runtime matrix. No model/provider calls were made.

An independent reviewer inspected the main-branch gates and tests. Its findings
are incorporated above: read/resume sharing, real-path race coverage, final-state
batch publication, effective-change/no-op semantics, case-local evidence IDs,
strict label-independent parsing and narrower restart guarantees. These are plan
corrections and coverage gaps, not claims that the new features already exist.

Normal storage preflight refused the proposed 2 GiB Rust build. The owner then
authorized a two-day exception. A project-scoped launcher now permits a 20 GiB
floor until **2026-11-17 18:04:38 UTC / 13:04:38 Toronto**. It uses the existing
reservation ledger and ENOSPC checks. `kairotensor-cargo --budget-gib 2 ...` uses
the installed Cargo recipe with a scoped admission entry; `kairotensor-storage`
provides its preflight/status. A persistent user timer expires the policy, and
each launch independently checks the deadline. The root policy stays at 50 GiB.
No existing reservation or user data was deleted. The exception does not
authorize feature implementation or provider spending.

## 7. Recommendation

Review and execute boundary A first using the accompanying plan. Treat the
submitted research snapshot as frozen historical evidence. Measure new behavior
on new source-bound fixtures, and amend research claims only when new results
exist. Keep B-E and the one-request hosted smoke as separately reviewed work.
