# Editorial successor verification

Base `eebfdef26be08f4edb9fc6d975fcea20f0ffd04f`. The measured research source remains
`20fab84a6baa525452191ecf80eeee339f0b5ac7`.

Changes implement the approved reference-paper review: concrete correction
example, Figure 1, finding labels, consistent typography/spacing, page numbers,
and Table 2 on reproducibility. The author provenance declaration moves intact
to the appendix. The 192-word abstract and all four original Results paragraphs
are preserved; prior evidence, archives and verification receipts are unchanged.

[report-check.json](report-check.json) binds the native revision, original export
and final numbered PDF. Seven pages: five main including title, references on
page 6 and appendix on page 7. The diagram and caption are together, both tables
are legible, and section headings remain with their following text. All pages
were rendered and inspected.

Native Google Docs edits used revision guards. The PDF-only numbering helper
uses hash-pinned pypdf 7.1.0. Verification compares every page's body pixels, text
and URI links before/after numbering; only footer digits may differ. A native
export duplicate title-destination warning is retained and documented in the
asset guide. The original export remains available in the retained packet.

One formatting batch was rejected atomically because page-break-before is not
allowed in table cells; readback confirmed no revision change before a corrected
request. The first numbering candidate inherited the export's flipped coordinate
system; wrapping original content in a saved graphics state corrected the footer.
The first provenance-check probe accidentally removed a space while constructing
its expected string; the checker now slices the exact baseline declaration.
These candidates and failures are retained; no artifact was changed to satisfy
an incorrect expected value.

No new application tests, model calls, training or dataset expansion occurred.
The source-bound historical tests remain distinct from this document verification.
Authenticated cloud readback and anonymous access are separate checks; an uploaded
link alone does not establish reviewer access. No event submission is claimed.

Full packet: `kairotensor-artifacts/editorial-pass-20261116`, including native
snapshots, render candidates, raw export, numbered PDF, scripts, hashes, storage
receipts and independent review. Post-upload delivery receipts are kept outside
the already-hashed reviewer bundle to avoid a self-referential manifest.
