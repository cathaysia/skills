---
name: debugging
description: Diagnose failing tests and runtime defects by separating environment, fixture, and product causes, then choosing the smallest reliable reproduction and debugging method. Use when investigating test failures, unexpected behavior, crashes, or regressions.
---

# Debugging

Find the earliest point where observed behavior diverges from expected behavior. Do not change product code or weaken an assertion until the failure has been reproduced and classified.

## 1. Establish a trustworthy reproduction

Before debugging application logic, determine whether the failure comes from the environment, test fixture/harness, or product:

- Inspect the exact command, test output, logs, artifacts, fixture setup, and relevant dependency/runtime versions.
- Check for environmental causes such as missing services, resource exhaustion, stale build artifacts, concurrency, network availability, or incompatible tool versions.
- Check whether the fixture, driver, test data, timing assumptions, or assertions are themselves incorrect or overly complex.
- Reproduce the failure in isolation when possible. Compare with a known-good run or manually exercise the relevant operation to establish actual system behavior.
- Inspect artifacts produced by the failing run, including screenshots, recordings, container and service logs, application log tails, crash traces, and captured requests or responses. Treat artifacts as evidence, not as a substitute for reproducing the behavior.
- Do not treat a failing test as proof of a product defect. Do not change assertions to make the test pass without evidence that the expectation is wrong.

If the fixture is complex, first execute its setup and actions step by step until the scenario is understood and reproducible. Repair the fixture or driver when it is the source of divergence; only then use it to investigate product behavior. A test harness is part of the system under diagnosis: verify that its setup, driver actions, timing, cleanup, and assertions observe the intended product surface.

If the environment is unstable or non-reproducible, make it deterministic before interpreting results. Use the lightest suitable isolation mechanism: pinned dependencies/toolchains, containers, or a VM for OS- or service-level requirements. Keep setup clean and repeatable, and expose common setup/run/cleanup operations through the repository's `justfile`, `Makefile`, or equivalent. Avoid changing a repository's environment architecture when a local, isolated run is sufficient.

Treat host-level system configuration, especially firewall and network settings, as high-risk. Do not change them as a routine debugging step. Prefer a container, network namespace, or VM that isolates the experiment. If a host-level change is necessary, inspect and record the current state first, limit the change to the smallest test scope, preserve the agent's management/network path, and prepare a rollback before applying it. Do not make the rule persistent; ensure reboot clears it and verify that no startup configuration restores it.

## 2. Reduce the problem

Choose the simplest reproduction that preserves the defect:

- If upstream and downstream dependencies can be mocked and the failure remains understandable, write a focused unit test and use direct code inspection, targeted logs, or a debugger such as GDB/LLDB.
- If mocks do not make the behavior tractable, keep the smallest integration path that still reproduces it. For an expensive build or runtime, start the environment once and manually perform the scenario one operation at a time while observing state at each boundary.
- When the cause is ambiguous, bisect the execution sequence, inputs, component boundaries, or recent changes to find the earliest divergence. In a multi-step test, bisect the steps; in a multi-tier system, bisect client, service, relay, controller, database, and other boundaries; for a regression, use `git bisect`.
- Temporarily isolate code regions by stubbing, mocking, or disabling them to determine which region changes the observed behavior. Make these diagnostic edits reversible and do not leave them in the final change.
- For exception-heavy paths, temporarily stop translating or propagating an error at the suspected boundary (for example, panic or rethrow there) to expose the originating stack. Restore intended error handling after locating the cause.

Use one diagnostic change at a time where practical, and record what each experiment proves or rules out. A workaround that merely suppresses the symptom is not a root-cause fix.

During iteration, run one failing test or scenario at a time. Do not use a full-suite result to locate a single failure, because interference, leaked state, and resource contention can obscure the cause. After each individual reproducer passes, run the relevant aggregate suite to detect cross-test regressions, side effects, and resource leaks.

## 3. Instrument the boundary

During investigation, add targeted, temporary diagnostics at points that distinguish competing hypotheses:

- Immediately before and after I/O or external calls: operation, endpoint/resource, duration, result/status, and payload size as relevant.
- Before and after important decisions or state transitions: selected branch, relevant non-sensitive state, and resulting state.
- At error translation, fallback, retry, and timeout boundaries: original error and the decision taken.

Never log credentials, tokens, secrets, or unnecessary personal data. Prefer structured logging when the codebase provides it. Mark every temporary diagnostic for removal before finishing; retain only diagnostics that meet the project's production logging conventions.

## 4. Confirm the fix and clean up

- Re-run the smallest reproducer first, then the relevant broader test suite to detect interaction or regression effects.
- Verify the original expected behavior directly when feasible; a green test alone is insufficient if its fixture or assertion may be wrong.
- Remove temporary logs, panic/rethrow changes, stubs, feature flags, captured data, and other debug-only artifacts, or lower lasting diagnostics to the project's structured trace/debug level when they have operational value.
- Recheck the environment after cleanup: stop containers, VMs, emulators, debuggers, and background processes started for the investigation, and confirm temporary network or firewall rules are gone.
- Report the root cause, evidence, code or fixture changed, exact verification performed, and any remaining uncertainty. Distinguish a confirmed product fix from a fixture repair, environment repair, or workaround.
