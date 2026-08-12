//! Wake channel abstraction.
//!
//! A *wake* is a push hint that tells the agent "new mesh messages arrived:
//! call `mux_pull()`". Business data never travels through a wake channel —
//! only the hint; the messages stay queued in the MCP process.
//!
//! Two channels exist:
//!
//! - **MCP notify** (highest priority): a JSON-RPC 2.0 id-less notification
//!   (`notifications/message`) written to the MCP stdout. Used when the client
//!   declares it can receive server-pushed notifications (custom capability in
//!   `initialize`), or when the operator forces it via `AGENT_MUX_WAKE=mcp` /
//!   `wake: "mcp"` in `mqtt.conf` / `mux_init(wake="mcp")`.
//! - **tmux wake**: `tmux send-keys` injection into the codex TUI pane.
//!
//! Resolution priority (unless explicitly overridden): MCP notify > tmux >
//! hard error, so an agent that supports neither channel cannot silently run
//! without notifications.

use crate::tmux::TmuxWake;
use serde_json::{json, Value};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::io::{AsyncWriteExt, BufWriter, Stdout};

/// Hint text shared by every channel.
pub const WAKE_TEXT: &str = "[mux] new message arrived: call mux_pull to view and handle it.";

const DEBOUNCE: Duration = Duration::from_millis(1500);

/// A push channel that can wake an idle agent.
pub trait Wake: Send + Sync {
    /// Short channel name for identity/logs (`"mcp"` | `"tmux"`).
    fn name(&self) -> &'static str;
    /// Fire a (debounced) wake hint. Never blocks the caller's task.
    fn wake(&self);
}

impl Wake for TmuxWake {
    fn name(&self) -> &'static str {
        "tmux"
    }
    fn wake(&self) {
        TmuxWake::wake(self);
    }
}

// ---------------------------------------------------------------------------
// serialized stdout writer
// ---------------------------------------------------------------------------

/// Serialized writer for the MCP stdout channel. Every write takes the mutex
/// for the whole line (payload + newline + flush), so background wake
/// notifications can never interleave with JSON-RPC responses.
#[derive(Clone)]
pub struct Out {
    inner: Arc<tokio::sync::Mutex<BufWriter<Stdout>>>,
}

impl Out {
    pub fn new() -> Self {
        Out {
            inner: Arc::new(tokio::sync::Mutex::new(BufWriter::new(tokio::io::stdout()))),
        }
    }

    /// The process-global stdout writer (created once by the MCP server loop).
    /// Responses and wake notifications share it so writes stay serialized.
    pub fn global() -> Out {
        static OUT: OnceLock<Out> = OnceLock::new();
        OUT.get_or_init(Out::new).clone()
    }

    /// Write one JSON-RPC line atomically (payload + newline + flush).
    pub async fn write_line(&self, line: &str) -> std::io::Result<()> {
        let mut w = self.inner.lock().await;
        w.write_all(line.as_bytes()).await?;
        w.write_all(b"\n").await?;
        w.flush().await
    }

    /// Best-effort JSON-RPC write (used by the server loop and wake alike).
    pub async fn write_json(&self, v: &Value) -> std::io::Result<()> {
        let line = serde_json::to_string(v).unwrap_or_else(|_| "{}".to_string());
        self.write_line(&line).await
    }

    /// Send a server -> client `notifications/message` log notification.
    pub async fn notify_message(&self, level: &str, data: Value) {
        let msg = json!({
            "jsonrpc": "2.0",
            "method": "notifications/message",
            "params": {"level": level, "data": data},
        });
        let _ = self.write_json(&msg).await;
    }
}

// ---------------------------------------------------------------------------
// MCP notify channel
// ---------------------------------------------------------------------------

pub struct McpNotify {
    out: Out,
    last: AtomicU64,
}

impl McpNotify {
    pub fn new(out: Out) -> Self {
        McpNotify {
            out,
            last: AtomicU64::new(0),
        }
    }
}

impl Wake for McpNotify {
    fn name(&self) -> &'static str {
        "mcp"
    }

    fn wake(&self) {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);
        let last = self.last.load(Ordering::Relaxed);
        if now.saturating_sub(last) < DEBOUNCE.as_millis() as u64 {
            return;
        }
        self.last.store(now, Ordering::Relaxed);
        let out = self.out.clone();
        tokio::spawn(async move {
            out.notify_message("info", json!({"mux": WAKE_TEXT, "ts": now_ts()}))
                .await;
        });
    }
}

// ---------------------------------------------------------------------------
// channel resolution
// ---------------------------------------------------------------------------

/// Explicit wake-channel preference (`mux_init(wake=...)` arg wins over
/// `AGENT_MUX_WAKE` env, which wins over `mqtt.conf` `wake` field).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WakePreference {
    Mcp,
    Tmux,
    None,
}

pub fn parse_wake_preference(s: &str) -> Result<WakePreference, String> {
    match s.trim().to_ascii_lowercase().as_str() {
        "mcp" | "mcp_notify" | "mcp-notify" => Ok(WakePreference::Mcp),
        "tmux" => Ok(WakePreference::Tmux),
        "none" | "off" | "disabled" => Ok(WakePreference::None),
        other => Err(format!(
            "invalid wake channel {other:?} (expected \"mcp\" | \"tmux\" | \"none\")"
        )),
    }
}

fn env_preference() -> Result<Option<WakePreference>, String> {
    match std::env::var_os("AGENT_MUX_WAKE") {
        None => Ok(None),
        Some(v) => {
            let s = v.to_string_lossy().to_string();
            if s.trim().is_empty() {
                Ok(None)
            } else {
                parse_wake_preference(&s).map(Some)
            }
        }
    }
}

/// Resolve the wake channel for a node.
///
/// Priority rules:
/// 1. **MCP notify always wins when the client declared support** (it is the
///    highest-priority channel and beats tmux even if tmux is present).
/// 2. Otherwise an explicit preference (`mux_init` arg > `AGENT_MUX_WAKE` env >
///    `mqtt.conf` `wake`) selects the channel.
/// 3. Otherwise tmux, when a pane is detected.
/// 4. Otherwise a hard error: the agent supports neither channel.
///
/// The only way to disable notifications is an explicit `wake=none`.
pub fn resolve(
    arg_pref: Option<WakePreference>,
    config_wake: Option<&str>,
    client_supports_notify: bool,
    tmux_pane: Option<String>,
) -> Result<Option<Arc<dyn Wake>>, String> {
    let pref = match arg_pref {
        Some(p) => Some(p),
        None => match env_preference()? {
            Some(p) => Some(p),
            None => match config_wake {
                Some(s) => Some(parse_wake_preference(s)?),
                None => None,
            },
        },
    };

    if pref == Some(WakePreference::None) {
        return Ok(None);
    }
    if client_supports_notify {
        if let Some(other) = pref {
            eprintln!(
                "agent-mux: warning: wake={other:?} requested but the client declared \
                 MCP-notify support; using MCP notify (always the highest-priority channel)"
            );
        }
        return Ok(Some(Arc::new(McpNotify::new(Out::global()))));
    }
    match pref {
        Some(WakePreference::None) => Ok(None), // unreachable: handled above
        Some(WakePreference::Mcp) => {
            eprintln!(
                "agent-mux: warning: wake=mcp forced but the client did not declare \
                 MCP-notify support; notifications may be ignored"
            );
            Ok(Some(Arc::new(McpNotify::new(Out::global()))))
        }
        Some(WakePreference::Tmux) => match TmuxWake::detect(tmux_pane) {
            Some(pane) => Ok(Some(Arc::new(TmuxWake::new(pane)))),
            None => Err(
                "wake=tmux requested but no tmux pane was detected (AGENT_MUX_NO_TMUX is set, \
                 or the TUI is not inside tmux)"
                    .to_string(),
            ),
        },
        None => match TmuxWake::detect(tmux_pane) {
            Some(pane) => Ok(Some(Arc::new(TmuxWake::new(pane)))),
            None => Err(
                "no wake channel available: this agent does not declare MCP-notify support \
                 (initialize capabilities.notify / capabilities.agentMuxNotify) and no tmux \
                 pane was detected. Run inside tmux, enable MCP notify in your agent, or set \
                 AGENT_MUX_WAKE=none to explicitly disable notifications"
                    .to_string(),
            ),
        },
    }
}

fn now_ts() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Pin the ambient environment so resolution is deterministic: tmux
    /// auto-detection is disabled (explicit pane overrides still work) and any
    /// AGENT_MUX_WAKE override is cleared.
    fn pin_env() {
        unsafe {
            std::env::set_var("AGENT_MUX_NO_TMUX", "1");
            std::env::set_var("AGENT_MUX_WAKE", "");
        }
    }

    fn wake_name(w: Option<Arc<dyn Wake>>) -> Option<&'static str> {
        w.map(|w| w.name())
    }

    #[test]
    fn resolution_priority_mcp_over_tmux_over_error() {
        pin_env();

        // Agent declares MCP notify -> mcp, even though tmux is unavailable.
        assert_eq!(wake_name(resolve(None, None, true, None).unwrap()), Some("mcp"));

        // MCP notify is always the highest-priority channel: it beats an
        // explicit wake=tmux preference.
        assert_eq!(
            wake_name(resolve(Some(WakePreference::Tmux), None, true, None).unwrap()),
            Some("mcp")
        );
        assert_eq!(
            wake_name(resolve(Some(WakePreference::Mcp), None, true, None).unwrap()),
            Some("mcp")
        );

        // No MCP notify + tmux present -> tmux.
        assert_eq!(
            wake_name(resolve(None, None, false, Some("%1".to_string())).unwrap()),
            Some("tmux")
        );

        // No MCP notify + no tmux -> hard error (requirement: error directly).
        assert!(resolve(None, None, false, None).is_err());

        // wake=none disables notifications even when MCP notify is supported.
        assert!(resolve(Some(WakePreference::None), None, true, None)
            .unwrap()
            .is_none());

        // wake=mcp forced without client support still resolves to mcp.
        assert_eq!(
            wake_name(resolve(Some(WakePreference::Mcp), None, false, None).unwrap()),
            Some("mcp")
        );

        // wake=tmux forced without a tmux pane -> hard error.
        assert!(resolve(Some(WakePreference::Tmux), None, false, None).is_err());
    }

    #[test]
    fn parse_preferences() {
        assert_eq!(parse_wake_preference("mcp").unwrap(), WakePreference::Mcp);
        assert_eq!(
            parse_wake_preference("mcp_notify").unwrap(),
            WakePreference::Mcp
        );
        assert_eq!(parse_wake_preference("tmux").unwrap(), WakePreference::Tmux);
        assert_eq!(parse_wake_preference("none").unwrap(), WakePreference::None);
        assert_eq!(
            parse_wake_preference("off").unwrap(),
            WakePreference::None
        );
        assert!(parse_wake_preference("carrier-pigeon").is_err());
    }
}
