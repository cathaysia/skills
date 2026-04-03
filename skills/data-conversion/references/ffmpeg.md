# FFmpeg

Use `ffmpeg` for audio and video conversion, audio extraction, remuxing, and
codec changes.

Official references:

- Download and install: https://ffmpeg.org/download.html
- General documentation: https://ffmpeg.org/documentation.html
- Formats: https://ffmpeg.org/ffmpeg-formats.html
- Codecs: https://ffmpeg.org/ffmpeg-codecs.html

## Common commands

Convert MP4 to AVI:

```bash
ffmpeg -i input.mp4 output.avi
```

Extract MP3 from MP4:

```bash
ffmpeg -i input.mp4 -vn -c:a libmp3lame output.mp3
```

Convert WAV to MP3:

```bash
ffmpeg -i input.wav -c:a libmp3lame -q:a 2 output.mp3
```

Convert MOV to MP4 with H.264 video and AAC audio:

```bash
ffmpeg -i input.mov -c:v libx264 -c:a aac output.mp4
```

Copy streams into a new container without re-encoding:

```bash
ffmpeg -i input.mkv -c copy output.mp4
```

List formats supported by the local build:

```bash
ffmpeg -formats
```

List codecs supported by the local build:

```bash
ffmpeg -codecs
```

## Practical format notes

Common containers and file types include:

- Video containers: `mp4`, `mkv`, `mov`, `avi`, `webm`, `mpeg`, `ts`
- Audio containers and files: `mp3`, `aac`, `m4a`, `wav`, `flac`, `ogg`, `opus`

Actual support depends on how the local FFmpeg binary was built. The official
formats and codecs docs describe the feature surface, but the installed binary
is the source of truth for what is available on the machine.

## Installation

If `ffmpeg` is missing, ask before proposing installation.

Typical installation approaches:

- macOS with Homebrew: `brew install ffmpeg`
- Debian or Ubuntu: `sudo apt install ffmpeg`
- Manual download or platform packages: use https://ffmpeg.org/download.html

## Caveats

- Container conversion and transcoding are different. `-c copy` only works when
  the target container can hold the existing streams.
- Some conversions need explicit codecs for compatibility.
- Media operations can be slow and lossy when re-encoding; preserve originals
  unless the user asks to overwrite them.
