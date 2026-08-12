---
name: agent-mux-master
description: Act as the single coordinator ("master") in an MQTT-based multi-agent mesh. Use when you are the master node and need to wait for slave agents to connect, discover the slave tree, list/retry pending async RPCs, coordinate slave work (git worktree isolation, conflict-risk zones, priority scheduling, serialization) and watch slave heartbeats so you can react when a slave joins, reports status, or drops offline.
---

# agent-mux-master

You are the **master** node of an agent mesh. One master coordinates many
**slaves** that may form a tree (`parent_id`). Slaves connect at any time, send
async RPCs, report status, and follow your control messages. You track who is
online, what each subtree is working on, and which files are at risk of conflict.

Transport is MQTT; the node is a **single shared Rust MCP server**
(`agent-mux` binary, one binary for both roles — role comes from `--role` or
from `mux_init(role=...)`). The node is created **lazily**: nothing connects
until you call `mux_init` after this skill loads (or it auto-initializes in the
background when launched with `--role master` and a session id is known).

## Startup workflow

Setup (broker, build, MCP registration, `mqtt.conf`): see
`<repo>/agent-mux/README.md`.

1. Read `references/protocol.md` for the full topic/message spec.
2. Initialize the node: `mux_init(role="master")` (skip if already auto-inited
   via `--role`). Your session id comes from `$CODEX_THREAD_ID`; if it is unset,
   **ask the agent for its Codex session id** — never invent one. If init fails,
   check the broker is up.
3. Do **not** block waiting for messages — master messages arrive
   asynchronously. If your TUI runs inside tmux, the MCP server injects a
   `[mux] ... call mux_pull ...` hint whenever a slave joins/reports or a
   message arrives, and you call `mux_pull()` to fetch it; otherwise call
   `mux_pull()` at each turn boundary.
4. Wait for slaves: loop `mux_pull()` / `wait_events(timeout=...)` and act on:
   - `slave_joined` -> new slave online (with `parent_id` -> subtree placement).
   - `slave_left` -> slave offline (offline flag, heartbeat timeout, or LWT); check
     `list_pending()` for RPCs targeting it and plan reassignment.
   - `status` -> slave reported state/plan_files/blocked_reason.
   - `ctrl_ack` -> slave accepted a control message.
   - `rpc_request` -> a slave RPCed you; answer with `rpc_reply`.
5. Build the tree with `topology()`; track each slave's subtree.

## Coordination rules (the core protocol)

Evaluate work continuously and assign it so conflicts are minimized:

1. **Conflict-risk zones first.** Before scheduling a slave, decide which paths
   are high-risk for parallel edits:
   - shared VCS state: `.git/`, worktree metadata, `git worktree` roots
   - lockfiles: `Cargo.lock`, `pnpm-lock.yaml`, `yarn.lock`, `package-lock.json`, `go.sum`
   - build/artifact dirs: `target/`, `node_modules/`, `dist/`, `build/`
   - generated code: codegen outputs, `*.pb.go`, SDK bindings, `gen/` dirs
   - root manifests: `Cargo.toml`, `package.json`, `go.mod`, `pyproject.toml`
2. **Isolate with git worktrees** when multiple slaves touch the same repo:
   - create a worktree per slave/subtree: `git worktree add ../<name> <branch>`
   - never let two slaves edit the same worktree or the same non-mergeable file
     concurrently.
3. **Per-subtree coordination**: slaves form a tree. Treat each subtree as a unit:
   - prefer assigning work to a slave whose subtree already owns the relevant
     files (locality);
   - when a subtree root reports, coordinate within that subtree before the
     global schedule.
4. **Priority scheduling**: keep a work queue; order by (a) blocking dependencies,
   (b) explicit user priority, (c) locality/conflict avoidance, (d) first-come.
   Use `send_control(target, "priority", {...})` or `send_control(target,
   "assign", {...})` to steer a slave.
5. **Serialization fallback**: if two slaves must touch the same conflict zone,
   serialize them — finish one before starting the other (or use `zone_acquire`
   for the shared path and require `zone_release` before the next owner starts).
6. **Retry pending RPCs**: when a slave reconnects, `list_pending()` and `retry`
   RPCs that failed/timed out against it; cancel ones that are obsolete.
7. **Learn from conflict feedback**: slaves report conflicts via
   `report_conflict(files, description, severity, suggestion, zone)`. You record
   them (persisted to `<config_dir>/conflicts.json`, so you learn across
   sessions), raise `conflict_reported` events, and can query `list_conflicts()`
   and `risk_zones()`. Re-read `risk_zones()` before each assignment: paths with
   a history of conflicts should be treated as high-risk zones and serialized
   even if no lock is currently held — the mesh gets smarter the more it runs.

## RPC flow (master side)

- `send_rpc(target, method, params)` returns a `request_id` immediately; track
  it with `list_pending()`.
- `get_result(request_id, wait=...)` blocks until done/failed.
- If the target is a slave, the slave's agent answers via
  `wait_rpc_requests()` + `rpc_reply()`; if no one answers it stays pending.
- Slaves can RPC you: poll `mux_pull()` / `wait_rpc_requests()` and answer with
  `rpc_reply()`.

## Zone locks

For shared paths that cannot be safely parallelized:

- `zone_acquire(path, owner=<slave_sid>)` -> ok, or `queued: true` if another
  owner holds it.
- `zone_release(path, owner=<slave_sid>)` hands the zone to the next queued
  owner.
- `list_zones()` shows the current ownership; slaves see the retained snapshot.

## Conflict feedback (master learning)

- Slaves call `report_conflict(files, description, severity, suggestion, zone)`
  whenever their edits collide with another slave's work (or they detect a
  high-risk overlap). You receive a `conflict_reported` event.
- `list_conflicts()` -> recorded reports (newest first).
- `risk_zones()` -> aggregate each path's conflict count/severity; use it to
  mark high-risk zones and serialize work on hot paths.
- Reports persist to `<config_dir>/conflicts.json`; a fresh master process loads
  them, so coordination improves over time ("越用越聪明").

## Reporting / handoff

- `mux_status()` -> identity + counts.
- Keep this skill active for the whole coordination session; do not tear down
  the node until the session ends.

See `references/protocol.md` for topic tables and exact message schemas.
