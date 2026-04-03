# pdftotext

Use `pdftotext` for extracting plain text from PDF documents when the goal is
text output rather than format-preserving conversion.

Official references:

- Poppler project: https://poppler.freedesktop.org/
- Debian man page example: https://manpages.debian.org/bookworm/poppler-utils/pdftotext.1.en.html

## Common commands

Extract text to a `.txt` file:

```bash
pdftotext input.pdf output.txt
```

Extract text and preserve physical layout as much as possible:

```bash
pdftotext -layout input.pdf output.txt
```

Extract only pages 2 through 5:

```bash
pdftotext -f 2 -l 5 input.pdf output.txt
```

Write extracted text to standard output:

```bash
pdftotext input.pdf -
```

Generate HTML-wrapped text with metadata:

```bash
pdftotext -htmlmeta input.pdf output.html
```

Generate TSV with bounding box metadata:

```bash
pdftotext -tsv input.pdf output.tsv
```

## Practical format notes

`pdftotext` is specialized for:

- Input: PDF
- Output: plain text, plus auxiliary HTML or TSV variants for metadata-oriented extraction

It is the right tool when the user wants readable extracted text, not when they
want layout-faithful HTML, DOCX, or image output.

## Installation

If `pdftotext` is missing, ask before proposing installation.

Typical installation approaches:

- macOS with Homebrew: `brew install poppler`
- Debian or Ubuntu: `sudo apt install poppler-utils`
- Package and project information: https://poppler.freedesktop.org/

## Caveats

- Text extraction quality depends heavily on the PDF's internal text layer.
- Scanned PDFs often need OCR, which `pdftotext` does not perform.
- Complex columns, tables, headers, and footers may produce text in an order
  that needs cleanup.
