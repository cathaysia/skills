---
name: long-task-management
description: >-
  Operating discipline for long-running, multi-step tasks: keep the original
  goal in view, keep context small, and never solve the same problem twice. Use
  when (1) starting work that will span many tool calls, many files, or hours —
  large refactors, migrations, feature builds, batch operations, long debugging
  sessions; (2) the conversation is growing long or context is being compacted;
  (3) you notice you are deep in a detail, unsure how the current step serves
  the original request, or stuck in a loop; (4) you need to decide whether to
  delegate to a subagent, keep a todo list, or write a note or script. Triggers:
  long task, context overflow, task drift, lost the goal, repeated mistakes,
  hitting the same error again, running out of context.
---

# Long Task Management

Long tasks rarely fail because one step was hard. They fail in three
predictable ways:

- **Context overflow** — file dumps and dead ends crowd out the goal; details
  you no longer need displace the ones you do.
- **Goal drift** — the current subtask quietly becomes the new objective, and
  the original request stops being served.
- **Repeated pits** — you re-debug something already solved, or re-run a
  command already known to fail in this environment.

Everything below exists to prevent one of those three. None of it is
ceremony: if a rule is not preventing drift, overflow, or repetition on your
current task, drop it.

## The loop

1. **Anchor** — restate the goal and its acceptance criteria, then write the
   todo list.
2. **Delegate** — push exploration and mechanical work to subagents; keep the
   conclusions, not the dumps.
3. **Record** — problems become notes, then scripts; decide commit vs. local
   exclude.
4. **Re-anchor** — before each new step, re-read the goal and the list.
5. **Verify** — run the real check at each milestone, and do a final
   requirement sweep at the end.

## 1. Todo list: the anti-drift anchor

Write the list before the first edit, not after the work is discovered.

- Each item is a **verifiable outcome**, not an activity: "`cargo test` passes"
  or "unknown ids return 404", never "work on tests". Outcomes can be checked;
  activities cannot, so they hide drift.
- Keep exactly one item `in_progress`. Two in-progress items means the goal has
  already split.
- Work discovered mid-task goes **onto the list**, not silently into the
  current step. Hidden scope growth is drift in its early stage.
- **Re-read the original request and the list before starting each new item.**
  If the item no longer obviously serves the request, either the item or the
  list is wrong — fix that before continuing.

### Sub-todos: investigation without drift

Some items need open-ended investigation (find where X is handled, figure out
why Y fails). Do that with **sub-todos scoped to the parent item**, not by
starting a new plan.

- A sub-todo answers one question, then **ends** with a recorded decision: an
  action item added to the main list, or "not needed, because …".
- A sub-investigation that never ends is drift with extra steps. If it grows
  its own branches, stop and check the parent: does the parent still serve the
  original request?
- When the sub-todos close, return to the main list before doing anything else.
  The investigation produced an answer, not a new goal.

## 2. Subagents: context isolation, not just speed

The main reason to delegate is that the subagent reads the twenty files and
the file dumps, and you keep the conclusion. That is what keeps the main
context small enough to hold the goal.

- **Delegate**: broad searches ("where is X used"), per-file mechanical work,
  independent chunks that can run in parallel.
- **Do not delegate**: the final acceptance test, anything the user will see,
  and decisions that require the whole picture. You cannot vouch for what you
  did not look at.
- Make each subagent prompt **self-contained**: the exact goal, the inputs,
  the deliverable format, and the boundaries.

  ```text
  Task: find every place that constructs a ClickHouse client.
  Report: file:line, enclosing function, whether the client is reused.
  Read-only. No edits, no refactors, no file dumps — conclusions only.
  ```

- Launch independent subagents in one message so they run concurrently.
- **Trust but verify.** A subagent's summary is a claim. Spot-check the
  load-bearing part — the one file, line, or result the next step depends on.

## 3. Record what you learn; script what repeats

A problem solved once and not recorded will be solved again next week. When
you get past something non-obvious, capture **symptom → root cause → fix →
exact command**. If the sequence will recur, turn it into a script next to
the note.

Then decide where the artifact belongs — a real decision, not a formality:

- **Commit it** when a teammate cloning this repo would need it: build quirks,
  workflow scripts, invariants, gotchas. It belongs in `docs/`, `scripts/`,
  the README, or `AGENTS.md`.
- **Keep it local** when it is machine- or run-specific: personal paths,
  scratch debug scripts, probe output, secrets-adjacent notes. Write the file
  wherever is convenient, then add its path to `.git/info/exclude` so it
  never pollutes `git status`:

  ```bash
  printf '%s\n' '/scratch/' >> .git/info/exclude
  git check-ignore -v scratch/probe.rs   # confirm the exclusion is active
  ```

  `.git/info/exclude` is a local file that is never committed and does not
  affect other clones — exactly right for knowledge that is true only on your
  machine. (Patterns that should be ignored for *everyone* — build output,
  editor directories — belong in the committed `.gitignore` instead.)
- **Discard it** if it is noise. A note nobody would ever read again is
  clutter with a paper trail.

The test in one line: *would a fresh clone be worse off without it?* Yes →
commit. No → `.git/info/exclude`.

Never commit secrets. If a note contains a token, a key, or a credential, it
is a local-only file by definition.

## 4. Protect the context budget

- Read narrowly: search first, then read the regions you need. Do not re-read
  what you already read, and do not re-dump large files to "refresh".
- Ask subagents for conclusions, not raw content.
- On tasks spanning many turns, keep a short task log: decisions made, dead
  ends, invariants, the current plan. After a compaction, re-read the log and
  the todo list **before** touching anything — that state is what survived,
  not the conversation.

## 5. Environment and resource exhaustion

When a failure does not match the code — cryptic linker errors, timeouts, OOM
kills, "No space left on device" — suspect the environment before rewriting
code that works.

- Check: `df -h .` for free space, `du -sh <dir>` for what consumed it.
- **Rust**: if disk is insufficient, run `cargo clean` first; `target/`
  routinely grows to tens of GB. That deletes the whole build cache, so a full
  rebuild follows — free space now, rebuild time later. For a narrower reset,
  `cargo clean -p <crate>` clears one package's artifacts, and deleting
  `target/debug/incremental` clears only incremental state.
- A build interrupted by a full disk can leave corrupt artifacts. After
  freeing space, rebuild from clean rather than trusting the partial state.

## Ending a long task

Before declaring done:

1. Re-read the original request line by line. Every explicit requirement maps
   to a delivered item, and every delivered item is verified by a real check —
   not by inspection.
2. Name anything found along the way that was **not** done, instead of leaving
   it implied.
3. Confirm `git status --short` shows only intended changes: scratch artifacts
   went to `.git/info/exclude`, reusable knowledge went into the commit.
