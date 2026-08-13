---
name: agent-mux-executor
description: Act as a worker ("executor") node in an MQTT-based multi-agent mesh. Use when you are an executor agent that must connect to the manager (or a parent executor), report your status and plan_files so the manager can coordinate parallel work and avoid conflicts, answer async RPCs, and maintain a heartbeat so the manager can detect when you drop offline.
---

# agent-mux-executor

You are an **executor** node of an agent mesh. You connect to the manager (or a
parent executor) in a tree, heartbeat to stay alive, and keep the manager
informed of your state and the files you plan to touch so it can **coordinate
parallel work and prevent collisions**. **The manager does not hand you your
work** — your work comes from the user who spawned you (or from a parent
executor); the manager coordinates that work. You do your own work, answer
async RPCs, and escalate to the manager only what needs coordination or a
human-level decision.

The node is a single Rust MCP server (`agent-mux`, one binary for both roles).
Setup (broker, build, MCP registration, `mqtt.conf`): see
`<repo>/agent-mux/README.md`.

## Start

1. Initialize the node once, passing your session id **explicitly**:
   - `mux_init(role="executor", session_id=<your Codex session id>,
     parent_id=<manager_sid>)` for a direct child of the manager, or
     `parent_id=<parent_executor_sid>` for a deeper tree node.
   - If the manager's session id is unknown, use
     `mux_init(role="executor", session_id=<your Codex session id>)` and read
     it from `mux_status()`.
   Do **not** rely on auto-init or `$CODEX_THREAD_ID`: the MCP server process
   does not inherit the interactive shell's environment, so the session id is
   frequently missing there. If you do not know your session id, **ask the
   user** — never invent one.
2. The heartbeat is automatic (background thread in the MCP process). Do **not**
   block at startup waiting for the manager: it coordinates, it does not
   dispatch. **Start your own work first**, and check `mux_pull()` at turn
   boundaries (or when a `[mux]` wake hint arrives — via MCP notify when your
   agent supports it, else tmux) so the manager's steering, zone grants and
   RPCs reach you.

## Reporting status — 4 states only

The manager coordinates by what you report. Report **only** these 4 states; no
progress ticks, no echoes, no repeated `working`:

- `report_status(state="blocked", blocked_reason=..., task_id=...)` — waiting
  on something (e.g. a zone held by another executor).
- `report_status(state="done", message=..., task_id=...)` — finished (include
  acceptance data in `message` when available).
- `report_status(state="error", message=..., task_id=...)` — failed; always
  report test/build failures to the manager — never silently expand your write
  set to fix them.
- `report_status(state="conflict", message=..., task_id=...)` — you hit (or
  foresee) a collision with another executor. Also use
  `report_conflict(files=[...], zone=..., description=..., severity=...,
  suggestion=...)` so the manager learns and adjusts.

When you **start** a piece of work, announce what it will touch with
`report_status(state="ready", plan_files=[...], message=...)` (plus
`target_crates` when relevant). The `plan_files` are the manager's primary
conflict picture — declare the concrete paths up front so overlaps are caught
before they happen, and re-declare (via `may_i_touch`) if your write set grows
mid-flight. If you receive an `assign` steering message (rare — see below),
confirm it with `report_status(state="working", plan_files=[...], message=...)`
including the task id, kind, files and target crates so the manager's task
table tracks it.

## Receiving manager messages

- `mux_pull()` non-blockingly returns everything queued for you:
  `{"control": [...], "rpc_requests": [...], "events": [...], "watch": [...]}`.
  Call it at the start of every turn / whenever idle.
- Control items are the manager **steering** your own work when coordination
  requires it: `assign` (reassign/steer — see "Assign is steering, not
  dispatching"), `pause`/`resume` -> stop/continue, `replan` -> adjust your
  plan, `priority` -> reorder your queue, `release` -> dependencies are done,
  you may run the Validate step now.
- Never block on the manager: rely on the `[mux]` wake (MCP notify when your
  agent supports it, else tmux) and `mux_pull()` at turn boundaries. Do not
  call `wait_control` / `wait_rpc_requests` to wait for input.

## Assign is steering, not dispatching

Your work normally comes from the user, not from an `assign`. If an `assign`
control message arrives, it is the manager **re-steering** existing work to
deconflict (e.g. the original owner died mid-work, or a file must move off an
overlapping executor). Adopt the reassigned file set as your current scope and
confirm with `report_status(state="working", ...)`. An `assign` carries `kind`
(`Src` | `Validate` | `Docs` | `Deps` | `Release`), `target_crates`, `files`
and a `task_id`; the server builds the dependency graph from it and may later
send `release` once dependencies clear.

## P1 / P2 discipline

- In **P1 (parallel changes)** you work on your own `Src`/`Deps` scope. If the
  manager steers a `Validate` task to you, its state is `scheduled` —
  **never run a full suite early**. Wait for the manager's `release` control
  message, which the server sends automatically once your dependencies are
  `Done`.
- In **P2 (unified validation)**, after release, run the **one full validation
  pass** the manager asks for.

## Avoiding conflicts

- Before touching any **new** file, ask with
  `send_rpc(manager_sid, "may_i_touch", {files: [...], owner: <your sid>})` and
  await the result with `get_result`. The server runs a 5-level impact check:
  exact-file collisions deny/queue, same-module/crate and dependency-neighbor
  touches escalate to the manager, global shared state (Cargo.lock / .git /
  generated dirs) is never auto-approved, and conflict-history paths escalate.
  If the result says `escalated`, wait for the manager's `approval_decide`
  before touching.
- Zones are manager-assigned: request ownership with `zone_request(path)` and
  relinquish it with `zone_request(path, release=true)` (async RPCs — await the
  returned request_id with `get_result`). You cannot lock zones yourself:
  `zone_acquire` / `zone_release` are manager-only tools.
- Never edit a file another executor claimed or in a high-risk zone
  (lockfiles, generated code, `.git`, build dirs) without confirmation.
- If your work overlaps a zone owned by another executor, wait or request
  serialization through the manager — do not race.

## Watching for events

Instead of polling, you can wait for a **manager-produced event** and be woken
when it fires:

- `mux_watch(kind="zone_released", filter={"path": "/abs/path"},
  ttl=60.0)` registers a watch; it returns a `watch_id`. `kind` names the
  event (today: `zone_released` — a zone got unlocked, including handoff to a
  queued owner). `filter` narrows it: `{"path": "/x"}` exact, `{"path_prefix":
  "/x"}` any path under it, `{}`/absent = any event of that kind. `ttl`
  (seconds) is optional.
- When a matching event fires, the manager routes it to you, your node fires
  the `[mux]` wake, and the next `mux_pull()` returns it under `watch` (with
  `watch_id`, `kind`, and the `event` payload). Then re-check
  `get_zone_snapshot()` / retry `zone_request`.
- `watch_cancel(watch_id)` drops the watch early; watches also expire after
  `ttl` and are cleaned up if you go offline.

## Answering RPCs

- Pick up requests via `mux_pull()` (`rpc_requests`, or the `[mux]` wake hint).
  Each item has `request_id`, `method`, `params`, `from`.
- Always reply with `rpc_reply(request_id, result=..., error=...)` so the
  caller's pending RPC completes; otherwise it times out and may be retried.

## Handoff

- `mux_status()` -> identity + connectivity.
- Keep the node alive for the whole session; the heartbeat is what lets the
  manager notice if you drop. The MCP process also watches its parent: if codex
  dies or the MQTT link stays down too long, it cleans up and exits by itself.
