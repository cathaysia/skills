# agent-mux protocol

MQTT-based asynchronous RPC + presence + coordination for one **master** and many
**slaves**. Slaves may be arranged in a **tree** (each slave has a `parent_id`),
so the master can coordinate per-subtree.

## Roles and topology

- **master** (single): subscribes the whole topic root, builds the slave tree,
  tracks heartbeats/presence, keeps the pending-RPC registry, owns the zone-lock
  registry, sends control messages.
- **slave** (many): registers with an optional `parent_id` (the master's session
  id, or another slave's session id) -> the mesh is a tree. Heartbeats, reports
  status, receives control messages, answers RPCs, can RPC any node (usually the
  master).
- Session id = the Codex session id (`CODEX_THREAD_ID`). **Never generate a
  random one**; if it cannot be resolved, ask the agent for its session id.

## Topic layout

Topic root = config dir with the home prefix stripped (`~/mqtt` -> `mqtt`).

| Topic | Pub | Sub | Payload (JSON) |
|---|---|---|---|
| `{root}/registry/{sid}` | every node (retained) | master | `{sid, parent_id, role, joined_at}` |
| `{root}/presence/{sid}` | every node (retained, LWT) | master | `{sid, role, status: online\|offline, ts}` |
| `{root}/hb/{sid}` | slave | master | `{sid, parent_id, role, state, ts}` |
| `{root}/status/{sid}` | slave (retained) | master | `{sid, parent_id, state, plan_files[], message, blocked_reason, ts}` |
| `{root}/ctrl/{sid}` | master | slave | `{kind, payload, from, request_id, ts}` |
| `{root}/ctrl_ack/{sid}` | slave | master | `{request_id, ok, from, ts}` |
| `{root}/rpc/req/{sid}` | any | target | `{id, method, params, reply_to, from, ts}` |
| `{root}/rpc/resp/{sid}` | any | caller | `{id, ok, result\|error, ts}` |
| `{root}/master` | master (retained) | all | `{sid, role, ts}` |
| `{root}/zones` | master (retained) | all | `{zones: {path: {owner, queued[]}}, updated}` |
| `{root}/conflict/{sid}` | any node (retained) | master | `{id, sid, files[], zone, description, severity, suggestion, ts}` |
| `{root}/conflicts` | master (retained) | all | `{conflicts: [entry...], updated}` |

## Lifecycle

1. `mux_init(role=...)` -> node connects, subscribes, announces (registry +
   presence retained), publishes `{root}/master` (master) or a heartbeat (slave).
2. Slave heartbeat loop publishes `{root}/hb/{sid}` every `hb_interval` (5 s).
3. Master sweep loop marks a slave offline if `last_seen` is older than
   `hb_timeout` (15 s). Graceful leave publishes presence `offline` (LWT covers
   abrupt loss).
4. Events: master receives `slave_joined` / `slave_left` / `status` /
   `ctrl_ack` / `rpc_request` / `conflict_reported`; slave receives
   `rpc_request` events and control messages. Codex CLI cannot receive
   server-pushed MCP notifications, so delivery uses two channels:
   - **tmux wake (push).** When a slave's TUI runs inside tmux, its MCP server
     finds its own pane by matching `pane_pid` against the process ancestor
     chain (codex does not export `$TMUX_PANE` to MCP servers); `mux_init`
     also accepts `tmux_pane` as an explicit override. On a new control
     message or RPC request it injects a short `[mux] ... call mux_pull ...`
     hint into the pane (debounced). Only a hint goes through tmux — the
     message itself stays queued in the MCP process.
   - **mux_pull (poll at turn boundaries).** `mux_pull()` non-blockingly drains
     the node's queues: `{control[], rpc_requests[], events[]}`. The agent
     calls it at turn boundaries / when idle, so nothing is missed without
     tmux. Blocking tools (`wait_events`, `wait_control`, `wait_rpc_requests`)
     remain for the rare case where the agent genuinely wants to wait.

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
  `ctrl_ack` topic automatically, queues the message, and (when tmux is
  available) injects a wake hint so the agent calls `mux_pull()`. The slave
  also calls `mux_pull()` at turn boundaries as a non-tmux-safe fallback; the
  blocking `wait_control(timeout)` remains for when the agent genuinely wants
  to wait. Either way the envelope is self-describing:
  `{"kind", "payload", "from", "request_id", "ts"}`.
- Slave -> master status: `report_status(state, plan_files, message,
  blocked_reason)`. Call with `state='ready'` and the concrete `plan_files`
  (files you intend to modify) when a plan is complete so the master can
  schedule work and avoid conflicts.

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

## Config

`~/mqtt/mqtt.conf` (JSON, auto-created) overrides defaults:

```json
{ "host": "127.0.0.1", "port": 1883, "keepalive": 60,
  "hb_interval": 5.0, "hb_timeout": 15.0, "rpc_timeout": 30.0, "qos": 1 }
```

Env vars: `CODEX_THREAD_ID` (session id), `AGENT_MUX_ROLE` (optional auto-init).

## MCP configuration (Codex)

Add to `~/.codex/config.toml` (or per-project config); each node gets its own
server process so role comes from `--role` (or from `mux_init` in-session):

```toml
[mcp_servers.agent-mux-master]
command = "python3.13"
args = ["/Users/loongtao/skills/skills/agent-mux-master/scripts/mux_mcp.py", "--role", "master"]

[mcp_servers.agent-mux-slave]
command = "python3.13"
args = ["/Users/loongtao/skills/skills/agent-mux-slave/scripts/mux_mcp.py", "--role", "slave"]
# blocking receive tools (wait_control/wait_events/wait_rpc_requests) wait
# inside the MCP call; raise the tool timeout so longer waits are allowed
tool_timeout_sec = 300
```

Requirements: Python >= 3.10, `pip install -r requirements.txt`
(`paho-mqtt>=2.0.0`, `mcp>=1.12.4`), broker from `docker-compose.yml`
(`docker compose up -d`).
