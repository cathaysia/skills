# Investigation — Phases 1–3

## Phase 1: Establish a Trustworthy Reproduction

Find the earliest point where observed behavior diverges from expected behavior.
Before touching application logic, determine whether the failure comes from the
**environment**, the **test fixture/harness**, or the **product itself**.

**Gather evidence first:**
- Inspect the exact command, test output, logs, artifacts, fixture setup, and
  relevant dependency/runtime versions.
- Inspect artifacts produced by the failing run: screenshots, recordings,
  container and service logs, application log tails, crash traces, captured
  requests or responses. Treat artifacts as evidence, not as a substitute for
  reproducing the behavior.

**Check environmental causes:**
- Missing services, resource exhaustion, stale build artifacts, concurrency,
  network availability, incompatible tool versions.
- If the environment is unstable or non-reproducible, make it deterministic
  before interpreting results. Use the lightest suitable isolation: pinned
  dependencies/toolchains, containers, or a VM for OS/service-level
  requirements. Keep setup clean and repeatable; expose setup/run/cleanup
  through the repo's `justfile`, `Makefile`, or equivalent.

**Check the fixture/harness:**
- Determine whether the fixture, driver, test data, timing assumptions, or
  assertions are themselves incorrect or overly complex.
- Do not treat a failing test as proof of a product defect. Do not change
  assertions to make the test pass without evidence that the expectation is wrong.
- If the fixture is complex, execute its setup and actions step by step until
  the scenario is understood and reproducible. Repair the fixture when it is the
  source of divergence; only then use it to investigate product behavior.

**Host-level changes are high-risk:**
Treat firewall and network settings as high-risk. Do not change them as a
routine debugging step. Prefer a container, network namespace, or VM. If a
host-level change is necessary: inspect and record current state first, limit
the change to the smallest test scope, preserve the management/network path,
prepare a rollback, do not make the rule persistent, and verify no startup
configuration restores it.

---

## Phase 2: Reduce the Problem

Choose the simplest reproduction that preserves the defect.

**Unit path (preferred when tractable):**
- If upstream and downstream dependencies can be mocked and the failure remains
  understandable, write a focused unit test and use direct code inspection,
  targeted logs, or a debugger (GDB/LLDB).

**Integration path (when mocks don't help):**
- Keep the smallest integration path that still reproduces the failure.
- For an expensive build or runtime: start the environment once, then manually
  perform the scenario one operation at a time while observing state at each
  boundary.

**Bisect to find the earliest divergence:**
- In a multi-step test: bisect the steps.
- In a multi-tier system: bisect client, service, relay, controller, database,
  and other boundaries.
- For a regression: use `git bisect`.

**Isolate code regions:**
- Temporarily stub, mock, or disable code regions to determine which region
  changes the observed behavior. Make these diagnostic edits reversible; do not
  leave them in the final change.
- For exception-heavy paths: temporarily stop translating or propagating an
  error at the suspected boundary (panic or rethrow) to expose the originating
  stack. Restore intended error handling after locating the cause.

**Run discipline:**
- Run one failing test or scenario at a time. Do not use a full-suite result to
  locate a single failure — interference, leaked state, and resource contention
  obscure the cause.
- After the individual reproducer passes, run the relevant aggregate suite to
  detect cross-test regressions, side effects, and resource leaks.

---

## Phase 3: Instrument the Boundary

Add targeted, temporary diagnostics at points that distinguish competing
hypotheses. Instrument:

- **Immediately before and after I/O or external calls:** operation,
  endpoint/resource, duration, result/status, payload size.
- **Before and after important decisions or state transitions:** selected
  branch, relevant non-sensitive state, resulting state.
- **At error translation, fallback, retry, and timeout boundaries:** original
  error and the decision taken.

**Rules:**
- Never log credentials, tokens, secrets, or unnecessary personal data.
- Prefer structured logging when the codebase provides it.
- Mark every temporary diagnostic for removal before finishing.
- Retain only diagnostics that meet the project's production logging conventions.
