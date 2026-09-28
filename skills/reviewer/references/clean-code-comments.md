# Clean Code: Comment Rules

## What Good Comments Look Like

Comments should be rare and meaningful. Acceptable comments include:

- **Legal information** placed at the top of a file (copyright headers, licenses)
- **Informational comments** that explain a return value or non-obvious behavior — though a better function name is always preferred
- **Intent explanations** that describe *why* a decision was made, not *what* the code does
- **Clarifications of obscure code** caused by external, immutable libraries (e.g., a C FFI binding that behaves unexpectedly)
- **Consequence warnings** — e.g., "Do not call this from a hot path; it acquires a global lock"
- **TODO comments** that are tracked and actionable

## What Bad Comments Look Like

Flag and request removal of:

- **Redundant comments**: restating what the code already clearly says
- **Misleading comments**: comments that no longer match the code they describe
- **Ritual comments**: commenting every method/field by convention, not necessity
- **Changelog comments**: git history exists; `# Added 2024-01-01 by Alice` has no place in source
- **Signature comments**: `# Author: Bob` — git blame is the right tool
- **Separator comments**: long lines of `===` or `---` used to visually divide sections of a file
- **Commented-out code**: dead code must be deleted, not preserved with `//`. Git remembers it
- **HTML comments** embedded in source code
- **Noise comments**: `# Default constructor` above a default constructor

## Review Actions

When you see bad comments, produce a concrete change: show the comment to delete and the (possibly renamed) code that replaces it.
