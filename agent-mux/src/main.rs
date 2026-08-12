//! agent-mux: MQTT-based async RPC + presence + coordination MCP server.
//!
//! Single binary, two roles (`--role master|slave`). Talks MCP/JSON-RPC over
//! stdio. The node is created lazily by the agent via the `mux_init` tool after
//! the master/slave skill loads; when `--role` is given and a session id is
//! known it auto-initializes in the background.

mod config;
mod mcp;
mod node;
mod tmux;

use anyhow::Result;

fn print_usage() {
    eprintln!(
        "agent-mux {} — MQTT async-RPC/coordination MCP server (master|slave)\n\
         \n\
         usage: agent-mux [--role master|slave] [--session-id <sid>] [--config <dir>] [--root <topic-root>]\n\
         \n\
         options:\n\
         \x20 --role       auto-init role (default: wait for the mux_init tool)\n\
         \x20 --session-id codex session id (default $CODEX_THREAD_ID)\n\
         \x20 --config     config dir holding mqtt.conf (default ~/mqtt)\n\
         \x20 --root       mqtt topic root override (default: config dir with home stripped)\n\
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
        if r.as_str() != "master" && r.as_str() != "slave" {
            eprintln!("agent-mux: invalid --role {r:?} (expected master|slave)");
            std::process::exit(2);
        }
    }

    let config_dir = config_dir.unwrap_or_else(|| config::DEFAULT_CONFIG_DIR.to_string());
    let conf = config::load_config(&config_dir, root.as_deref())?;

    let initial: Option<std::sync::Arc<node::Node>> = if let Some(r) = &role {
        match config::resolve_session_id(session_id.as_deref()) {
            Ok(sid) => {
                // Auto-init in the background so the stdio server starts
                // responding immediately; the agent can still call mux_init
                // later (which replaces this node).
                let r = r.clone();
                let sid = sid.clone();
                let cfg_dir = config_dir.clone();
                let conf = conf.clone();
                let root = conf.root.clone();
                tokio::spawn(async move {
                    let wake = tmux::TmuxWake::detect(None)
                        .map(|pane| std::sync::Arc::new(tmux::TmuxWake::new(pane)));
                    match node::Node::start(&r, &sid, None, None, &root, &cfg_dir, &conf, wake).await {
                        Ok(n) => {
                            let should_stop = {
                                let mut g = mcp::global_node().lock().unwrap();
                                if g.is_none() {
                                    *g = Some(n.clone());
                                    false
                                } else {
                                    true
                                }
                            };
                            if should_stop {
                                // A same-identity node (from mux_init) is already
                                // live. Refresh its retained registry/presence so
                                // the correct parent_id wins, then exit quietly
                                // without clearing our shared retained presence.
                                let same_sid = {
                                    let g = mcp::global_node().lock().unwrap();
                                    g.as_ref().map(|n| n.sid == sid).unwrap_or(false)
                                };
                                if same_sid {
                                    let existing = {
                                        let g = mcp::global_node().lock().unwrap();
                                        g.clone()
                                    };
                                    if let Some(existing) = existing {
                                        existing.reannounce().await;
                                    }
                                    n.stop_with(false).await;
                                } else {
                                    n.stop().await;
                                }
                            } else {
                                eprintln!("agent-mux: auto-initialized as {r} ({sid})");
                            }
                        }
                        Err(e) => eprintln!(
                            "agent-mux: auto-init failed: {e}; call mux_init after the skill loads"
                        ),
                    }
                });
                None
            }
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

    mcp::run(initial).await;
    Ok(())
}
