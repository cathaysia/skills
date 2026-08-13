//! Configuration loading, topic-root computation and session-id resolution.
//!
//! A single config dir (default `~/mqtt`) holds `mqtt.conf`. The MQTT topic
//! root defaults to the project directory (git repo root / cwd) with the home
//! prefix stripped, so each project gets its own
//! isolated mesh; override with `--root` or a `root` field in `mqtt.conf`.

use anyhow::{anyhow, Result};
use serde_json::Value;
use std::env;
use std::path::{Component, PathBuf};
use std::sync::OnceLock;

pub const DEFAULT_CONFIG_DIR: &str = "~/mqtt";
pub const ENV_SESSION_ID: &str = "CODEX_THREAD_ID";

/// Process-level default topic root, remembered at startup (from `--root` or
/// the project-dir default) so a later `mux_init` without a `root` argument
/// inherits it instead of recomputing from a possibly different cwd.
static DEFAULT_ROOT: OnceLock<String> = OnceLock::new();

pub fn set_default_root(root: &str) {
    let _ = DEFAULT_ROOT.set(root.trim_matches('/').to_string());
}

pub fn default_root() -> Option<&'static str> {
    DEFAULT_ROOT.get().map(|s| s.as_str())
}

#[derive(Debug, Clone)]
pub struct Config {
    pub host: String,
    pub port: u16,
    pub keepalive: u64,
    pub hb_interval: f64,
    pub hb_timeout: f64,
    pub rpc_timeout: f64,
    pub qos: u8,
    pub root: String,
    /// Wake channel preference: "mcp" | "tmux" | "none" (default: MCP notify
    /// when the agent supports it, else tmux, else error).
    pub wake: Option<String>,
    /// When true (default), the node classifies events internally and only
    /// wakes the agent for actionable items (digest mode). When false, every
    /// event wakes the agent as before (opt-out / grayscale rollback).
    pub digest_mode: bool,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            host: "127.0.0.1".into(),
            port: 1883,
            keepalive: 60,
            hb_interval: 5.0,
            hb_timeout: 15.0,
            rpc_timeout: 60.0,
            qos: 1,
            root: String::new(),
            wake: None,
            digest_mode: true,
        }
    }
}

/// Expand a `~`-prefixed path against $HOME.
pub fn expand(path: &str) -> PathBuf {
    if path == "~" {
        return home().unwrap_or_else(|| PathBuf::from(path));
    }
    if let Some(rest) = path.strip_prefix("~/") {
        if let Some(h) = home() {
            return h.join(rest);
        }
    }
    PathBuf::from(path)
}

pub fn home() -> Option<PathBuf> {
    env::var_os("HOME").map(PathBuf::from)
}

/// Home-relative topic path for `p` (home prefix stripped). Returns
/// `None` when `p` is home itself or not under home.
fn strip_home(p: &PathBuf) -> Option<String> {
    let canon = p.canonicalize().unwrap_or_else(|_| p.clone());
    let home = home().and_then(|h| h.canonicalize().ok())?;
    if canon == home {
        return None;
    }
    let rel = canon.strip_prefix(&home).ok()?;
    let parts: Vec<String> = rel
        .components()
        .filter_map(|c| match c {
            Component::Normal(s) => Some(s.to_string_lossy().to_string()),
            _ => None,
        })
        .collect();
    if parts.is_empty() {
        None
    } else {
        Some(parts.join("/"))
    }
}

/// Project directory: the git repository root (via `--git-common-dir`, so
/// linked worktrees share the main repo's root), else the cwd.
fn project_dir() -> PathBuf {
    if let Ok(out) = std::process::Command::new("git")
        .args(["rev-parse", "--git-common-dir"])
        .output()
    {
        if out.status.success() {
            let raw = String::from_utf8_lossy(&out.stdout).trim().to_string();
            if !raw.is_empty() {
                let g = PathBuf::from(&raw);
                let g = if g.is_absolute() {
                    g
                } else if let Ok(cwd) = std::env::current_dir() {
                    cwd.join(g)
                } else {
                    g
                };
                if let Some(parent) = g.parent() {
                    if !parent.as_os_str().is_empty() {
                        return parent.to_path_buf();
                    }
                }
            }
        }
    }
    std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."))
}

/// Topic root = project dir path with the home prefix stripped;
/// basename fallback when not under home.
pub fn project_root_for() -> Option<String> {
    let dir = project_dir();
    strip_home(&dir).or_else(|| {
        dir.file_name()
            .map(|n| n.to_string_lossy().to_string())
    })
}

/// Topic root = config dir path with the home prefix stripped (`~/mqtt` -> `mqtt`).
/// Used only as a fallback when no project dir can be determined.
pub fn topic_root_for(config_dir: &str) -> String {
    let d = expand(config_dir);
    strip_home(&d).unwrap_or_else(|| {
        d.file_name()
            .map(|n| n.to_string_lossy().to_string())
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| "mqtt".to_string())
    })
}

/// Load config from `<config_dir>/mqtt.conf` (created with defaults if absent).
pub fn load_config(config_dir: &str, root: Option<&str>) -> Result<Config> {
    let dir = expand(config_dir);
    std::fs::create_dir_all(&dir)?;
    let f = dir.join("mqtt.conf");
    let mut cfg = Config::default();
    if f.exists() {
        let text = std::fs::read_to_string(&f)?;
        let v: Value = serde_json::from_str(&text)
            .map_err(|e| anyhow!("invalid config {}: {}", f.display(), e))?;
        if let Some(s) = v.get("host").and_then(|x| x.as_str()) {
            cfg.host = s.to_string();
        }
        if let Some(n) = v.get("port").and_then(|x| x.as_u64()) {
            cfg.port = n as u16;
        }
        if let Some(n) = v.get("keepalive").and_then(|x| x.as_u64()) {
            cfg.keepalive = n;
        }
        if let Some(n) = v.get("hb_interval").and_then(|x| x.as_f64()) {
            cfg.hb_interval = n;
        }
        if let Some(n) = v.get("hb_timeout").and_then(|x| x.as_f64()) {
            cfg.hb_timeout = n;
        }
        if let Some(n) = v.get("rpc_timeout").and_then(|x| x.as_f64()) {
            cfg.rpc_timeout = n;
        }
        if let Some(n) = v.get("qos").and_then(|x| x.as_u64()) {
            cfg.qos = n as u8;
        }
        if let Some(s) = v.get("wake").and_then(|x| x.as_str()) {
            if !s.trim().is_empty() {
                cfg.wake = Some(s.trim().to_string());
            }
        }
        if let Some(b) = v.get("digest_mode").and_then(|x| x.as_bool()) {
            cfg.digest_mode = b;
        }
        if let Some(s) = v.get("root").and_then(|x| x.as_str()) {
            if !s.is_empty() {
                cfg.root = s.trim_matches('/').to_string();
            }
        }
    } else {
        let defaults = serde_json::json!({
            "host": cfg.host,
            "port": cfg.port,
            "keepalive": cfg.keepalive,
            "hb_interval": cfg.hb_interval,
            "hb_timeout": cfg.hb_timeout,
            "rpc_timeout": cfg.rpc_timeout,
            "qos": cfg.qos,
            "digest_mode": cfg.digest_mode,
        });
        std::fs::write(&f, serde_json::to_string_pretty(&defaults)? + "\n")?;
    }
    if let Some(r) = root {
        if !r.trim().is_empty() {
            cfg.root = r.trim_matches('/').to_string();
        }
    }
    if cfg.root.is_empty() {
        cfg.root = project_root_for().unwrap_or_else(|| topic_root_for(config_dir));
    }
    Ok(cfg)
}

/// Session id = explicit arg, else `$CODEX_THREAD_ID`. Never random.
pub fn resolve_session_id(session_id: Option<&str>) -> Result<String> {
    let sid = session_id
        .map(|s| s.to_string())
        .filter(|s| !s.is_empty())
        .or_else(|| env::var(ENV_SESSION_ID).ok().filter(|s| !s.is_empty()));
    match sid {
        Some(s) => Ok(s),
        None => Err(anyhow!(
            "no session id: pass --session-id or set {ENV_SESSION_ID}. \
             Ask the agent for its Codex session id; never generate a random one."
        )),
    }
}

pub fn now_ts() -> f64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0)
}
