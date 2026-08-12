# agent-mux

MQTT-based async RPC + liveness + coordination mesh for Codex agents, as a
**single Rust MCP server binary** that runs in both roles (`master` / `slave`).

- One binary: role comes from `--role master|slave` (or from the `mux_init`
  MCP tool the agent calls after the skill loads).
- Async RPC over MQTT (rumqttc + tokio): slaves can connect at any time and
  send RPC requests; the master can list pending RPCs and retry them.
- Liveness via a single retained heartbeat topic (`hb/{sid}`): slaves publish
  `status: "online"` periodically on a background thread so a stalled agent
  cannot block liveness; graceful shutdown publishes an `offline` flag and the
  MQTT Last Will covers abrupt loss, so the master detects dropped slaves and
  expires their work.
- Slave tree: each node reports its parent session id, so the master sees the
  full slave tree and coordinates per-branch.
- Coordination: the master plans work against conflict-risk zones (git
  worktrees / path zones), serializes agents when conflicts are unavoidable,
  and learns from `report_conflict` feedback.
- tmux wake: notifications are pushed into a tmux pane (when available) so an
  idle agent wakes up and pulls pending work, instead of blocking on a poll.

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
[mcp_servers.agent-mux-master]
command = "<repo>/agent-mux/target/release/agent-mux"
args = ["--role", "master"]

[mcp_servers.agent-mux-slave]
command = "<repo>/agent-mux/target/release/agent-mux"
args = ["--role", "slave"]
```

When a session id is available (`$CODEX_THREAD_ID`) the node auto-initializes
in the background. Otherwise initialization is deferred until the agent calls
`mux_init` after loading the skill.

## Usage

```
agent-mux [--role master|slave] [--session-id <sid>] [--config <dir>] [--root <topic-root>]

  --role        auto-init role (default: wait for the mux_init tool)
  --session-id  codex session id (default $CODEX_THREAD_ID)
  --config      config dir holding mqtt.conf (default ~/mqtt)
  --root        mqtt topic root override (default: config dir with home stripped)
```

Environment:

- `CODEX_THREAD_ID` — session id used to identify the node (never random;
  `mux_init` refuses to invent one and asks the agent instead).
- `AGENT_MUX_NO_TMUX=1` — disable tmux wake injection (useful in tests/CI).

## Configuration

`mqtt.conf` in the config dir (default `~/mqtt`) is a JSON file:

```json
{"host": "127.0.0.1", "port": 1883, "keepalive": 60,
 "hb_interval": 1.0, "hb_timeout": 3.0, "rpc_timeout": 5.0, "qos": 1}
```

The MQTT topic root is the config dir path with the home prefix stripped
(`~/mqtt` -> `mqtt`), overridable with `--root`.

## Layout

- `src/main.rs` — CLI, deferred init, stdio MCP server bootstrap.
- `src/mcp.rs` — JSON-RPC/MCP tool dispatch (23 tools) + global node slot.
- `src/node.rs` — MQTT node: async RPC, heartbeat/liveness, worktree/zone
  planning, pending queue + retry.
- `src/tmux.rs` — tmux pane detection + wake injection.
- `src/config.rs` — config loading, topic-root and session-id resolution.

See `PROTOCOL.md` for the full topic/message spec. The skills only
describe usage; exact topics/schemas are an implementation detail.
