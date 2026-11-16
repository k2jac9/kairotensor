# Report assets and export

`correction-paths.dot` is the editable source for Figure 1. Its SVG and PNG are
presentation artifacts, not new empirical evidence. Render with Graphviz:

```sh
dot -Tsvg -Gdpi=200 correction-paths.dot -o correction-paths.svg
dot -Tpng -Gdpi=200 correction-paths.dot -o correction-paths.png
```

The accepted render used installed Graphviz 15.1.1. The native document embeds
the PNG at 420 by 116.1 points; the caption states the full explicit reuse boundary.

Google Docs was edited in place. Its public batch-update request catalogue does
not expose insertion of a page-number AutoText field:
https://developers.google.com/workspace/docs/api/reference/rest/v1/documents/request
The final delivery therefore adds numbered footers after exporting the native PDF.

In an isolated environment with the hash-pinned wheel in `pdf-requirements.txt`:

```sh
python number_pdf_pages.py native-export.pdf numbered-report.pdf
```

The helper preserves the input, refuses an existing output, uses an artifact-marked
footer and isolates the original graphics state. The seven body renders, extracted
text and all URI annotations were compared before/after numbering. This helper
is an export step, not an application dependency or a claim of universal PDF
compatibility. The accepted input is a non-encrypted, unrotated Google Docs export.

The native exporter repeats one title destination key at two positions on the
same title page. pypdf emits a normalization warning; the original export and
warning are retained. Body pixels, text and URI links are unchanged by the step.
