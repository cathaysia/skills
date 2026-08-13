# agent-mux protocol

MQTT-based asynchronous RPC + liveness + coordination for one **manager** and many
**executors**. Executors may be arranged in a **tree** (each executor has a `parent_id`),
so the manager can coordinate per-subtree.

## Roles and topology

- **manager** (single): subscribes the whole topic root, builds the executor tree,
  tracks heartbeats/liveness, keeps the pending-RPC registry, owns the zone-lock
  registry, sends control messages.
- **executor** (many): registers with an optional `parent_id` (the manager's session
  id, or another executor's session id) -> the mesh is a tree. Heartbeats, reports
  status, receives control messages, answers RPCs, can RPC any node (usually the
  manager).
- Session id = the Codex session id (`CODEX_THREAD_ID`). **Never generate a
  random one**; if it cannot be resolved, ask the agent for its session id.

## Topic layout

Topic root = project directory (git repo root / cwd) with the home prefix stripped; override with `--root`.

| Topic | Pub | Sub | Payload (JSON) |
|---|---|---|---|
| `{root}/registry/{sid}` | every node (retained) | manager | `{sid, parent_id, role, joined_at}` |
| `{root}/hb/{sid}` | executor (retained, LWT) | manager | `{sid, parent_id, role, state, status: online\|offline, reason?, ts}` |
| `{root}/status/{sid}` | executor (retained) | manager | `{sid, parent_id, state, plan_files[], message, blocked_reason, ts}` |
| `{root}/ctrl/{sid}` | manager | executor | `{kind, payload, from, request_id, ts}` |
| `{root}/ctrl_ack/{sid}` | executor | manager | `{request_id, ok, from, ts}` |
| `{root}/rpc/req/{sid}` | any | target | `{id, method, params, reply_to, from, ts}` |
| `{root}/rpc/resp/{sid}` | any | caller | `{id, ok, result\|error, ts}` |
| `{root}/manager` | manager (retained) | all | `{sid, role, ts}` |
| `{root}/zones` | manager (retained) | all | `{zones: {path: {owner, queued[]}}, updated}` |
| `{root}/conflict/{sid}` | any node (retained) | manager | `{id, sid, files[], zone, description, severity, suggestion, ts}` |
| `{root}/conflicts` | manager (retained) | all | `{conflicts: [entry...], updated}` |
| `{root}/watch/reg` | executor | manager | `{watch_id, watcher_sid, kind, filter, ttl?, ts}` or `{watch_id, watcher_sid, cancel: true, ts}` |
| `{root}/watch/evt/{sid}` | manager | watcher executor | `{watch_id, kind, event, ts}` |

## Lifecycle

1. `mux_init(role=...)` -> node connects, subscribes, announces (registry
   retained + hb retained with `status: "online"`), publishes `{root}/manager`
   (manager) or a heartbeat (executor).
2. Executor heartbeat loop publishes a retained `{root}/hb/{sid}` every
   `hb_interval`; it carries `status: "online"` and the current `state`.
3. Liveness is a single retained hb topic. Leave is explicit: graceful shutdown
   publishes `status: "offline", reason: "shutdown"` on `{root}/hb/{sid}`, and
   abrupt loss is covered by the MQTT Last Will (`reason: "connection_lost"`).
   The manager emits `executor_left` immediately on either flag; the sweep loop only
   catches silent stalls — it marks an executor offline when `last_seen` is older
   than `hb_timeout`.
4. Events: manager receives `executor_joined` / `executor_left` / `status` /
   `ctrl_ack` / `rpc_request` / `conflict_reported`; executor receives
   `rpc_request` events and control messages. Delivery uses a wake channel
   plus a poll, with a fixed priority: **MCP notify > tmux > hard error**.
   - **MCP notify (push, highest priority).** Any agent that declares it can
     receive server-pushed MCP notifications (custom `initialize` capability:
     `capabilities.notify` / `mcpNotify` / `notifications` /
     `agentMuxNotify`, or `experimental.agentMuxNotify`) gets an id-less
     `notifications/message` written to the MCP stdout whenever a control
     message, RPC request, or executor join/status event arrives (debounced).
     MCP notify always wins over tmux when declared — it cannot be demoted.
   - **tmux wake (push, fallback).** When the agent does not declare MCP-notify
     support but its TUI runs inside tmux (manager **and** executor), the MCP
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
  result arrives asynchronously. The manager keeps a **pending registry**
  (`list_pending`, `retry`, `cancel`).
- On the receiver: if a registered handler exists it runs in-process (built-in
  `ping`); otherwise the request is **queued** and the agent answers via
  `wait_rpc_requests()` + `rpc_reply(request_id, result, error)`.
- `retry` re-publishes the same `request_id` (e.g. after the target executor
  reconnects or a handler was missing).

## Control / status

- Manager -> executor coordination messages: `send_control(target, kind, payload)`
  (e.g. `pause`, `replan`, `priority`). Delivery is **asynchronous**: the manager
  publishes whenever it needs to coordinate; it does not send on a schedule.
- Executor delivery: on a control message the executor's node ACKs on the manager's
  `ctrl_ack` topic automatically, queues the message, and injects a wake hint
  (MCP notify when the agent supports it, else tmux) so the agent calls
  `mux_pull()`. The executor also calls `mux_pull()` at turn boundaries as a
  push-free fallback; the blocking `wait_control(timeout)` remains for when
  the agent genuinely wants to wait. Either way the envelope is
  self-describing: `{"kind", "payload", "from", "request_id", "ts"}`.
- Executor -> manager status: `report_status(state, plan_files, message,
  blocked_reason)`. Call with `state='ready'` and the concrete `plan_files`
  (files you intend to modify) when a plan is complete so the manager can
  schedule work and avoid conflicts.

## Watch (event subscription)

An executor can **watch a manager-produced event** (e.g. "this zone got unlocked")
and be **woken when it fires**, instead of polling `zone_request` /
`get_zone_snapshot`. The watcher never needs to know *which* other node owns
the resource — it only names the event it cares about (`kind` + `filter`).

### Registration

`mux_watch(kind, filter?, ttl?)` publishes `{root}/watch/reg`:

```json
{"watch_id": "<uuid>", "watcher_sid": "<executor sid>", "kind": "zone_released",
 "filter": {"path": "/abs/path"}, "ttl": 60.0, "ts": 123.0}
```

- `kind` — which manager-produced event to watch. Today: `zone_released`
  (fired when `zone_release()` succeeds, including handoff to a queued owner).
- `filter` — partial match on the event payload. `{"path": "/x"}` matches only
  that exact path; `{"path_prefix": "/x"}` matches any path under it; `{}` /
  absent matches every event of that kind.
- `ttl` — optional lifetime in seconds; the manager drops the watch after
  `ttl` even if no event fired (`0` / absent = no expiry).

### Delivery and wake

On every matched event the manager publishes `{root}/watch/evt/{watcher_sid}`:

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
  the manager removes the watch.
- Watches expire after `ttl` (swept by the manager).
- When a watcher goes offline (hb `offline` / registry cleared /
  heartbeat-timeout), the manager drops all of its watches.

## Conflict feedback (manager learning)

Any node reports a conflict (or a conflict risk) with `report_conflict(files,
zone, description, severity, suggestion)`:

- The entry is published retained on `{root}/conflict/{sid}` and the manager
  records it, persists it to `<config_dir>/conflicts.json` (loaded again on
  manager restart, so the manager **learns across sessions**), and re-publishes
  the full list retained on `{root}/conflicts`.
- The manager emits a `conflict_reported` event to its own event queue.
- `risk_zones()` aggregates the conflict history into per-path counts/severity;
  the manager treats high-count paths as conflict-risk zones and serializes work
  on them (the more conflicts reported, the smarter the scheduling).

## Zone locks

Manager-owned registry of `path -> {owner, queued[]}` published retained on
`{root}/zones`. **Only the manager locks zones** — an executor never declares
ownership:

- `zone_acquire(path, owner?, force?)` (manager-only) acquires/assigns a zone and
  fails with `queued: true` when another owner holds it (FIFO queue); `owner`
  assigns the zone to an executor; `force` steals it from the current owner.
- `zone_release(path, owner?)` (manager-only) releases a zone and hands it to the
  next queued owner.
- Executors request via `zone_request(path, release?)`, which sends an async RPC
  `zone_request` to `{root}/rpc/req/{manager_sid}`. The manager node answers
  against its registry: grants when free, returns `queued: true` when held, or
  (with `release: true`, requester owns the zone) releases to the next queued
  owner. The response arrives on the requester's pending-RPC result.

Executors observe the retained snapshot via `get_zone_snapshot()` / `list_zones()`
and can `mux_watch` the `zone_released` event instead of polling.

## Setup / configuration

Broker, build, MCP registration and `mqtt.conf` live in
`<repo>/agent-mux/README.md` (one shared Rust binary serves both roles via
`--role manager|executor`). Requirements: Rust toolchain (1.94.0 per
`rust-toolchain.toml`), broker from `agent-mux/docker-compose.yml`
(`docker compose up -d`).
