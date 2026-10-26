---
name: code-notes
description: >-
  Write a concise, reusable technical note after solving a system- or
  framework-level problem that took more than 30 minutes to resolve. Do NOT
  activate for routine tasks, project-progress updates, or issue tracking.
---

# Code Notes

## When to Activate

Activate this skill when **both** conditions are true:

1. The problem is **system- or framework-level** — it involves OS behaviour,
   compiler/runtime internals, a library's non-obvious mechanism, a protocol
   constraint, or similar infrastructure that sits below application logic.
2. Resolving it took **more than 30 minutes** of investigation.

Do **not** activate for:

- Routine implementation steps.
- Pure application-logic bugs with an obvious cause.
- Project progress updates ("feature X is now done").
- Decision logs or product research.

---

## Note Structure

Every note must follow this order. Omit a section only if it is genuinely not
applicable — never add filler.

```
# <Title: one concise noun phrase, no trailing period>

## Why

One or two sentences: the root cause, mechanism, or constraint that makes this
non-obvious. No project names, no file paths.

## How

The general method or technique. Write at the level of a recipe, not a
diff. Use pseudocode or shell commands when they clarify the method.

## Environment  (omit if not environment-sensitive)

The minimal system context required to reproduce the issue: OS, distro,
kernel version, compositor, runtime version, or other infrastructure
components that are directly relevant. Omit anything that does not affect
reproducibility.

## Minimal Example  (omit if no script is needed)

A self-contained script or command sequence that demonstrates the technique.
Wrap in a <details> block if it exceeds ~15 lines.

## Limitations

Bullet list of known constraints, environment requirements, or edge cases where
this approach breaks down. Every note must have at least one limitation or
explicitly state "None known."
```

---

## Writing Rules

### Write knowledge, not history

- **Record the mechanism**, not the journey. A note should answer "how does this
  work?" — not "what did we try?"
- **No project names, file paths, or symbol names** that are tightly coupled to
  one codebase. The moment a detail becomes obsolete when the file is renamed,
  cut it.
- **No abandoned alternatives.** If a method was not adopted, it does not belong
  in the note.

### Stay at the right level of abstraction

- Describe _what to do_ and _why it works_, not every parameter of every call.
- A code sample must be **minimal and reproducible** — strip all
  project-specific scaffolding.
- Prefer shell commands and pseudocode over full program listings.

### Meaningful scripts and reproductions

When a script or command is the primary artifact of the solution:

- Include it verbatim in the note, inside a `<details>` block if long.
- The script must run standalone — no undeclared dependencies, no project
  imports.
- Add a one-line comment at the top explaining what it demonstrates.

---

## Anti-Patterns (never do these)

| Anti-pattern                                        | Why it is harmful                                              |
| --------------------------------------------------- | -------------------------------------------------------------- |
| Recording source file paths                         | Paths change; paths are implementation detail, not knowledge   |
| Listing dead-ends and ruled-out hypotheses          | Wastes the reader's attention; the note is not an issue thread |
| "We tried X but it didn't work"                     | Unpromoted alternatives have no reuse value                    |
| "Confirmed by screenshot / manual test"             | The note asserts truth; proofs are not needed                  |
| Project-progress sentences ("X is now implemented") | Progress belongs in issues or changelogs, not knowledge notes  |
| Vague titles ("Fix for the bug")                    | Titles must be searchable and self-describing                  |

---

## Output

Write the note as a Markdown file. Choose the location based on context:

- If there is an obvious notes or docs directory in the workspace, write there.
- Otherwise write to `notes/<topic>.md` relative to the workspace root.
- Ask the user for the target location only when it is genuinely ambiguous.

After writing, report only:

1. The file path.
2. Any open limitation that the user should be aware of.

Do not summarise the note contents back to the user.

---

## Quick Example

**Prompt**: "We just figured out how to add a drop shadow to a borderless
window."

**Good note**:

`````markdown
# Drop shadow on borderless windows

## Why

Compositor-managed decorations are stripped when a window removes its title bar.
The shadow is part of the decoration, so it disappears along with it.

## How

Re-request the shadow via the compositor protocol directly from the client. On
KWin / Wayland, set the `_KDE_NET_WM_SHADOW` property after window creation.

## Minimal Example

**Environment**: KDE Plasma 6, KWin compositor, Wayland session, PyQt6 6.7+

<details>
<summary>shadow.py — attach KWin shadow to a borderless Qt window</summary>

````python
# Demonstrates attaching a KWin drop-shadow to a frameless window on Wayland.
from PyQt6.QtWidgets import QApplication, QWidget
from PyQt6.QtCore import Qt

app = QApplication([])
w = QWidget()
w.setWindowFlags(Qt.WindowType.FramelessWindowHint)
# … set _KDE_NET_WM_SHADOW via xcb or KWindowEffects …
w.show()
app.exec()
\```

</details>

## Limitations

- Works on KWin (Wayland) only; no equivalent API on GNOME/Mutter or X11
  without a compositor extension.
- The shadow style (blur radius, colour) is controlled by the compositor theme
  and cannot be customised from the client side.
````
`````

**Bad note** (do not write like this):

- Mentions "see `src/ui/main_window.py` line 42" — path-coupled, will rot.
- Has a section "What we tried first" — ruled-out history, not knowledge.
- Ends with "confirmed working via screenshot" — unnecessary proof.
