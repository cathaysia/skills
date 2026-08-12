---
name: agent-mux-slave
description: Act as a worker ("slave") node in an MQTT-based multi-agent mesh. Use when you are a slave agent that must connect to the master (or a parent slave), report your status (state + plan files) so the master can coordinate, accept the master's work assignments/control messages and adjust in real time, answer async RPCs, and maintain a heartbeat so the master can detect when you drop offline.
---

# agent-mux-slave

You are a **slave** node of an agent mesh. One master coordinates many slaves
that may form a **tree** — you connect to the master (or to a parent slave) via
`parent_id`, heartbeat to stay alive, report your status, answer RPCs, and
follow the master's control messages. The master schedules work so that
different slaves avoid touching the same conflict-risk files.

Transport is MQTT; the node is a **single shared Rust MCP server**
(`agent-mux` binary, one binary for both roles — role comes from `--role` or
from `mux_init(role=...)`). The node is created **lazily**: nothing connects
until you call `mux_init` after this skill loads (or it auto-initializes in the
background when launched with `--role slave` and a session id is known).

## Startup workflow

Setup (broker, build, MCP registration, `mqtt.conf`): see
`<repo>/agent-mux/README.md`.

1. Read `references/protocol.md` for the full topic/message spec.
2. Initialize the node. Your session id comes from `$CODEX_THREAD_ID`; if it
   is unset, **ask the agent for its Codex session id** — never invent one. You
   must know the master's session id:
   - `mux_init(role="slave", parent_id=<master_sid>)` for a direct child of the
     master, or `parent_id=<parent_slave_sid>` for a deeper tree node.
   - If the master's session id is unknown, use `mux_init(role="slave")` and
     read it from the retained `master` message (the node exposes `mux_status()`).
3. Heartbeat starts automatically (background thread in the MCP process); the
   master will see `slave_joined` and your tree position.
4. If the TUI runs inside tmux, the node auto-detects its own pane and injects
   a short `[mux] ...` hint whenever the master sends a control message or an
   RPC request — you get woken instead of having to wait. On that hint, call
   `mux_pull()` to fetch the message.

> **Do not block at startup waiting for the master.** The master's messages
> are asynchronous — it only sends control when it has something to coordinate,
> and that may not happen for a while. Do your own work first (analyze, plan,
> `report_status`), and check `mux_pull()` at turn boundaries.

## Reporting status (very important)

The master coordinates by what you report. Whenever your situation changes:

- `report_status(state="planning", message="...")` — you are analyzing.
- `report_status(state="ready", plan_files=[...], message="...")` — **when you
  finish a plan, always include the concrete files you intend to modify** so the
  master can detect conflict-risk zones and schedule/order work.
- `report_status(state="working", plan_files=[...])` — you started editing.
- `report_status(state="blocked", blocked_reason="...")` — waiting on something.
- `report_status(state="done", message="...")` — finished.
- `report_conflict(files=[...], zone="...", description="...",
  severity="high", suggestion="...")` — **whenever you hit (or foresee) a
  conflict** with another slave or a master assignment, report it so the master
  learns and adjusts scheduling; the more feedback, the smarter the master.

## Receiving master messages (asynchronous — mux_pull + tmux wake)

The mesh is **asynchronous**: master control messages arrive over MQTT at any
time, not on a fixed schedule. Codex CLI cannot receive server-pushed MCP
notifications, so delivery uses two complementary channels — **never block at
startup**:

1. **tmux wake (push, when available).** If the Codex TUI runs inside tmux, the
   MCP server detects its own pane (it matches `pane_pid` against the process
   ancestor chain — codex does not export `$TMUX_PANE` to MCP servers) and, on
   a new control message or RPC request, injects a short hint into the input
   box:
   `[mux] master sent a message: call mux_pull to view and handle it.`
   React by calling `mux_pull()`. The injected text is only a hint — the actual
   message stays in the MCP queue, so nothing is lost even if the hint gets
   merged into a busy turn. If auto-detection ever fails, run
   `tmux display-message -p '#{pane_id}'` in your TUI pane and pass it as
   `tmux_pane` to `mux_init`.
2. **mux_pull (always available).** `mux_pull()` non-blockingly returns
   everything already queued for this node:
   `{"control": [...], "rpc_requests": [...], "events": [...]}`. Call it at the
   start of every turn / whenever idle, so nothing is missed even without tmux.

Workflow:

1. After `mux_init`, do your own work first: analyze, plan, and
   `report_status(state="ready", plan_files=[...])` with the concrete files you
   will modify, so the master can schedule around you.
2. At each turn boundary (or when woken by a `[mux]` hint) call `mux_pull()`:
   - control item `{"kind": "assign", "payload": {task, files, priority}}`
     -> adopt the assignment, update `report_status`.
   - `{"kind": "pause"}` / `{"kind": "resume"}` -> stop / continue work.
   - `{"kind": "replan", "payload": {...}}` -> adjust your plan (files changed).
   - `{"kind": "priority", "payload": {...}}` -> reorder your queue.
   - `rpc_requests` items -> answer with `rpc_reply(request_id, result=...,
     error=...)` (see below).
   - `events` -> mesh lifecycle events; react if relevant.
3. Only when you genuinely need to **wait** for the master's next input (rare)
   use the blocking `wait_control(timeout=...)` / `wait_rpc_requests(timeout=...)`
   **once** — they block inside the call and return on arrival or timeout. Keep
   `timeout` below the MCP tool timeout configured for the server (see the
   agent-mux README). Do not use them as a polling loop.

## Answering RPCs

You receive async RPCs (e.g. from the master or a sibling). For each request:

1. Pick up requests via `mux_pull()` (`rpc_requests` list, non-blocking) or
   `wait_rpc_requests(timeout=...)` (blocks until at least one arrives, returns
   `[]` on timeout). Each item has `request_id`, `method`, `params`.
2. Do the work (or decide to defer).
3. `rpc_reply(request_id, result=..., error=...)` — always reply so the caller's
   pending RPC completes; otherwise it times out and may be retried.

## Conflict avoidance

- Before starting edits, call `get_zone_snapshot()` / check with the master via
  `send_rpc(master_sid, "may_i_touch", {files: [...]})`.
- Never edit a file the master has assigned to another slave or that lives in a
  high-risk zone (lockfiles, generated code, `.git`, build dirs) without
  confirmation.
- If your work overlaps a zone owned by another slave, wait or request
  serialization through the master — do not race.

## Handoff

- `mux_status()` -> identity + connectivity.
- Keep the node alive for the whole session; the heartbeat is what lets the
  master notice if you drop. The MCP process also runs a watchdog: if its parent
  (codex) dies or the MQTT link stays down too long, it cleans up retained
  hb/registry and exits by itself.

See `references/protocol.md` for topic tables and exact message schemas.
