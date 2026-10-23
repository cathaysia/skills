# Fix Confirmation + Self-Review — Phase 4

## Step 1: Confirm the Fix and Clean Up

Before presenting the solution:

- Re-run the smallest reproducer first, then the relevant broader test suite to
  detect interaction or regression effects.
- Verify the original expected behavior directly when feasible — a green test
  alone is insufficient if its fixture or assertion may be wrong.
- Remove temporary logs, panic/rethrow changes, stubs, feature flags, captured
  data, and other debug-only artifacts; or lower lasting diagnostics to the
  project's structured trace/debug level when they have operational value.
- Recheck the environment after cleanup: stop containers, VMs, emulators,
  debuggers, and background processes started for the investigation, and confirm
  temporary network or firewall rules are gone.
- Report the root cause, evidence, code or fixture changed, exact verification
  performed, and any remaining uncertainty. Distinguish a confirmed product fix
  from a fixture repair, environment repair, or workaround.

---

## Step 2: Self-Review the Diff (Think Twice)

Debugging creates tunnel vision. After a fix clicks into place, the relief of
"it works now" bypasses critical thinking. Run the following four checkpoints
**after** proposing a solution but **before** finalizing it.

This review should take ~30 seconds of reasoning, not 5 minutes. The value
comes from consistently catching obvious mistakes. Do not skip it because the
fix is "small" or "obvious" — small fixes are where the worst blind spots hide.

---

### Checkpoint 1: Over-Engineering

Is the fix proportional to the actual problem?

**Red flags:**
- Defending against scenarios the codebase will never encounter
- Large abstractions for a case affecting <1% of users (a comment or known-limitation note would suffice)
- Configuration systems for values that are effectively constants
- Production-grade retry logic, circuit breakers, or rate limiters in a prototype/dev context
- Treating security hardening as a blocker when the service only runs on localhost

**How to evaluate:** Think about who uses this code and what environment it runs
in. Match the weight of the fix to the weight of the problem.

**If over-engineered:** Do not silently simplify. Present both options:
- **Option A (current):** The original, more defensive solution — what it covers and its cost.
- **Option B (simplified):** A leaner alternative — what it drops and why that's acceptable.

Ask the user which option they prefer before proceeding.

---

### Checkpoint 2: Production Safety

Would this fix be safe to deploy to production as-is?

**Red flags:**
- Hardcoding `http://` instead of deriving protocol from the environment
- Disabling authentication, CORS, or CSRF protections to "make it work"
- Logging sensitive data (tokens, passwords, PII) that was added for debugging
- Using `eval()`, `dangerouslySetInnerHTML`, or equivalent unsafe APIs without justification
- Catching and silently swallowing errors that should propagate
- `TODO: remove before production` comments as the only safeguard
- Weakening input validation to work around a test case
- `*` in CORS origins or overly permissive IAM/permissions

**How to evaluate:** Imagine this exact diff being merged and deployed with no
further review. If it wouldn't pass a security-conscious code review, either
fix it now or clearly flag it as known debt with a concrete remediation plan.

---

### Checkpoint 3: Performance Impact

Does the fix introduce unnecessary runtime cost?

**Red flags:**
- Synchronous I/O or blocking calls in a hot path
- O(n²) or worse complexity when O(n) is achievable
- Test-specific logic behind runtime checks (`if (isTestMode)` scattered through business logic)
- Middleware or hooks that run on every request but only serve the debugging scenario
- Fetching already-available data, or re-computing values that could be cached
- Loading entire modules or datasets when only a small subset is needed

**How to evaluate:** Trace the execution path of your fix during normal
operation. If it adds work that only exists to support the debug/test scenario,
it's leaking test concerns into production. Restructure so test-specific
behavior lives in test infrastructure, not in application code.

---

### Checkpoint 4: Band-Aid Fix

Does the fix solve the root cause, or merely suppress the symptom?

**Red flags:**
- Silencing warnings or errors instead of addressing their cause
  (`deprecation_warnings=False`, `@SuppressWarnings`, `# type: ignore`, `eslint-disable` without explanation)
- Bare `try/except` or `catch` that swallows the real error
- `sleep()`, retry loops, or polling as a workaround for a race condition instead of fixing synchronization
- Pinning a dependency to an old version to avoid a breaking change rather than migrating
- Flags or env vars to disable a misbehaving feature (`SKIP_VALIDATION=true`)
- `any` type casts or force-unwraps to bypass type errors
- Resetting state or clearing caches at suspicious points to "make the bug go away"
- Hand-rolling solutions the standard library or established packages already provide

**How to evaluate:** Ask: "If I removed this fix, would the underlying problem
still exist?" If yes, you've suppressed a symptom. A band-aid is sometimes
acceptable short-term, but it must be:
1. Documented with a comment explaining what the real fix would be.
2. Tracked (TODO with a ticket reference or concrete follow-up plan).
3. Flagged to the user as a workaround, not a proper fix.

---

## Output Format

After all four checkpoints, present results as:

```
### Think Twice — Self-Review

| Checkpoint        | Result                        | Notes           |
|-------------------|-------------------------------|-----------------|
| Over-Engineering  | ✅ Proportional / ⚠️ Issue    | (brief summary) |
| Production Safety | ✅ Safe / ⚠️ Risk found       | (brief summary) |
| Performance       | ✅ No impact / ⚠️ Concern     | (brief summary) |
| Band-Aid Fix      | ✅ Root cause / ⚠️ Workaround | (brief summary) |
```

- If any checkpoint flags ⚠️: add a **Remediation** section with specific,
  actionable changes. Apply them before presenting the final solution.
- If all pass ✅: do not print the table or mention the review. Proceed silently.
- If the user explicitly wants a quick-and-dirty fix: still run the review, but
  frame findings as "things to clean up later" rather than blockers.
