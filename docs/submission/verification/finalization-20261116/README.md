# Submission finalization receipts

These new receipts cover the 16 November 2026 report and reviewer-package
finalization. The earlier files in the parent directory remain immutable and
describe the 15 November package, including its six-page PDF.

- `report-check.json`: the seven-page final PDF, unchanged 192-word abstract,
  preserved measured results, manuscript/native-document/PDF agreement, and
  visual review of all seven pages. The main report including title ends on
  page 5; references occupy page 6; the ethics appendix and LLM statement are on
  page 7. Link annotations point to the cited primary sources and exact Git
  snapshot; this does not establish anonymous access to the private repository.
- `source-archive-check.json`: fresh byte comparison of all 145 files (162 ZIP
  entries including directories) against Git snapshot
  `20fab84a6baa525452191ecf80eeee339f0b5ac7`.
- `related-work-check.md`: primary-source checks and the bounded contribution.
- `reproduction-check.json` and matching stdout/stderr files: the documented
  commands run after a fresh candidate-package extraction under Python 3.15.2.
  Reference tests: 18 passed. Belief oracle: 8 passed. Script tests: 203 total,
  28 skipped, 175 executed without failure. Two unclosed SQLite connection
  warnings remain visible. Both benchmark summaries match the historical bytes.
  The read-only recovery audit reconciles 20 recorded calls and 5,241 charged
  tokens with zero outstanding reservations; held-out success remains zero.

The final package includes these fresh receipts. Its manifest and fresh unpack
are verified separately after packaging, and the measured-source ZIP bytes are
required to equal the archive used for the command run. That outer archive
receipt is retained alongside the final ZIP and in the repository, rather than
inside the ZIP whose checksum it records. The nested recovery audit's field
`new_model_calls: 20` describes historical collection; this finalization made
zero new model calls.

No new model inference, dataset creation or training occurred during this
finalization. Later reliability work, PR #18 and active AutoScientist runs remain
excluded from the measured results. No fresh Rust run or complete
stable/preview/previous-stable matrix is claimed. Visibility and sharing settings
were not changed, and no event entry was submitted.

The official-template document was edited in place with revision guards. Before
and after snapshots, the earlier and final exports, rendered pages and packaging
records are retained in the separate finalization artifact directory. The
historical submission artifact directory was read only throughout this work.
