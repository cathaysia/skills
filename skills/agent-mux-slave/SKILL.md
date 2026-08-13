---
name: agent-mux-slave
description: Act as a worker ("slave") node in an MQTT-based multi-agent mesh. Use when you are a slave agent that must connect to the master (or a parent slave), report your status (state + plan files) so the master can coordinate, accept the master's work assignments/control messages and adjust in real time, answer async RPCs, and maintain a heartbeat so the master can detect when you drop offline.
---

# agent-mux-slave

You are a **slave** node of an agent mesh. You connect to the master (or a
parent slave) in a tree, heartbeat to stay alive, report status so the master
can schedule work, answer async RPCs, and follow the master's control
messages. The server computes scheduling; you confirm assignments, do your
work, and only escalate what needs a human-level decision.

The node is a single Rust MCP server (`agent-mux`, one binary for both roles).
Setup (broker, build, MCP registration, `mqtt.conf`): see
`<repo>/agent-mux/README.md`.

## Start

1. Initialize the node once:
   - `mux_init(role="slave", parent_id=<master_sid>)` for a direct child of the
     master, or `parent_id=<parent_slave_sid>` for a deeper tree node.
   - If the master's session id is unknown, use `mux_init(role="slave")` and
     read it from `mux_status()`.
   Your session id comes from `$CODEX_THREAD_ID`; if it is unset, **ask the
   user for the Codex session id** — never invent one.
2. The heartbeat is automatic (background thread in the MCP process). Do **not**
   block at startup waiting for the master: it only sends control when it has
   something to coordinate. Do your own work first, and check `mux_pull()` at
   turn boundaries (or when a `[mux]` wake hint arrives — via MCP notify when
   your agent supports it, else tmux).

## Reporting status — 4 states only

The master coordinates by what you report. Report **only** these 4 states; no
progress ticks, no echoes, no repeated `working`:

- `report_status(state="blocked", blocked_reason=..., task_id=...)` — waiting
  on something.
- `report_status(state="done", message=..., task_id=...)` — finished (include
  acceptance data in `message` when available).
- `report_status(state="error", message=..., task_id=...)` — failed; always
  report test/build failures to the master — never silently expand your write
  set to fix them.
- `report_status(state="conflict", message=..., task_id=...)` — you hit (or
  foresee) a collision with another slave. Also use
  `report_conflict(files=[...], zone=..., description=..., severity=...,
  suggestion=...)` so the master learns and adjusts.

When you receive an `assign`, confirm it with
`report_status(state="working", plan_files=[...], message=...)` **including
the task id, kind, files and target crates** so the master's task table tracks
your progress.

## Receiving master messages

- `mux_pull()` non-blockingly returns everything queued for you:
  `{"control": [...], "rpc_requests": [...], "events": [...], "watch": [...]}`.
  Call it at the start of every turn / whenever idle.
- React to control items: `assign` -> adopt the task and confirm it (see
  above); `pause`/`resume` -> stop/continue; `replan` -> adjust your plan;
  `priority` -> reorder your queue; `release` -> dependencies are done, you may
  start the Validate task now.
- Never block on the master: rely on the `[mux]` wake (MCP notify when your
  agent supports it, else tmux) and `mux_pull()` at turn boundaries. Do not
  call `wait_control` / `wait_rpc_requests` to wait for input.

## P1 / P2 discipline

- In **P1 (parallel changes)** you work only on your assigned `Src`/`Deps`
  task. If you are assigned a `Validate` task, its state is `scheduled` —
  **never run a full suite early**. Wait for the master's `release` control
  message, which the server sends automatically once your dependencies are
  `Done`.
- In **P2 (unified validation)**, after release, run the **one full validation
  pass** the master asks for.

## Avoiding conflicts

- Before touching any **new** file, ask with
  `send_rpc(master_sid, "may_i_touch", {files: [...], owner: <your sid>})` and
  await the result with `get_result`. The server runs a 5-level impact check:
  exact-file collisions deny/queue, same-module/crate and dependency-neighbor
  touches escalate to the master, global shared state (Cargo.lock / .git /
  generated dirs) is never auto-approved, and conflict-history paths escalate.
  If the result says `escalated`, wait for the master's `approval_decide`
  before touching.
- Zones are master-assigned: request ownership with `zone_request(path)` and
  relinquish it with `zone_request(path, release=true)` (async RPCs — await the
  returned request_id with `get_result`). You cannot lock zones yourself:
  `zone_acquire` / `zone_release` are master-only tools.
- Never edit a file the master assigned to another slave or in a high-risk zone
  (lockfiles, generated code, `.git`, build dirs) without confirmation.
- If your work overlaps a zone owned by another slave, wait or request
  serialization through the master — do not race.

## Watching for events

Instead of polling, you can wait for a **master-produced event** and be woken
when it fires:

- `mux_watch(kind="zone_released", filter={"path": "/abs/path"},
  ttl=60.0)` registers a watch; it returns a `watch_id`. `kind` names the
  event (today: `zone_released` — a zone got unlocked, including handoff to a
  queued owner). `filter` narrows it: `{"path": "/x"}` exact, `{"path_prefix":
  "/x"}` any path under it, `{}`/absent = any event of that kind. `ttl`
  (seconds) is optional.
- When a matching event fires, the master routes it to you, your node fires
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
  master notice if you drop. The MCP process also watches its parent: if codex
  dies or the MQTT link stays down too long, it cleans up and exits by itself.
