#!/usr/bin/env python3
"""agent-mux MCP server.

Wraps MuxNode (mux_rpc.py) as MCP tools for Codex agents. One MCP server
process == one node. The node is created LAZILY:

* The MCP server starts up without connecting to MQTT.
* After the agent loads the agent-mux-master / agent-mux-slave skill, it calls
  mux_init(role="master" | "slave", ...) and only then does the node connect.
* The role is decided by which skill loaded (the SKILL.md tells the agent which
  role to pass), not by MCP server startup. Optional short-cuts:
  - env  AGENT_MUX_ROLE=master|slave   -> auto-init on startup
  - cli  python3 mux_mcp.py --role master|slave [--session-id ...]

Run over stdio (default): the Codex MCP config launches this file, e.g.
    [mcp_servers.agent-mux-master]
    command = "python3"
    args = ["/abs/path/agent-mux-master/scripts/mux_mcp.py", "--role", "master"]
"""
from __future__ import annotations

import argparse
import json
import os
import subprocess
import sys
import threading
import time
from typing import Any, Callable, Dict, List, Optional

# Make `mux_rpc` importable no matter where this file is launched from.
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))

from mcp.server import MCPServer  # noqa: E402

from mux_rpc import (  # noqa: E402
    DEFAULT_CONFIG_DIR,
    ENV_ROLE,
    ConfigError,
    MuxNode,
    resolve_session_id,
)

mcp = MCPServer("agent-mux")

_node: Optional[MuxNode] = None


def _require_node() -> MuxNode:
    if _node is None:
        raise ConfigError(
            "agent-mux node not initialized. Call mux_init(role='master'|'slave', ...) "
            "first (after loading the agent-mux-master / agent-mux-slave skill), or set "
            f"{ENV_ROLE} / pass --role when launching this server."
        )
    return _node


# ---------------------------------------------------------------------------
# tmux wake (slave push channel)
# ---------------------------------------------------------------------------
#
# Codex CLI cannot receive server-pushed MCP notifications, but when the TUI
# runs inside tmux the MCP server (a child of the codex process) inherits
# $TMUX_PANE. On a new control/RPC-request message we inject a short wake text
# into that pane (send-keys -l + Enter). The agent sees it as a user message,
# calls mux_pull() and handles the queued message. Business data never goes
# through tmux -- only a wake hint; messages themselves stay in the MCP queues.

_TMUX_DEBOUNCE_SEC = 1.5


def _ancestor_pids() -> set:
    """All ancestor pids of this process (walking ppid up to init)."""
    pids: set = set()
    pid = os.getpid()
    for _ in range(64):
        try:
            r = subprocess.run(["ps", "-o", "ppid=", "-p", str(pid)],
                               capture_output=True, text=True, timeout=3)
        except Exception:
            break
        if r.returncode != 0:
            break
        ppid = int((r.stdout.strip() or "0").split()[0])
        pids.add(pid)
        if ppid <= 0 or ppid == pid:
            break
        pids.add(ppid)
        pid = ppid
        if pid == 1:
            break
    return pids


def _detect_tmux_pane() -> Optional[str]:
    """Return the tmux pane that runs THIS codex TUI, else None.

    Codex does not export $TMUX_PANE to MCP server processes (verified on
    codex-cli 0.147.0), so we cannot rely on the env var. Instead we find the
    pane whose pane_pid appears in our own ancestor chain: the MCP server is a
    child of the codex process, which is a child of the pane's process (or is
    itself the pane process), so the pane_pid is one of our ancestors.
    """
    ancestors = _ancestor_pids()
    # fast path: env var (when present) that points at one of our panes
    env_pane = os.environ.get("TMUX_PANE")
    if env_pane:
        try:
            r = subprocess.run(
                ["tmux", "display-message", "-p", "-t", env_pane, "#{pane_id} #{pane_pid}"],
                capture_output=True, text=True, timeout=3,
            )
        except Exception:
            r = None
        if r is not None and r.returncode == 0:
            parts = r.stdout.split()
            if len(parts) >= 2 and parts[1].isdigit() and int(parts[1]) in ancestors:
                return env_pane
    # fallback: scan every pane and match pane_pid against our ancestors
    try:
        r = subprocess.run(
            ["tmux", "list-panes", "-a", "-F", "#{pane_id} #{pane_pid} #{pane_current_command}"],
            capture_output=True, text=True, timeout=3,
        )
    except Exception:
        return None
    if r.returncode != 0:
        return None
    best: Optional[str] = None
    best_is_codex = False
    for line in r.stdout.splitlines():
        parts = line.split(" ", 2)
        if len(parts) < 2 or not parts[1].isdigit():
            continue
        pid = int(parts[1])
        if pid not in ancestors:
            continue
        pane_id = parts[0]
        cmd = parts[2].strip().lower() if len(parts) > 2 else ""
        is_codex = cmd == "codex"
        if best is None or (is_codex and not best_is_codex):
            best, best_is_codex = pane_id, is_codex
    return best


def _make_tmux_wake(pane_override: Optional[str] = None) -> Optional[Callable[[Dict[str, Any]], None]]:
    """Build a debounced tmux injector for this process's own TUI pane.

    pane_override lets the agent pass its pane id explicitly (from
    `tmux display-message -p '#{pane_id}'`) when auto-detection is impossible.
    Returns None when no usable tmux/codex pane is detectable (e.g. running
    outside tmux, or in a daemon-hosted MCP server) -- the node then simply
    falls back to turn-boundary mux_pull().
    """
    pane = pane_override or _detect_tmux_pane()
    if pane is None:
        return None
    lock = threading.Lock()
    last: List[float] = [0.0]

    def wake(_msg: Dict[str, Any]) -> None:
        now = time.monotonic()
        with lock:
            if now - last[0] < _TMUX_DEBOUNCE_SEC:
                return
            last[0] = now
        text = "[mux] master sent a message: call mux_pull to view and handle it."
        try:
            # Type the hint first, then give the TUI a moment to render it into
            # the input box before sending Enter -- sending Enter back-to-back
            # with the text can swallow the submit key (verified in e2e).
            subprocess.run(["tmux", "send-keys", "-t", pane, "-l", text], timeout=3)
            time.sleep(0.4)
            subprocess.run(["tmux", "send-keys", "-t", pane, "Enter"], timeout=3)
        except Exception:  # pragma: no cover - best effort
            pass

    return wake


# ---------------------------------------------------------------------------
# lifecycle
# ---------------------------------------------------------------------------


@mcp.tool()
def mux_init(
    role: str,
    session_id: Optional[str] = None,
    parent_id: Optional[str] = None,
    config_dir: str = DEFAULT_CONFIG_DIR,
    root: Optional[str] = None,
    master_sid: Optional[str] = None,
    tmux_pane: Optional[str] = None,
) -> Dict[str, Any]:
    """Initialize the agent-mux node and connect to the MQTT broker.

    Must be called once after the master/slave skill loads; the role is decided
    by the skill (agent-mux-master -> 'master', agent-mux-slave -> 'slave').
    session_id defaults to $CODEX_THREAD_ID; if that is unset the agent must
    provide it (ask the user for the Codex session id, never invent one).
    parent_id (slave only) makes the mesh a tree: pass the master's session id
    or the parent slave's session id.

    tmux_pane (slave only, optional): the slave TUI's pane id (e.g. "%3"), e.g.
    from `tmux display-message -p '#{pane_id}'`. Codex does not export
    $TMUX_PANE to MCP server processes, so the node auto-detects its own pane
    by matching pane_pid against the process ancestor chain; pass this only
    when auto-detection fails. Returns the node identity.
    """
    global _node
    old = _node
    if old is not None:
        try:
            old.stop()
        except Exception:
            pass
    node = MuxNode(role=role, config_dir=config_dir, session_id=session_id,
                   parent_id=parent_id, root=root, master_sid=master_sid)
    if role == "slave":
        # Push channel: when the TUI runs in tmux, wake the agent by injecting
        # a short hint so it calls mux_pull() instead of blocking on wait_*.
        node.wake_hook = _make_tmux_wake(tmux_pane)
    node.start()
    try:
        node.call(node.wait_ready(15.0))
    except Exception as e:
        node.stop()
        raise ConfigError(
            f"could not connect to MQTT broker: {e}. Is the broker running? "
            "See agent-mux-master/scripts/docker-compose.yml (docker compose up -d)."
        )
    _node = node
    return {"ok": True, **node.identity()}


@mcp.tool()
def mux_status() -> Dict[str, Any]:
    """Return node identity plus a compact summary of known slaves, pending RPCs and zones."""
    node = _require_node()
    slaves = node.registry if node.role == "master" else {}
    return {
        **node.identity(),
        "known_slaves": len(slaves),
        "pending_rpcs": len(node.pending),
        "zones": len(node.zones),
    }


@mcp.tool()
def topology() -> Dict[str, Any]:
    """Return the slave tree (session ids + parent ids) known to the master."""
    node = _require_node()
    return {"role": node.role, "session_id": node.sid, "tree": node.call(node.list_slaves(), timeout=30)}


# ---------------------------------------------------------------------------
# events
# ---------------------------------------------------------------------------


@mcp.tool()
def wait_events(timeout: float = 30.0) -> List[Dict[str, Any]]:
    """Wait for mesh events; blocks until at least one arrives (or timeout).

    Returns the queued events as a list ([] on timeout). Master events:
    slave_joined, slave_left, status, ctrl_ack, rpc_request,
    conflict_reported. Slave events: rpc_request. Call ONCE when you are ready
    to react; do not busy-loop.
    """
    node = _require_node()
    return node.call(node.wait_events(timeout), timeout=timeout + 5)


@mcp.tool()
def mux_pull() -> Dict[str, Any]:
    """Non-blocking: return all messages already queued for this node.

    Returns {"control": [...], "rpc_requests": [...], "events": [...]} without
    waiting. Call this at turn boundaries, or when a tmux wake tells you the
    master sent something. Control items are {kind, payload, from, request_id,
    ts}; rpc_requests items need rpc_reply(); events are mesh lifecycle events.
    Messages stay queued until consumed here or by the blocking wait_* tools,
    so nothing is lost.
    """
    node = _require_node()
    return node.call(node.pull_queued(), timeout=30)


@mcp.tool()
def wait_control(timeout: float = 30.0) -> Dict[str, Any]:
    """Wait for the next control message from the master (blocks inside the call).

    The mesh is asynchronous: master messages can arrive at any time, so do NOT
    poll. Call this ONCE when you genuinely need the master's input (e.g. you
    reported 'ready' and need a go-ahead, or before touching a high-risk file);
    it blocks until a control message arrives or the timeout elapses.

    Returns:
      {"received": true,  "message": {kind, payload, from, request_id, ts}}
      {"received": false, "reason": "timeout", "waited": <seconds>}
    """
    node = _require_node()
    return node.call(node.wait_control(timeout), timeout=timeout + 5)


@mcp.tool()
def wait_rpc_requests(timeout: float = 30.0) -> List[Dict[str, Any]]:
    """Wait for incoming RPC requests; blocks until at least one (or timeout).

    Each item has request_id / method / params / from. Answer with rpc_reply().
    Returns [] on timeout. Call ONCE when ready to answer; do not busy-loop.
    """
    node = _require_node()
    return node.call(node.wait_rpc_requests(timeout), timeout=timeout + 5)


# ---------------------------------------------------------------------------
# async rpc
# ---------------------------------------------------------------------------


@mcp.tool()
def send_rpc(target: str, method: str, params: Any = None,
             timeout: Optional[float] = None) -> Dict[str, Any]:
    """Send an async RPC request to target (session id) and return a request id.

    Non-blocking: the result arrives later via get_result() / list_pending().
    """
    node = _require_node()
    rid = node.call(node.send_rpc(target, method, params, timeout=timeout), timeout=30)
    return {"ok": True, "request_id": rid, "target": target, "method": method}


@mcp.tool()
def get_result(request_id: str, wait: Optional[float] = None) -> Dict[str, Any]:
    """Return the result of a previously sent RPC; wait (seconds) if still pending."""
    node = _require_node()
    return node.call(node.await_result(request_id, wait=wait), timeout=(wait or 0) + 30)


@mcp.tool()
def ping(target: str, wait: float = 5.0) -> Dict[str, Any]:
    """Send an RPC 'ping' to target and wait for the reply (liveness check)."""
    node = _require_node()
    rid = node.call(node.send_rpc(target, "ping"), timeout=30)
    return node.call(node.await_result(rid, wait=wait), timeout=wait + 30)


@mcp.tool()
def list_pending() -> Dict[str, Any]:
    """List pending RPC requests the master has sent but not yet completed."""
    node = _require_node()
    return node.call(node.list_pending(), timeout=30)


@mcp.tool()
def retry(request_id: str) -> Dict[str, Any]:
    """Re-publish a pending/failed RPC request (retry it)."""
    node = _require_node()
    return node.call(node.retry(request_id), timeout=30)


@mcp.tool()
def cancel(request_id: str) -> Dict[str, Any]:
    """Cancel a pending RPC request."""
    node = _require_node()
    return node.call(node.cancel(request_id), timeout=30)


@mcp.tool()
def rpc_reply(request_id: str, result: Any = None, error: Optional[str] = None) -> Dict[str, Any]:
    """Answer an incoming RPC request previously seen via wait_rpc_requests()."""
    node = _require_node()
    return node.call(node.rpc_reply(request_id, result=result, error=error), timeout=30)


# ---------------------------------------------------------------------------
# control / status
# ---------------------------------------------------------------------------


@mcp.tool()
def send_control(target: str, kind: str, payload: Any = None) -> Dict[str, Any]:
    """Send a control message (master -> slave). kind is free-form, e.g. 'pause'."""
    node = _require_node()
    rid = node.call(node.send_control(target, kind, payload), timeout=30)
    return {"ok": True, "request_id": rid, "target": target, "kind": kind}


@mcp.tool()
def report_status(state: str, plan_files: Optional[List[str]] = None,
                  message: str = "", blocked_reason: str = "") -> Dict[str, Any]:
    """Report this slave's status to the master (state + touched files + message).

    Call with state='planning' / 'ready' / 'working' / 'blocked' / 'done' and
    plan_files = the concrete files you intend to modify when you are ready to
    coordinate, so the master can schedule work and avoid conflicts.
    """
    node = _require_node()
    return node.call(node.report_status(state, plan_files, message, blocked_reason), timeout=30)


# ---------------------------------------------------------------------------
# zone locks (master owns the registry)
# ---------------------------------------------------------------------------


@mcp.tool()
def list_zones() -> Dict[str, Any]:
    """List the zone-lock registry (which paths are owned by which session)."""
    node = _require_node()
    return node.call(node.list_zones(), timeout=30)


@mcp.tool()
def zone_acquire(path: str, owner: Optional[str] = None, force: bool = False) -> Dict[str, Any]:
    """Acquire the lock for a path/zone (default owner = this node)."""
    node = _require_node()
    return node.call(node.zone_acquire(path, owner=owner, force=force), timeout=30)


@mcp.tool()
def zone_release(path: str, owner: Optional[str] = None) -> Dict[str, Any]:
    """Release the lock for a path/zone (only the owner may release)."""
    node = _require_node()
    return node.call(node.zone_release(path, owner=owner), timeout=30)


@mcp.tool()
def get_zone_snapshot() -> Dict[str, Any]:
    """Return the last retained zone snapshot observed on the bus."""
    node = _require_node()
    return node.call(node.get_zone_snapshot(), timeout=30)


# ---------------------------------------------------------------------------
# conflict feedback (all nodes report; the master learns)
# ---------------------------------------------------------------------------


@mcp.tool()
def report_conflict(files: Optional[List[str]] = None,
                    description: str = "",
                    severity: str = "medium",
                    suggestion: str = "",
                    zone: Optional[str] = None) -> Dict[str, Any]:
    """Report a conflict (or a conflict risk) to the master.

    Call this when your edits collide with another slave's work, or when you
    detect a high-risk overlap. files = concrete paths involved, zone = an
    optional shared path/zone name, severity = low|medium|high|critical,
    suggestion = how the master should adjust (e.g. 'serialize zone X').
    The master records and persists the report and uses it to mark risk zones
    and serialize work, so coordination improves over time.
    """
    node = _require_node()
    return node.call(node.report_conflict(files=files, description=description,
                                          severity=severity, suggestion=suggestion,
                                          zone=zone), timeout=30)


@mcp.tool()
def list_conflicts(limit: int = 50) -> Dict[str, Any]:
    """Master: list recorded conflict reports (newest first)."""
    node = _require_node()
    return node.call(node.list_conflicts(limit=limit), timeout=30)


@mcp.tool()
def risk_zones() -> Dict[str, Any]:
    """Master: aggregate conflict history into per-path risk zones.

    Paths with more conflict reports rank higher; the master should serialize
    work on high-count paths and treat them as conflict-risk zones.
    """
    node = _require_node()
    return node.call(node.risk_zones(), timeout=30)


# ---------------------------------------------------------------------------
# startup
# ---------------------------------------------------------------------------


def _auto_init(role: Optional[str]) -> None:
    """Best-effort auto-init when --role is given and a session id is known.

    Codex does not always export $CODEX_THREAD_ID to MCP server processes, so a
    missing session id is NOT fatal here: the server stays alive and the agent
    calls mux_init(role=..., session_id=<its Codex session id>) after the skill
    loads (initialization is deferred until then).
    """
    if not role:
        return
    if role not in ("master", "slave"):
        sys.stderr.write(f"agent-mux: invalid --role {role!r} (expected master|slave)\n")
        sys.exit(2)
    try:
        node = MuxNode(role=role, config_dir=DEFAULT_CONFIG_DIR,
                       session_id=resolve_session_id(None))
        if role == "slave":
            node.wake_hook = _make_tmux_wake()
    except ConfigError as e:
        sys.stderr.write(
            f"agent-mux: {e}\n"
            "agent-mux: deferring init -- call mux_init(role=..., "
            "session_id=<your Codex session id>) after the skill loads\n"
        )
        return
    node.start()
    try:
        node.call(node.wait_ready(15.0))
    except Exception as e:
        sys.stderr.write(f"agent-mux: could not connect to MQTT broker: {e}\n")
        sys.exit(1)
    global _node
    _node = node
    sys.stderr.write(f"agent-mux: auto-initialized as {role} ({node.sid})\n")


def main(argv: Optional[List[str]] = None) -> int:
    p = argparse.ArgumentParser(prog="mux_mcp.py", description="agent-mux MCP server (stdio)")
    p.add_argument("--role", default=None, choices=["master", "slave"],
                   help="auto-init role at startup (default: wait for mux_init tool)")
    p.add_argument("--session-id", default=None,
                   help="codex session id (default $CODEX_THREAD_ID)")
    args = p.parse_args(argv)
    role = args.role or os.environ.get(ENV_ROLE)
    if role and args.session_id:
        os.environ["CODEX_THREAD_ID"] = args.session_id
    _auto_init(role)
    mcp.run()
    return 0


if __name__ == "__main__":
    sys.exit(main())
