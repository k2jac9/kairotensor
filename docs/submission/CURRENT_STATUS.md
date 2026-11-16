# KairoTensor submission handoff: 16 November 2026

This is a dated supplement to the frozen report, with live status observations
recorded at approximately 07:39 UTC after PR integration and branch cleanup.
The paper and PDF still report source
`20fab84a6baa525452191ecf80eeee339f0b5ac7`. The subsequent template review replaces
stale branch-status wording with an explicit measured-scope boundary, adds three
contribution bullets and a numbered results table, and links the reviewer package.
The abstract and original result paragraphs are unchanged. No result below is
silently added to the paper.

The reference-inspired editorial successor adds a concrete opening, Figure 1,
finding labels and an appendix reproducibility table. The author provenance
declaration moves intact to that appendix. Consistent typography and tighter
spacing reduce the report to seven pages: five main, one references, one appendix.
Numbering is applied to the PDF export; the native editable Doc retains the same
content and diagram. See the new editorial verification receipt for exact hashes.

## Integrated implementation

PRs [20](https://github.com/k2jac9/kairotensor/pull/20),
[19](https://github.com/k2jac9/kairotensor/pull/19),
[18](https://github.com/k2jac9/kairotensor/pull/18) and
[21](https://github.com/k2jac9/kairotensor/pull/21) are merged. The integrated source
is `109798d776b93d6730b2d6bac74d0fd328c19815`.
[Main CI run 43821777107](https://github.com/k2jac9/kairotensor/actions/runs/43821777107)
passed all six jobs on that exact source. The five contract lanes are:

| Lane | Rust | Python |
| --- | --- | --- |
| Stable | 1.100.0 | 3.15.2 |
| Upcoming Rust | nightly-2026-11-16 / 1.102.0-nightly | 3.15.2 |
| Previous and retained Rust | 1.99.1 | 3.15.2 |
| Upcoming Python | 1.100.0 | 3.16.0a2 |
| Previous Python | 1.100.0 | 3.14.10 |

Each contract lane passed 365 Rust tests, 462 Python script tests, 18 reference
tests and eight oracle tests, plus the explicit integration/example stages. The
retained logs contain no compiler/Python warnings or test skips. The sixth job
checks previous-checkout compatibility. These are overlapping runtime checks,
not five independent model experiments. The separate scheduled recovery workflow
was not dispatched as part of this integration.

The Store now includes shared current-eligibility explanations, atomic admission
batches, the fixture-backed incident bridge, authorized decision export/replay,
and evaluated-policy activation. See the
[reliability review guide](../reliability-review-guide.md). The hosted adapter from
PR #18 remains qualified through offline transport/process fixtures; provider
compatibility and complete live billing reconciliation are not established.

## Separate controlled-correction pilot

The [observed results](../controlled-correction-results.md) and
[protocol](../controlled-correction-pilot.md) describe a later local experiment at
`f6694110e3df5b1a13f71dbf9fae2bde89e07c44`. Its cached Ornith/Ollama execution is
separate from the report's pinned Qwen measurements and the incident fixtures.

All four planned changes completed. Post-change repair used four calls and 751
reported tokens; fresh recomputation used six calls and 1,167 tokens. Including
the shared three-call bootstrap once, collection recorded 13 calls and 2,515
tokens. Corresponding generated requests and checked plans matched; withdrawal
refused before dispatch in both arms. The sample is one correlated synthetic
example per condition. It establishes neither population accuracy nor latency
savings, and its configured token envelopes are not proven hard billing bounds.

The reviewer handoff includes the corrected pilot supplement with SHA-256
`53061caf647e72a51cd5410b545a7f774346646e31feb28ecbdf91e177eb54e2`.
This successor fixes three guide links; raw evidence remains unchanged. Its
original cloud copy has SHA-256
`d1e6160e0f7e19777cefb7cd3005e5666447bc87edf7b213c83629a0da66487b`
and is not the corrected successor.

## Training and delivery boundaries

The existing AutoScientist run `fb16665b-f02c-4cdc-bbde-22cacfd81830` was still
running when checked on 16 November: one of three iterations completed, reported
best win rate 0.1689 against target 0.8, no error and no downloadable model.
That platform metric has not been independently evaluated and is not a KairoTensor
research result. No dataset expansion, restart or new training run is part of this
handoff. A later observation must recheck the run rather than assume this status.

The repository is now public, verified on 16 November at 08:36 UTC through
unauthenticated requests. GitHub reported `private: false` and `visibility: public`;
the main README and the measured `20fab84` README returned HTTP 200, with the
latter matching the local Git snapshot byte for byte. The
[verification receipt](verification/public-access-20261116/anonymous-check.json)
records the exact URLs and hashes. This closes the source-access gap.

At the 08:36 UTC check, the public PDF on main was an earlier export (SHA-256
`a755007e453b633cae4f19d38fd1c1f557c365a04d07ddfffb52d5753bf10c5a`).
The earlier editorial successor had SHA-256
`86bf058fcaf0cf5ed886af7b198a7a6aff9978bac5e2994a5a217e9f8d4c692c`.
The current presentation successor in this revision has SHA-256
`5350a885ee9be61300b2743ef35f78bed674e283578a5c4aa487edd4f0b9b476` and seven pages. It adds restrained teal
accents and directs readers to the public measured source. Abstract, results and
author declaration are unchanged. The repository overview now links the research
narrative, a documentation index and the preserved detailed Store guide. See
[presentation verification](verification/presentation-20261116/README.md).

The owner authorized read-only link access to the final Drive report PDF and
combined reviewer bundle. The last Drive checks at 08:29 UTC still showed
owner-only access and anonymous reads reached sign-in. The connector supports
named-user and Workspace-domain sharing but does not expose "Anyone with the
link." GitHub visibility does not change these Drive permissions. Earlier PDF
and package statements about repository privacy are dated and superseded by this
note and the current paper; old verified bytes and historical receipts remain unchanged.

The refreshed combined reviewer bundle contains the current report package,
corrected pilot package, this status note, the exact integrated source and
source-bound CI receipts. Its current report package retains the measured-source
archive and raw evidence unchanged. Prior packages remain retained locally as
historical evidence. Private submission-form contact details and account-access
observations are omitted from the reviewer distribution. Its manifest verifies
byte consistency; it does not authenticate model execution.

The [official sprint page](https://apartresearch.com/sprints/ai-x-epistemics-research-sprint-2026-11-13-to-2026-11-15)
lists the submission deadline as 16 November at 11:59 UTC, which is 06:59 in
America/Toronto. The report PDF is the primary upload; the form also accepts an
additional-material URL. A local ZIP or owner-only link is not an accessible URL
for judges. The public repository can now serve as a source/evidence URL; public
access to the latest Drive PDF and combined bundle is still unverified.
Event submission remains a separate action; this preparation establishes no
event receipt.

Ready form values remain: title **KairoTensor: Evidence-Grounded Revision for Agent
Memory**; team **KairoTensor**; location **Online**; author **Alejandro Castellanos**;
affiliation **Independent Research**; publishing interest **Yes**; primary track
**Track 2: Trust Infrastructure**. The abstract is the unchanged 192-word text in
the [report](report.md). Contact details are retained in the private handoff.
