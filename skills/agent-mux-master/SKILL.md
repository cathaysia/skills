---
name: agent-mux-master
description: Act as the single coordinator ("master") in an MQTT-based multi-agent mesh. Use when you are the master node and need to wait for slave agents to connect, discover the slave tree, list/retry pending async RPCs, coordinate slave work (git worktree isolation, conflict-risk zones, priority scheduling, serialization) and watch slave heartbeats so you can react when a slave joins, reports status, or drops offline.
---

# agent-mux-master

You are the **master** node of an agent mesh. One master coordinates many
slaves that may form a tree. Slaves connect at any time, send async RPCs,
report status, and follow your control messages. You track who is online, what
each subtree is working on, and which files are at risk of conflict.

The node is a single Rust MCP server (`agent-mux`, one binary for both roles).
Setup (broker, build, MCP registration, `mqtt.conf`): see
`<repo>/agent-mux/README.md`.

## Start

1. Initialize the node once: `mux_init(role="master")` (skip if it already
   auto-initialized via `--role master`). Your session id comes from
   `$CODEX_THREAD_ID`; if it is unset, **ask the user for the Codex session id**
   — never invent one. If init fails, check the broker is up.
2. Then **stop — do not wait.** The node is passive: when a slave joins/reports
   or a message arrives it pushes a `[mux] ... call mux_pull ...` wake hint
   (via MCP notify when your agent supports it, else into your tmux pane);
   call `mux_pull()` then to fetch the queued events. If no wake channel is
   available, call `mux_pull()` at each turn boundary instead. Never call
   `wait_events` / `wait_control` to block on input.

## Coordinating slaves

- `mux_pull()` returns the events to react to: `slave_joined`, `slave_left`,
  `status`, `ctrl_ack`, `rpc_request`, `conflict_reported` — the node wakes you
  via a `[mux]` hint (MCP notify or tmux) when they arrive. `topology()` shows
  the slave tree; `mux_status()` gives a compact summary.
- When a slave drops (`slave_left`), check `list_pending()` for RPCs targeting
  it and reassign or `retry()` them.
- Schedule work so conflicts are minimized:
  - `risk_zones()` shows paths with a conflict history; treat them as high-risk
    (lockfiles, generated code, `.git`, build dirs, root manifests) and
    serialize work on them.
  - Use `git worktree` isolation when multiple slaves touch the same repo; never
    let two slaves edit the same worktree or the same non-mergeable file at
    once.
  - Prefer assigning work to the subtree that already owns the relevant files.
  - Queue work by blocking dependencies, user priority, locality, then
    first-come; steer slaves with `send_control(target, kind, payload)`.
  - When two slaves must touch the same conflict zone, serialize them: finish
    one before the next starts, or assign the zone with `zone_acquire`.
- Zone locks are **master-only**: `zone_acquire(path, owner=...)` (fails
  `queued: true` if another owner holds it; `force` steals it),
  `zone_release(path, owner=...)` hands it to the next queued owner,
  `list_zones()` shows ownership. Slaves cannot lock zones themselves — they ask
  via the `zone_request` RPC, which the master node auto-answers against its
  registry (grant / FIFO queue / release), so no `rpc_request` for it is queued
  to you.
- Watch routing: a slave can register a watch via `{root}/watch/reg`
  (`mux_watch(kind, filter, ttl)` on its side). The master stores watches,
  matches them against the events it produces (today: `zone_released`, fired
  on `zone_release()` success), and publishes matches to
  `{root}/watch/evt/{watcher_sid}` so the watcher is woken. Watches expire
  after `ttl`; the master drops a watcher's watches when it goes offline.

## RPCs

- `send_rpc(target, method, params)` returns a request id immediately;
  `get_result()` waits for it, `list_pending()` shows outstanding ones.
- `retry(request_id)` re-publishes a pending/failed RPC (e.g. after the target
  slave reconnects); `cancel(request_id)` drops one.
- Slaves can RPC you: `mux_pull()` (or the `[mux]` wake hint) delivers their
  `rpc_request`s; answer with `rpc_reply()`.

## Learning from conflicts

- Slaves report collisions via `report_conflict(...)`, which raises
  `conflict_reported`. `list_conflicts()` shows the history; `risk_zones()`
  aggregates it into per-path risk so you can serialize hot paths.
- Reports persist to `<config_dir>/conflicts.json` and are loaded on restart,
  so coordination improves across sessions.

## Handoff

- `mux_status()` -> identity + counts.
- Keep the node alive for the whole coordination session; do not tear it down
  until the session ends.
