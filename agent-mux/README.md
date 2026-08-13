# agent-mux

MQTT-based async RPC + liveness + coordination mesh for Codex agents, as a
**single Rust MCP server binary** that runs in both roles (`manager` / `executor`).

- One binary: role comes from `--role manager|executor` (or from the `mux_init`
  MCP tool the agent calls after the skill loads).
- Async RPC over MQTT (rumqttc + tokio): executors can connect at any time and
  send RPC requests; the manager can list pending RPCs and retry them.
- Liveness via a single retained heartbeat topic (`hb/{sid}`): executors publish
  `status: "online"` periodically on a background thread so a stalled agent
  cannot block liveness; graceful shutdown publishes an `offline` flag and the
  MQTT Last Will covers abrupt loss, so the manager detects dropped executors and
  expires their work.
- Executor tree: each node reports its parent session id, so the manager sees the
  full executor tree and coordinates per-branch.
- Coordination: the manager plans work against conflict-risk zones (git
  worktrees / path zones), serializes agents when conflicts are unavoidable,
  and learns from `report_conflict` feedback.
- Watch: an executor can subscribe to a **manager-produced event** (e.g.
  `zone_released` — "this zone got unlocked") with `mux_watch(kind,
  filter)`. When the event fires, the manager routes it to the watcher and the
  watcher's wake channel (MCP notify / tmux) wakes the agent — so an executor can
  wait for another node to release a scope instead of polling `zone_request`.
- Wake channels (push): **MCP notify** is used whenever the agent declares it
  can receive server-pushed MCP notifications (always the highest priority);
  otherwise a **tmux wake** is used when the TUI runs inside tmux; if neither
  is available the node refuses to start (hard error) instead of silently
  running without notifications. `mux_pull()` at turn boundaries remains the
  always-safe poll fallback.

## Build

```sh
cd agent-mux
cargo build --release        # rust-toolchain.toml pins 1.94.0
# binary: target/release/agent-mux
```

## Broker

The bundled Mosquitto stack (no password, port 1883):

```sh
docker compose up -d    # from the agent-mux dir (agent-mux/docker-compose.yml)
```

## MCP config

Add to your Codex config (one entry per role; both point at the same binary):

```toml
[mcp_servers.agent-mux-manager]
command = "<repo>/agent-mux/target/release/agent-mux"
args = ["--role", "manager"]

[mcp_servers.agent-mux-executor]
command = "<repo>/agent-mux/target/release/agent-mux"
args = ["--role", "executor"]
```

When a session id is available in the server process environment
(`$CODEX_THREAD_ID`) the node auto-initializes right after the MCP
`initialize` handshake, so the wake channel can honor the agent's MCP-notify
support (MCP notify > tmux > hard error). Otherwise initialization is
deferred until the agent calls `mux_init` after loading the skill.

Note: `$CODEX_THREAD_ID` is read from the **server process** environment, and
Codex does not always pass it to the MCP server child process, so the manager
and executor skills pass `session_id` **explicitly** in `mux_init(...)` —
never rely on the env var being visible to the server.

## Usage

```
agent-mux [--role manager|executor] [--session-id <sid>] [--config <dir>] [--root <topic-root>]

  --role        auto-init role (default: wait for the mux_init tool)
  --session-id  codex session id (default $CODEX_THREAD_ID)
  --config      config dir holding mqtt.conf (default ~/mqtt)
  --root        mqtt topic root override (default: project dir / git root with home stripped)
```

Environment:

- `CODEX_THREAD_ID` — session id used to identify the node (never random;
  `mux_init` refuses to invent one and asks the agent instead).
- `AGENT_MUX_WAKE` — wake channel override: `mcp` | `tmux` | `none`. Default:
  MCP notify when the agent declares support (always highest priority), else
  tmux, else a hard error. `none` disables notifications.
- `AGENT_MUX_NO_TMUX=1` — disable tmux pane auto-detection (useful in
  tests/CI; an explicit `tmux_pane` override still wins).

## Configuration

`mqtt.conf` in the config dir (default `~/mqtt`) is a JSON file:

```json
{"host": "127.0.0.1", "port": 1883, "keepalive": 60,
 "hb_interval": 1.0, "hb_timeout": 3.0, "rpc_timeout": 5.0, "qos": 1,
 "wake": "mcp"}  // optional: "mcp" | "tmux" | "none"
```

The MQTT topic root defaults to the project directory (git repo root, or
the cwd) with the home prefix stripped — each project gets its own isolated
mesh. Override with `--root` (or a `root` field in `mqtt.conf`).

## Layout

- `src/main.rs` — CLI, deferred init, stdio MCP server bootstrap.
- `src/mcp.rs` — JSON-RPC/MCP tool dispatch (26 tools) + global node slot.
- `src/node.rs` — MQTT node: async RPC, heartbeat/liveness, worktree/zone
  planning, pending queue + retry.
- `src/wake.rs` — wake channel abstraction: MCP notify (highest priority),
  tmux fallback, and resolution (error when neither is available).
- `src/tmux.rs` — tmux pane detection + wake injection.
- `src/config.rs` — config loading, topic-root and session-id resolution.

See `PROTOCOL.md` for the full topic/message spec. The skills only
describe usage; exact topics/schemas are an implementation detail.
