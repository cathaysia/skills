# agent-mux protocol

MQTT-based asynchronous RPC + liveness + coordination for one **master** and many
**slaves**. Slaves may be arranged in a **tree** (each slave has a `parent_id`),
so the master can coordinate per-subtree.

## Roles and topology

- **master** (single): subscribes the whole topic root, builds the slave tree,
  tracks heartbeats/liveness, keeps the pending-RPC registry, owns the zone-lock
  registry, sends control messages.
- **slave** (many): registers with an optional `parent_id` (the master's session
  id, or another slave's session id) -> the mesh is a tree. Heartbeats, reports
  status, receives control messages, answers RPCs, can RPC any node (usually the
  master).
- Session id = the Codex session id (`CODEX_THREAD_ID`). **Never generate a
  random one**; if it cannot be resolved, ask the agent for its session id.

## Topic layout

Topic root = project directory (git repo root / cwd) with the home prefix stripped; override with `--root`.

| Topic | Pub | Sub | Payload (JSON) |
|---|---|---|---|
| `{root}/registry/{sid}` | every node (retained) | master | `{sid, parent_id, role, joined_at}` |
| `{root}/hb/{sid}` | slave (retained, LWT) | master | `{sid, parent_id, role, state, status: online\|offline, reason?, ts}` |
| `{root}/status/{sid}` | slave (retained) | master | `{sid, parent_id, state, plan_files[], message, blocked_reason, ts}` |
| `{root}/ctrl/{sid}` | master | slave | `{kind, payload, from, request_id, ts}` |
| `{root}/ctrl_ack/{sid}` | slave | master | `{request_id, ok, from, ts}` |
| `{root}/rpc/req/{sid}` | any | target | `{id, method, params, reply_to, from, ts}` |
| `{root}/rpc/resp/{sid}` | any | caller | `{id, ok, result\|error, ts}` |
| `{root}/master` | master (retained) | all | `{sid, role, ts}` |
| `{root}/zones` | master (retained) | all | `{zones: {path: {owner, queued[]}}, updated}` |
| `{root}/conflict/{sid}` | any node (retained) | master | `{id, sid, files[], zone, description, severity, suggestion, ts}` |
| `{root}/conflicts` | master (retained) | all | `{conflicts: [entry...], updated}` |
| `{root}/watch/reg` | slave | master | `{watch_id, watcher_sid, kind, filter, ttl?, ts}` or `{watch_id, watcher_sid, cancel: true, ts}` |
| `{root}/watch/evt/{sid}` | master | watcher slave | `{watch_id, kind, event, ts}` |

## Lifecycle

1. `mux_init(role=...)` -> node connects, subscribes, announces (registry
   retained + hb retained with `status: "online"`), publishes `{root}/master`
   (master) or a heartbeat (slave).
2. Slave heartbeat loop publishes a retained `{root}/hb/{sid}` every
   `hb_interval`; it carries `status: "online"` and the current `state`.
3. Liveness is a single retained hb topic. Leave is explicit: graceful shutdown
   publishes `status: "offline", reason: "shutdown"` on `{root}/hb/{sid}`, and
   abrupt loss is covered by the MQTT Last Will (`reason: "connection_lost"`).
   The master emits `slave_left` immediately on either flag; the sweep loop only
   catches silent stalls — it marks a slave offline when `last_seen` is older
   than `hb_timeout`.
4. Events: master receives `slave_joined` / `slave_left` / `status` /
   `ctrl_ack` / `rpc_request` / `conflict_reported`; slave receives
   `rpc_request` events and control messages. Delivery uses a wake channel
   plus a poll, with a fixed priority: **MCP notify > tmux > hard error**.
   - **MCP notify (push, highest priority).** Any agent that declares it can
     receive server-pushed MCP notifications (custom `initialize` capability:
     `capabilities.notify` / `mcpNotify` / `notifications` /
     `agentMuxNotify`, or `experimental.agentMuxNotify`) gets an id-less
     `notifications/message` written to the MCP stdout whenever a control
     message, RPC request, or slave join/status event arrives (debounced).
     MCP notify always wins over tmux when declared — it cannot be demoted.
   - **tmux wake (push, fallback).** When the agent does not declare MCP-notify
     support but its TUI runs inside tmux (master **and** slave), the MCP
     server finds its own pane by matching `pane_pid` against the process
     ancestor chain (codex does not export `$TMUX_PANE` to MCP servers);
     `mux_init` also accepts `tmux_pane` as an explicit override. It injects a
     short `[mux] ... call mux_pull ...` hint into the pane (debounced). Only a
     hint goes through tmux — the message itself stays queued in the MCP
     process.
   - **Hard error.** If the agent supports neither MCP notify nor tmux, node
     startup fails with an explicit error (auto-init exits non-zero;
     `mux_init` returns an error) instead of silently running without
     notifications. `wake=none` / `AGENT_MUX_WAKE=none` disables notifications
     explicitly.
   - **mux_pull (poll at turn boundaries).** `mux_pull()` non-blockingly drains
     the node's queues: `{control[], rpc_requests[], events[], watch[]}`. The agent
     calls it at turn boundaries / when idle, so nothing is missed without a
     push channel. Blocking tools (`wait_events`, `wait_control`,
     `wait_rpc_requests`) remain for the rare case where the agent genuinely
     wants to wait.

## Async RPC

- `send_rpc(target, method, params)` returns a `request_id` immediately; the
  result arrives asynchronously. The master keeps a **pending registry**
  (`list_pending`, `retry`, `cancel`).
- On the receiver: if a registered handler exists it runs in-process (built-in
  `ping`); otherwise the request is **queued** and the agent answers via
  `wait_rpc_requests()` + `rpc_reply(request_id, result, error)`.
- `retry` re-publishes the same `request_id` (e.g. after the target slave
  reconnects or a handler was missing).

## Control / status

- Master -> slave coordination messages: `send_control(target, kind, payload)`
  (e.g. `pause`, `replan`, `priority`). Delivery is **asynchronous**: the master
  publishes whenever it needs to coordinate; it does not send on a schedule.
- Slave delivery: on a control message the slave's node ACKs on the master's
  `ctrl_ack` topic automatically, queues the message, and injects a wake hint
  (MCP notify when the agent supports it, else tmux) so the agent calls
  `mux_pull()`. The slave also calls `mux_pull()` at turn boundaries as a
  push-free fallback; the blocking `wait_control(timeout)` remains for when
  the agent genuinely wants to wait. Either way the envelope is
  self-describing: `{"kind", "payload", "from", "request_id", "ts"}`.
- Slave -> master status: `report_status(state, plan_files, message,
  blocked_reason)`. Call with `state='ready'` and the concrete `plan_files`
  (files you intend to modify) when a plan is complete so the master can
  schedule work and avoid conflicts.

## Watch (event subscription)

A slave can **watch a master-produced event** (e.g. "this zone got unlocked")
and be **woken when it fires**, instead of polling `zone_acquire` /
`get_zone_snapshot`. The watcher never needs to know *which* other node owns
the resource — it only names the event it cares about (`kind` + `filter`).

### Registration

`mux_watch(kind, filter?, ttl?)` publishes `{root}/watch/reg`:

```json
{"watch_id": "<uuid>", "watcher_sid": "<slave sid>", "kind": "zone_released",
 "filter": {"path": "/abs/path"}, "ttl": 60.0, "ts": 123.0}
```

- `kind` — which master-produced event to watch. Today: `zone_released`
  (fired when `zone_release()` succeeds, including handoff to a queued owner).
- `filter` — partial match on the event payload. `{"path": "/x"}` matches only
  that exact path; `{"path_prefix": "/x"}` matches any path under it; `{}` /
  absent matches every event of that kind.
- `ttl` — optional lifetime in seconds; the master drops the watch after
  `ttl` even if no event fired (`0` / absent = no expiry).

### Delivery and wake

On every matched event the master publishes `{root}/watch/evt/{watcher_sid}`:

```json
{"watch_id": "<uuid>", "kind": "zone_released",
 "event": {"path": "/abs/path", "next_owner": null, "ts": 123.0}, "ts": 123.0}
```

The watcher's node queues the event and fires its **wake channel** (MCP notify
or tmux), so the agent is woken out of an idle turn instead of polling.
`mux_pull()` returns it under the `watch` array alongside `control` /
`rpc_requests` / `events`.

### Cancellation and cleanup

- `watch_cancel(watch_id)` publishes `{root}/watch/reg` with `cancel: true`;
  the master removes the watch.
- Watches expire after `ttl` (swept by the master).
- When a watcher goes offline (hb `offline` / registry cleared /
  heartbeat-timeout), the master drops all of its watches.

## Conflict feedback (master learning)

Any node reports a conflict (or a conflict risk) with `report_conflict(files,
zone, description, severity, suggestion)`:

- The entry is published retained on `{root}/conflict/{sid}` and the master
  records it, persists it to `<config_dir>/conflicts.json` (loaded again on
  master restart, so the master **learns across sessions**), and re-publishes
  the full list retained on `{root}/conflicts`.
- The master emits a `conflict_reported` event to its own event queue.
- `risk_zones()` aggregates the conflict history into per-path counts/severity;
  the master treats high-count paths as conflict-risk zones and serializes work
  on them (the more conflicts reported, the smarter the scheduling).

## Zone locks

Master-owned registry of `path -> {owner, queued[]}` published retained on
`{root}/zones`. `zone_acquire(path)` fails with `queued: true` when another
owner holds it (FIFO queue); `zone_release(path, owner)` hands over to the next
queued owner. Slaves observe the retained snapshot via `get_zone_snapshot()`.

## Setup / configuration

Broker, build, MCP registration and `mqtt.conf` live in
`<repo>/agent-mux/README.md` (one shared Rust binary serves both roles via
`--role master|slave`). Requirements: Rust toolchain (1.94.0 per
`rust-toolchain.toml`), broker from `agent-mux/docker-compose.yml`
(`docker compose up -d`).
