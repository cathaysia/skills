---
name: debug-methodology
description: >-
  End-to-end debugging methodology: investigate root causes, then self-review
  proposed fixes. Use when diagnosing test failures, crashes, unexpected
  behavior, or regressions, and whenever proposing any code fix or patch.
---

# Debugging Methodology

## When to Activate

- **Investigating** a failure, crash, regression, or unexpected behavior →
  follow all four phases in order.
- **Proposing a fix** without a prior investigation → run Phase 4 at minimum.

Do not weaken assertions or change product code before the failure is reproduced
and classified.

## The Four Phases

| Phase             | Goal                                                 | Entry condition                |
| ----------------- | ---------------------------------------------------- | ------------------------------ |
| **1. Reproduce**  | Establish a trustworthy, isolated reproduction       | Before any code change         |
| **2. Reduce**     | Find the smallest path that still shows the defect   | After confirming reproduction  |
| **3. Instrument** | Add targeted diagnostics at hypothesis boundaries    | During active investigation    |
| **4. Deliver**    | Confirm the fix, clean up, then self-review the diff | Before presenting the solution |

→ Investigation detail (Phases 1–3): [investigation.md](investigation.md) → Fix
confirmation + self-review (Phase 4): [fix-review.md](fix-review.md)

## Non-Negotiables

- One diagnostic change at a time. Record what each experiment proves or rules
  out.
- A workaround that suppresses the symptom is not a root-cause fix.
- Never log credentials, tokens, or PII in temporary diagnostics.
- The self-review in Phase 4 is mandatory — run it even when the fix feels
  obvious.
  </Parameter>
  </invoke>
