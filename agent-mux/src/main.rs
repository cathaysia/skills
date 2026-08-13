//! agent-mux: MQTT-based async RPC + liveness + coordination MCP server.
//!
//! Single binary, two roles (`--role manager|executor`). Talks MCP/JSON-RPC over
//! stdio. The node is created lazily by the agent via the `mux_init` tool after
//! the manager/executor skill loads; when `--role` is given and a session id is
//! known it auto-initializes in the background.

mod config;
mod mcp;
mod node;
mod tmux;
mod wake;

use anyhow::Result;

fn print_usage() {
    eprintln!(
        "agent-mux {} — MQTT async-RPC/coordination MCP server (manager|executor)\n\
         \n\
         usage: agent-mux [--role manager|executor] [--session-id <sid>] [--config <dir>] [--root <topic-root>]\n\
         \n\
         options:\n\
         \x20 --role       auto-init role (default: wait for the mux_init tool)\n\
         \x20 --session-id codex session id (default $CODEX_THREAD_ID)\n\
         \x20 --config     config dir holding mqtt.conf (default ~/mqtt)\n\
         \x20 --root       mqtt topic root override (default: project dir / git root with home stripped)\n\
         \x20 --help       show this help\n\
         \n\
         The agent calls mux_init(role=..., session_id=<its session id>) after the skill\n\
         loads; initialization is deferred until then when no session id is available.",
        env!("CARGO_PKG_VERSION")
    );
}

#[tokio::main]
async fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut role: Option<String> = None;
    let mut session_id: Option<String> = None;
    let mut config_dir: Option<String> = None;
    let mut root: Option<String> = None;

    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--role" => {
                i += 1;
                role = args.get(i).cloned();
            }
            "--session-id" => {
                i += 1;
                session_id = args.get(i).cloned();
            }
            "--config" | "--config-dir" => {
                i += 1;
                config_dir = args.get(i).cloned();
            }
            "--root" => {
                i += 1;
                root = args.get(i).cloned();
            }
            "--help" | "-h" => {
                print_usage();
                return Ok(());
            }
            other => {
                eprintln!("agent-mux: unknown argument: {other}");
                print_usage();
            }
        }
        i += 1;
    }

    if let Some(r) = &role {
        if r.as_str() != "manager" && r.as_str() != "executor" {
            eprintln!("agent-mux: invalid --role {r:?} (expected manager|executor)");
            std::process::exit(2);
        }
    }

    let config_dir = config_dir.unwrap_or_else(|| config::DEFAULT_CONFIG_DIR.to_string());
    let conf = config::load_config(&config_dir, root.as_deref())?;
    config::set_default_root(&conf.root);

    // Auto-init is deferred until the MCP `initialize` handshake completes
    // (mcp::run), so the wake channel can honor the client's MCP-notify
    // support: MCP notify > tmux > hard error.
    let auto = if let Some(r) = &role {
        match config::resolve_session_id(session_id.as_deref()) {
            Ok(sid) => Some(mcp::AutoInit {
                role: r.clone(),
                sid,
                config_dir: config_dir.clone(),
                root: conf.root.clone(),
                conf: conf.clone(),
            }),
            Err(e) => {
                eprintln!("agent-mux: {e}");
                eprintln!(
                    "agent-mux: deferring init -- call mux_init(role=..., \
                     session_id=<your Codex session id>) after the skill loads"
                );
                None
            }
        }
    } else {
        None
    };

    mcp::run(None, auto).await;

    Ok(())
}
