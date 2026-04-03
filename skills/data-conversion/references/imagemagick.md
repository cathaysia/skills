# ImageMagick Convert

Use ImageMagick `convert` for image format conversion and simple image pipeline
operations around that conversion.

Official references:

- Download and install: https://imagemagick.org/script/download.php
- Format overview: https://imagemagick.org/script/formats.php
- Command reference: https://imagemagick.org/script/convert.php

## Common commands

Convert JPG to PNG:

```bash
convert input.jpg output.png
```

Convert PNG to WebP:

```bash
convert input.png output.webp
```

Convert TIFF to JPG with quality setting:

```bash
convert input.tiff -quality 92 output.jpg
```

Resize while converting:

```bash
convert input.png -resize 1600x1600 output.jpg
```

List formats supported by the local build:

```bash
convert -list format
```

## Practical format notes

Common formats handled by ImageMagick builds include:

- Raster images: `png`, `jpg`, `jpeg`, `gif`, `tiff`, `bmp`, `webp`, `heic`
- Working or sequence formats may vary by delegates compiled into the build

The official formats page lists coders and delegates, but local support still
depends on how ImageMagick was compiled and which optional libraries are
installed.

## Installation

If `convert` is missing, ask before proposing installation.

Typical installation approaches:

- macOS with Homebrew: `brew install imagemagick`
- Debian or Ubuntu: `sudo apt install imagemagick`
- Manual install and binaries: https://imagemagick.org/script/download.php

## Caveats

- `convert` here means ImageMagick. On some systems the executable may be
  `magick`, with usage like `magick input.jpg output.png`.
- HEIC, WebP, and some TIFF variants depend on optional delegate libraries.
- Metadata preservation is format-dependent. Do not assume EXIF survives every
  conversion unless you verify it.
