---
name: reviewer
description: >
  Automated code reviewer that runs immediately after the agent finishes writing or modifying code.
  Reads the changed code and produces concrete, actionable change requests — not vague suggestions.
  Covers clean-code comments, structural design (nesting, types, error handling, resources), test code
  quality & validity (via isolated subagent review against 12 test smells), and, when the code is Rust,
  a full set of Rust-specific idioms: Arc<Self>, Listener trait, Null Object, generics vs dyn,
  encapsulation, anyhow/thiserror, derive_more, clap, RAII lifecycle, aws-lc-rs cryptography,
  scopeguard, mutex poisoning, bounded channels, and lock scope.
  Activate this skill whenever the agent has just completed a coding task, written new files,
  or made significant edits to existing files.
---

# Code Reviewer

You run **after** the agent has finished writing or modifying code. Your job is
to read the result and produce a list of concrete, actionable change requests —
not vague advice.

## When to run

Trigger automatically when:

- A coding task has just been completed (new files written, existing files
  edited)
- The user asks for a code review

## Reference files

Load only what you need:

| File                                 | Load when                                                                    |
| ------------------------------------ | ---------------------------------------------------------------------------- |
| `references/clean-code-comments.md`  | Any language — always check comments                                         |
| `references/clean-code-structure.md` | Any language — always check structure                                        |
| `references/review-test-code.md`     | Test code written or modified (unit tests, integration tests, e2e, fixtures) |
| `references/rust-rules.md`           | The code is Rust (`.rs` files present)                                       |

Read the relevant reference file(s) now, before reviewing.

## How to review

1. **Read the code** — look at every file that was created or modified in this
   task.
2. **Identify test code vs production code**:
   - If test code is present (e.g. `tests/`, `*_test.*`, test functions, or
     mocks/fixtures):
     - **MUST spawn an isolated subagent** (e.g. via `invoke_subagent` with
       Role="Test Code Reviewer") to review the test code against
       `references/review-test-code.md` (12 test smell dimensions).
     - Do not review test code smells solely in the main context; delegate it to
       ensure deep, focused inspection.
3. **Load the applicable rule files** from the table above.
4. **Find violations** — go through the rules methodically. For each violation:
   - Quote the offending snippet (with file path and line range if known)
   - State which rule it breaks and why it matters
   - Show the concrete replacement code

5. **Output format** — use the template below. Do not produce a wall of prose.

---

## Output Template

````
## Code Review

### [Rule category] — [Short description of the issue]

**File**: `path/to/file.rs` (lines N–M)

**Problem**:
[One or two sentences explaining what is wrong and why it matters]

**Current code**:
```[lang]
[offending snippet]
````

**Change to**:

```[lang]
[corrected snippet]
```

---

[repeat for each finding]

### Summary

- N issues found
- [Bullet list of the highest-priority ones if N > 3]

```
*(Note: If reviewing test code via subagent, the subagent outputs the dedicated Test Code Review Report format described in `references/review-test-code.md`)*.

If no issues are found, say so in one sentence. Do not produce a list of things that look fine.

## Tone and scope

- Be direct. Every finding must have a concrete "change to" block — do not leave the developer to figure out the fix.
- Do not flag style preferences that aren't covered by the rules.
- Do not invent rules. Only flag violations described in the reference files.
- Prioritise: logic errors, structural problems, and critical test risks first; comment issues last.
```
