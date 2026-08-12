//! Configuration loading, topic-root computation and session-id resolution.
//!
//! Mirrors the Python reference (`mux_rpc.py`): a single config dir (default
//! `~/mqtt`) holds `mqtt.conf`; the MQTT topic root is the config dir path
//! with the home prefix stripped (`~/mqtt` -> `mqtt`).

use anyhow::{anyhow, Result};
use serde_json::Value;
use std::env;
use std::path::{Component, PathBuf};

pub const DEFAULT_CONFIG_DIR: &str = "~/mqtt";
pub const ENV_SESSION_ID: &str = "CODEX_THREAD_ID";

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
}

impl Default for Config {
    fn default() -> Self {
        Config {
            host: "127.0.0.1".into(),
            port: 1883,
            keepalive: 60,
            hb_interval: 5.0,
            hb_timeout: 15.0,
            rpc_timeout: 30.0,
            qos: 1,
            root: String::new(),
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

/// Topic root = config dir path with the home prefix stripped (`~/mqtt` -> `mqtt`).
pub fn topic_root_for(config_dir: &str) -> String {
    let d = expand(config_dir);
    let canon = d.canonicalize().unwrap_or_else(|_| d.clone());
    let home = home().and_then(|h| h.canonicalize().ok());
    let rel: PathBuf = match &home {
        Some(h) => canon.strip_prefix(h).map(|r| r.to_path_buf()).unwrap_or_else(|_| {
            PathBuf::from(canon.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default())
        }),
        None => canon.clone(),
    };
    let parts: Vec<String> = rel
        .components()
        .filter_map(|c| match c {
            Component::Normal(s) => Some(s.to_string_lossy().to_string()),
            _ => None,
        })
        .collect();
    if parts.is_empty() {
        "mqtt".to_string()
    } else {
        parts.join("/")
    }
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
        });
        std::fs::write(&f, serde_json::to_string_pretty(&defaults)? + "\n")?;
    }
    if let Some(r) = root {
        if !r.trim().is_empty() {
            cfg.root = r.trim_matches('/').to_string();
        }
    }
    if cfg.root.is_empty() {
        cfg.root = topic_root_for(config_dir);
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
