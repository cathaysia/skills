#!/usr/bin/env python3
"""Recognize an image with a local OpenAI-compatible vision model.

Wraps qwen/qwen3-vl-4b served at http://127.0.0.1:1234 (LM Studio / llama.cpp
style endpoint) so that non-multimodal models can still "see" images: read the
image from a file path or the system clipboard, send it to the vision model,
and print the recognition result.

Examples:
  python3 recognize_image.py --path screenshot.png
  python3 recognize_image.py --clipboard
  python3 recognize_image.py image.png "What color is the submit button?"
  python3 recognize_image.py --list-models
"""

import argparse
import base64
import json
import mimetypes
import os
import subprocess
import sys
import tempfile
import urllib.error
import urllib.request

DEFAULT_BASE_URL = "http://127.0.0.1:1234/v1"
DEFAULT_MODEL = "qwen/qwen3-vl-4b"
DEFAULT_PROMPT = (
    "Recognize the content of this image in detail. Describe the objects, "
    "scene, and any visible text. If there is text in the image, transcribe "
    "it completely."
)

MIME_BY_EXT = {
    ".png": "image/png",
    ".jpg": "image/jpeg",
    ".jpeg": "image/jpeg",
    ".gif": "image/gif",
    ".webp": "image/webp",
    ".bmp": "image/bmp",
    ".tif": "image/tiff",
    ".tiff": "image/tiff",
}


def parse_args(argv):
    p = argparse.ArgumentParser(
        description="Recognize an image from a file path or clipboard using a "
                    "local OpenAI-compatible vision model.",
        formatter_class=argparse.RawDescriptionHelpFormatter,
        epilog=(
            "If neither --path/--clipboard nor a positional path is given, "
            "reads from the clipboard."
        ),
    )
    p.add_argument("image", nargs="?", help="image file path (alternative to --path)")
    p.add_argument("--path", help="image file path to read")
    p.add_argument("--clipboard", action="store_true",
                   help="read the image from the system clipboard")
    p.add_argument("--prompt", default=DEFAULT_PROMPT,
                   help="recognition prompt (default: transcribe/describe in Chinese)")
    p.add_argument("--model", default=DEFAULT_MODEL,
                   help=f"model id (default: {DEFAULT_MODEL})")
    p.add_argument("--base-url", default=DEFAULT_BASE_URL,
                   help=f"OpenAI-compatible base URL (default: {DEFAULT_BASE_URL})")
    p.add_argument("--max-tokens", type=int, default=1024,
                   help="max completion tokens (default: 1024)")
    p.add_argument("--temperature", type=float, default=0.2,
                   help="sampling temperature (default: 0.2)")
    p.add_argument("--json", action="store_true",
                   help="print the full JSON response instead of just the text")
    p.add_argument("--list-models", action="store_true",
                   help="list models served by the endpoint and exit")
    return p.parse_args(argv)


def list_models(base_url):
    data = api_get(f"{base_url.rstrip('/')}/models")
    for m in data.get("data", []):
        print(m.get("id"))
    if not data.get("data"):
        print("(no models returned)", file=sys.stderr)


def api_get(url):
    req = urllib.request.Request(url, headers={"Content-Type": "application/json"})
    with urllib.request.urlopen(req, timeout=10) as resp:
        return json.load(resp)


def load_from_path(path):
    if not os.path.isfile(path):
        raise FileNotFoundError(f"image file not found: {path}")
    ext = os.path.splitext(path)[1].lower()
    mime = MIME_BY_EXT.get(ext) or mimetypes.guess_type(path)[0] or "image/png"
    try:
        from PIL import Image
        import io
        img = Image.open(path)
        img.load()
        if img.mode not in ("RGB", "RGBA"):
            img = img.convert("RGB")
        buf = io.BytesIO()
        fmt = "PNG"
        if mime == "image/jpeg":
            fmt = "JPEG"
        img.save(buf, format=fmt)
        return buf.getvalue(), ("image/png" if fmt == "PNG" else "image/jpeg")
    except ImportError:
        with open(path, "rb") as f:
            return f.read(), mime


def load_from_clipboard():
    # Strategy 1: Pillow ImageGrab (macOS, Windows, X11 with xclip).
    try:
        from PIL import Image, ImageGrab
        import io
        grabbed = ImageGrab.grabclipboard()
        if grabbed is not None:
            if isinstance(grabbed, list):  # Windows: list of file paths
                if not grabbed:
                    raise ValueError("clipboard image list is empty")
                return load_from_path(grabbed[0])
            img = grabbed
            if img.mode not in ("RGB", "RGBA"):
                img = img.convert("RGB")
            buf = io.BytesIO()
            img.save(buf, format="PNG")
            return buf.getvalue(), "image/png"
    except ImportError:
        pass
    except Exception as exc:
        raise ValueError(f"failed to read clipboard with Pillow: {exc}")

    # Strategy 2: macOS osascript (no Pillow needed).
    if sys.platform == "darwin":
        try:
            with tempfile.NamedTemporaryFile(suffix=".png", delete=False) as tmp:
                tmp_path = tmp.name
            script = (
                'set pngData to (the clipboard as «class PNGf»)\n'
                f'set outFile to open for access POSIX file "{tmp_path}" with write permission\n'
                "write pngData to outFile\n"
                "close access outFile\n"
            )
            subprocess.run(["osascript", "-e", script], check=True, capture_output=True)
            with open(tmp_path, "rb") as f:
                data = f.read()
            os.unlink(tmp_path)
            if not data:
                raise ValueError("clipboard does not contain an image")
            return data, "image/png"
        except subprocess.CalledProcessError as exc:
            raise ValueError(
                "clipboard does not contain an image or osascript failed"
            ) from exc
        except Exception:
            raise

    # Strategy 3: Linux (X11 / Wayland).
    for cmd in (["xclip", "-selection", "clipboard", "-t", "image/png", "-o"],
                ["wl-paste", "--type", "image/png"]):
        try:
            data = subprocess.run(cmd, check=True, capture_output=True).stdout
            if data:
                return data, "image/png"
        except (subprocess.CalledProcessError, FileNotFoundError):
            continue

    raise ValueError(
        "could not read an image from the clipboard "
        "(empty clipboard or no supported clipboard tool)"
    )


def chat(base_url, model, prompt, image_bytes, mime, max_tokens, temperature):
    b64 = base64.b64encode(image_bytes).decode("ascii")
    payload = {
        "model": model,
        "messages": [
            {
                "role": "user",
                "content": [
                    {"type": "text", "text": prompt},
                    {"type": "image_url",
                     "image_url": {"url": f"data:{mime};base64,{b64}"}},
                ],
            }
        ],
        "max_tokens": max_tokens,
        "temperature": temperature,
    }
    body = json.dumps(payload).encode("utf-8")
    req = urllib.request.Request(
        f"{base_url.rstrip('/')}/chat/completions",
        data=body,
        headers={"Content-Type": "application/json"},
    )
    try:
        with urllib.request.urlopen(req, timeout=300) as resp:
            return json.load(resp)
    except urllib.error.HTTPError as exc:
        detail = exc.read().decode("utf-8", "replace")
        hint = ""
        if exc.code == 404:
            hint = (f"\nModel '{model}' not found. Use --list-models to see what "
                    f"the endpoint serves.")
        elif exc.code == 401:
            hint = "\nThe endpoint requires an API key (use an OPENAI_API_KEY env or add auth support)."
        raise RuntimeError(
            f"API error {exc.code}: {detail}{hint}"
        ) from exc
    except urllib.error.URLError as exc:
        raise RuntimeError(
            f"cannot reach {base_url}: {exc.reason}. "
            "Is the local model server running?"
        ) from exc


def main(argv=None):
    args = parse_args(argv)
    base_url = args.base_url.rstrip("/")

    if args.list_models:
        list_models(base_url)
        return 0

    if args.path and args.image:
        print("error: pass either a positional path or --path, not both",
              file=sys.stderr)
        return 2

    path = args.path or args.image
    if args.clipboard and path:
        print("error: --clipboard cannot be combined with an image path",
              file=sys.stderr)
        return 2

    try:
        if path:
            image_bytes, mime = load_from_path(path)
        else:
            if not args.clipboard:
                print("no image path given; reading from clipboard",
                      file=sys.stderr)
            image_bytes, mime = load_from_clipboard()
    except Exception as exc:
        print(f"error: {exc}", file=sys.stderr)
        return 1

    try:
        resp = chat(base_url, args.model, args.prompt,
                    image_bytes, mime, args.max_tokens, args.temperature)
    except Exception as exc:
        print(f"error: {exc}", file=sys.stderr)
        return 1

    if args.json:
        print(json.dumps(resp, ensure_ascii=False, indent=2))
        return 0

    try:
        content = resp["choices"][0]["message"].get("content")
    except (KeyError, IndexError):
        print("error: unexpected response shape", file=sys.stderr)
        print(json.dumps(resp, ensure_ascii=False, indent=2), file=sys.stderr)
        return 1
    print(content if content else "(empty response)")
    return 0


if __name__ == "__main__":
    sys.exit(main())
