# Pandoc

Use `pandoc` for text and document conversion across markup and common document
formats.

Official references:

- Install: https://pandoc.org/installing.html
- User guide and manual: https://pandoc.org/MANUAL.html
- Supported input and output formats: https://pandoc.org/demo/

## Common commands

Convert HTML to GitHub Flavored Markdown:

```bash
pandoc input.html -t gfm -o output.md
```

Convert DOCX to Markdown:

```bash
pandoc input.docx -t gfm -o output.md
```

Convert Markdown to DOCX:

```bash
pandoc input.md -o output.docx
```

Convert Markdown to PDF:

```bash
pandoc input.md -o output.pdf
```

Show output formats supported by the local binary:

```bash
pandoc --list-output-formats
```

Show input formats supported by the local binary:

```bash
pandoc --list-input-formats
```

## Practical format notes

Pandoc supports many textual and document formats. Common examples include:

- Inputs: `markdown`, `gfm`, `html`, `docx`, `odt`, `epub`, `latex`, `rst`
- Outputs: `gfm`, `markdown`, `html`, `docx`, `odt`, `epub`, `latex`, `pdf`

When converting to Markdown, prefer GitHub Flavored Markdown unless the user
asks for another dialect:

```bash
pandoc input.docx -t gfm -o output.md
```

Some targets rely on external engines. For example, PDF output often needs a
LaTeX engine or another PDF-capable backend available on the machine.

## Installation

If `pandoc` is missing, ask before proposing installation.

Typical installation approaches:

- macOS with Homebrew: `brew install pandoc`
- Debian or Ubuntu: `sudo apt install pandoc`
- Official packages and binaries: https://pandoc.org/installing.html

## Caveats

- Rich formatting does not always round-trip cleanly across formats.
- Embedded media, comments, tracked changes, and advanced layout features may
  be simplified or lost.
- PDF generation can fail even when `pandoc` is installed if no PDF engine is
  present.
