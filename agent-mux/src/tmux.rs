//! tmux wake (push channel).
//!
//! Codex CLI cannot receive server-pushed MCP notifications, but when the TUI
//! runs inside tmux the MCP server (a child of the codex process) can find its
//! own pane (pane_pid appears in the process ancestor chain) and inject a short
//! wake hint into that pane (`send-keys -l` + Enter). The agent sees it as a
//! user message and calls `mux_pull()` to drain the real queues. Business data
//! never goes through tmux -- only a hint.

use std::collections::HashSet;
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const WAKE_TEXT: &str = "[mux] new message arrived: call mux_pull to view and handle it.";
const DEBOUNCE: Duration = Duration::from_millis(1500);

pub struct TmuxWake {
    pane: String,
    last: AtomicU64, // millis since epoch, for debounce
}

impl TmuxWake {
    pub fn new(pane: String) -> Self {
        TmuxWake {
            pane,
            last: AtomicU64::new(0),
        }
    }

    /// Detect the tmux pane that runs THIS codex TUI.
    ///
    /// `pane_override` (from `mux_init(tmux_pane=...)`) wins. Otherwise the
    /// env var `$TMUX_PANE` is used when it points at one of our ancestor
    /// panes; finally every pane is scanned and the first whose pane_pid is in
    /// our ancestor chain is chosen (codex panes preferred).
    pub fn detect(pane_override: Option<String>) -> Option<String> {
        // Opt-out for tests / non-interactive runs: AGENT_MUX_NO_TMUX=1.
        if std::env::var_os("AGENT_MUX_NO_TMUX").is_some() {
            return None;
        }
        if let Some(p) = pane_override {
            if !p.trim().is_empty() {
                return Some(p.trim().to_string());
            }
        }
        let ancestors = ancestor_pids();
        if ancestors.is_empty() {
            return None;
        }
        if let Ok(env_pane) = std::env::var("TMUX_PANE") {
            let env_pane = env_pane.trim().to_string();
            if !env_pane.is_empty()
                && pane_pid(&env_pane).map(|pid| ancestors.contains(&pid)).unwrap_or(false)
            {
                return Some(env_pane);
            }
        }
        let out = Command::new("tmux")
            .args(["list-panes", "-a", "-F", "#{pane_id} #{pane_pid} #{pane_current_command}"])
            .output()
            .ok()?;
        if !out.status.success() {
            return None;
        }
        let text = String::from_utf8_lossy(&out.stdout);
        let mut best: Option<String> = None;
        let mut best_is_codex = false;
        for line in text.lines() {
            let mut parts = line.splitn(3, ' ');
            let Some(pane) = parts.next() else { continue };
            let Some(pid_s) = parts.next() else { continue };
            let Ok(pid) = pid_s.trim().parse::<u32>() else { continue };
            if !ancestors.contains(&pid) {
                continue;
            }
            let cmd = parts.next().unwrap_or("").trim().to_lowercase();
            let is_codex = cmd.contains("codex");
            if best.is_none() || (is_codex && !best_is_codex) {
                best = Some(pane.to_string());
                best_is_codex = is_codex;
            }
        }
        best
    }

    /// Best-effort, debounced wake: inject the hint into the pane.
    pub fn wake(self: &Arc<TmuxWake>) {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);
        let last = self.last.load(Ordering::Relaxed);
        if now.saturating_sub(last) < DEBOUNCE.as_millis() as u64 {
            return;
        }
        self.last.store(now, Ordering::Relaxed);
        let this = self.clone();
        tokio::task::spawn_blocking(move || {
            // Type the hint first, then give the TUI a moment to render it into
            // the input box before sending Enter (verified in e2e).
            let _ = Command::new("tmux")
                .args(["send-keys", "-t", &this.pane, "-l", WAKE_TEXT])
                .status();
            std::thread::sleep(Duration::from_millis(400));
            let _ = Command::new("tmux")
                .args(["send-keys", "-t", &this.pane, "Enter"])
                .status();
        });
    }
}

fn ancestor_pids() -> HashSet<u32> {
    let mut set = HashSet::new();
    let mut pid = std::process::id();
    for _ in 0..64 {
        let out = Command::new("ps")
            .args(["-o", "ppid=", "-p", &pid.to_string()])
            .output()
            .ok();
        let Some(out) = out else { break };
        if !out.status.success() {
            break;
        }
        let s = String::from_utf8_lossy(&out.stdout);
        let s = s.trim();
        if s.is_empty() {
            break;
        }
        let Ok(ppid) = s.split_whitespace().next().unwrap_or("0").parse::<u32>() else {
            break;
        };
        set.insert(pid);
        if ppid == 0 || ppid == pid {
            break;
        }
        set.insert(ppid);
        pid = ppid;
        if pid == 1 {
            break;
        }
    }
    set
}

fn pane_pid(pane: &str) -> Option<u32> {
    let out = Command::new("tmux")
        .args(["display-message", "-p", "-t", pane, "#{pane_pid}"])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    String::from_utf8_lossy(&out.stdout).trim().parse().ok()
}
