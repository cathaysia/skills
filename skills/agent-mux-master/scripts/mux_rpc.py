#!/usr/bin/env python3
"""agent-mux: MQTT-based async RPC + presence + coordination primitives.

Single-file core shared by the agent-mux-master and agent-mux-slave skills.

Roles
-----
master : one process. Subscribes to the whole topic root, builds the slave
         tree (session id + parent id), tracks heartbeats/presence, keeps a
         pending-RPC registry (list/retry), and owns the zone-lock registry.
slave  : many processes. Registers itself (optionally with a parent id so the
         mesh is a tree), heartbeats, reports status, receives control
         messages, and can RPC any other node (usually the master).

The MCP server in mux_mcp.py wraps this class as tools for Codex agents. The
node itself is created lazily: nothing connects to MQTT until the agent calls
mux_init() (or AGENT_MUX_ROLE is set), i.e. after the master/slave skill loads.
"""
from __future__ import annotations

import argparse
import asyncio
import json
import os
import sys
import threading
import time
import uuid
from pathlib import Path
from typing import Any, Callable, Dict, List, Optional

import paho.mqtt.client as mqtt

try:
    from paho.mqtt.enums import CallbackAPIVersion
except Exception:  # paho < 2.0
    CallbackAPIVersion = None

DEFAULT_CONFIG: Dict[str, Any] = {
    "host": "127.0.0.1",
    "port": 1883,
    "keepalive": 60,
    "hb_interval": 5.0,
    "hb_timeout": 15.0,
    "rpc_timeout": 30.0,
    "qos": 1,
}
DEFAULT_CONFIG_DIR = "~/mqtt"
ENV_SESSION_ID = "CODEX_THREAD_ID"
ENV_ROLE = "AGENT_MUX_ROLE"


class ConfigError(RuntimeError):
    """Raised for invalid role/session/config; message is agent-facing."""


def _home() -> str:
    return os.path.expanduser("~")


def topic_root_for(config_dir: str) -> str:
    """Topic root = config dir path with the home prefix stripped (~/mqtt -> mqtt)."""
    d = Path(config_dir).expanduser().resolve()
    home = Path(_home()).resolve()
    try:
        rel = d.relative_to(home)
    except ValueError:
        rel = Path(d.name)
    return "/".join(rel.parts).strip("/") or "mqtt"


def load_config(config_dir: str, root: Optional[str] = None) -> Dict[str, Any]:
    d = Path(config_dir).expanduser()
    d.mkdir(parents=True, exist_ok=True)
    conf = dict(DEFAULT_CONFIG)
    f = d / "mqtt.conf"
    if f.exists():
        try:
            conf.update(json.loads(f.read_text()))
        except Exception as e:
            raise ConfigError(f"invalid config {f}: {e}")
    else:
        f.write_text(json.dumps(conf, indent=2) + "\n")
    if root:
        conf["root"] = root
    if not conf.get("root"):
        conf["root"] = topic_root_for(str(d))
    return conf


def resolve_session_id(session_id: Optional[str]) -> str:
    """Session id = explicit arg, else $CODEX_THREAD_ID. Never random."""
    sid = session_id or os.environ.get(ENV_SESSION_ID)
    if not sid:
        raise ConfigError(
            f"no session id: pass --session-id or set {ENV_SESSION_ID}. "
            "Ask the agent for its Codex session id; never generate a random one."
        )
    return str(sid)


class Pending:
    __slots__ = ("request_id", "target", "method", "params", "status",
                 "attempts", "created_at", "expires_at", "future", "result", "error")

    def __init__(self, request_id: str, target: str, method: str, params: Any):
        self.request_id = request_id
        self.target = target
        self.method = method
        self.params = params
        self.status = "pending"
        self.attempts = 1
        self.created_at = time.time()
        self.expires_at = self.created_at
        self.future: Optional[asyncio.Future] = None
        self.result: Any = None
        self.error: Any = None

    def to_dict(self) -> Dict[str, Any]:
        return {
            "request_id": self.request_id,
            "target": self.target,
            "method": self.method,
            "params": self.params,
            "status": self.status,
            "attempts": self.attempts,
            "created_at": self.created_at,
            "expires_at": self.expires_at,
            "result": self.result,
            "error": self.error,
        }


class MuxNode:
    """One node in the agent-mux mesh. Role is fixed at construction time."""

    def __init__(self, role: str, config_dir: str = DEFAULT_CONFIG_DIR,
                 session_id: Optional[str] = None, parent_id: Optional[str] = None,
                 root: Optional[str] = None, master_sid: Optional[str] = None):
        if role not in ("master", "slave"):
            raise ConfigError(f"role must be 'master' or 'slave', got {role!r}")
        self.role = role
        self.config_dir = str(Path(config_dir).expanduser())
        self.conf = load_config(self.config_dir, root)
        self.root = self.conf["root"].strip("/")
        self.sid = resolve_session_id(session_id)
        self.parent_id = str(parent_id) if parent_id else None
        self.master_sid = master_sid

        self._loop: Optional[asyncio.AbstractEventLoop] = None
        self._thread: Optional[threading.Thread] = None
        self._client: Optional[mqtt.Client] = None
        self._connected = False
        self._shutting_down = False
        self._ready: Optional[asyncio.Event] = None

        self.registry: Dict[str, Dict[str, Any]] = {}
        self.pending: Dict[str, Pending] = {}
        self.handlers: Dict[str, Callable[[Any], Any]] = {"ping": self._handle_ping}
        self.ctrl_queue: Optional[asyncio.Queue] = None
        self.events: Optional[asyncio.Queue] = None
        self.rpc_requests: Optional[asyncio.Queue] = None
        self._rpc_meta: Dict[str, Dict[str, Any]] = {}
        self.zones: Dict[str, Dict[str, Any]] = {}
        self.zone_snapshot: Dict[str, Any] = {}
        self.status: Dict[str, Any] = {}
        self.conflicts: Dict[str, Dict[str, Any]] = self._load_conflicts()
        self._conflict_lock = threading.Lock()
        # Optional callback fired (on the node loop) when a control message
        # or an incoming RPC request arrives. The MCP layer sets this to a
        # tmux injection when running inside a Codex TUI so the agent is
        # woken without polling. Must be cheap/thread-safe; run in a thread.
        self.wake_hook: Optional[Callable[[Dict[str, Any]], None]] = None
        self._hb_task: Optional[asyncio.Task] = None
        self._sweep_task: Optional[asyncio.Task] = None

    # ---------- identity / topics ----------

    def t(self, *parts: str) -> str:
        return "/".join(p for p in (self.root,) + parts if p)

    def identity(self) -> Dict[str, Any]:
        return {
            "role": self.role,
            "session_id": self.sid,
            "parent_id": self.parent_id,
            "root": self.root,
            "config_dir": self.config_dir,
            "broker": f"{self.conf['host']}:{self.conf['port']}",
            "master_session_id": self.master_sid,
            "connected": self._connected,
            "status": self.status,
        }

    # ---------- lifecycle ----------

    def start(self) -> None:
        if self._loop is not None:
            return
        self._loop = asyncio.new_event_loop()
        self._thread = threading.Thread(target=self._run_loop, name=f"mux-{self.role}", daemon=True)
        self._thread.start()

    def stop(self) -> None:
        loop = self._loop
        if loop is None or not loop.is_running():
            return
        try:
            fut = asyncio.run_coroutine_threadsafe(self._shutdown(), loop)
            fut.result(timeout=5)
        except Exception:
            pass
        finally:
            # Stop the loop from within itself, only after _shutdown finished.
            # This avoids creating coroutines on an already-closed loop.
            if not loop.is_closed():
                try:
                    loop.call_soon_threadsafe(loop.stop)
                except Exception:
                    pass

    def call(self, coro, timeout: float = 120.0) -> Any:
        """Run a coroutine on the node loop and wait for its result (thread-safe)."""
        if self._loop is None:
            raise ConfigError("node not started")
        return asyncio.run_coroutine_threadsafe(coro, self._loop).result(timeout=timeout)

    def _run_loop(self) -> None:
        asyncio.set_event_loop(self._loop)
        try:
            self._loop.run_until_complete(self._start_async())
        except Exception as e:  # pragma: no cover - defensive
            sys.stderr.write(f"mux start failed: {e}\n")
            return
        self._loop.run_forever()
        self._loop.close()

    async def _start_async(self) -> None:
        self._ready = asyncio.Event()
        self.ctrl_queue = asyncio.Queue()
        self.events = asyncio.Queue()
        self.rpc_requests = asyncio.Queue()
        self._client = self._make_client()
        self._client.connect_async(self.conf["host"], self.conf["port"], keepalive=self.conf["keepalive"])
        self._client.loop_start()
        if self.role == "slave":
            self._hb_task = asyncio.create_task(self._heartbeat_loop())
        else:
            self._sweep_task = asyncio.create_task(self._sweep_loop())

    async def wait_ready(self, timeout: float = 15.0) -> None:
        if self._ready is None:
            raise ConfigError("node not started")
        await asyncio.wait_for(self._ready.wait(), timeout)

    async def _shutdown(self) -> None:
        self._shutting_down = True
        tasks = []
        for task in (self._hb_task, self._sweep_task):
            if task is not None and not task.done():
                task.cancel()
                tasks.append(task)
        for task in tasks:
            try:
                await task
            except (asyncio.CancelledError, Exception):
                pass
        if self._client is not None and self._connected:
            await self._publish_raw(self.t("presence", self.sid), b"", retain=True)
            await self._publish_raw(self.t("registry", self.sid), b"", retain=True)
            try:
                self._client.disconnect()
            except Exception:
                pass
            self._client.loop_stop()

    # ---------- mqtt plumbing ----------

    def _make_client(self) -> mqtt.Client:
        cid = f"mux-{self.role}-{self.sid[:8]}-{os.getpid()}-{uuid.uuid4().hex[:6]}"
        if CallbackAPIVersion is not None:
            client = mqtt.Client(CallbackAPIVersion.VERSION2, client_id=cid,
                                 protocol=mqtt.MQTTv311, clean_session=True)
        else:  # pragma: no cover - paho 1.x
            client = mqtt.Client(client_id=cid, protocol=mqtt.MQTTv311, clean_session=True)
        client.reconnect_delay_set(1, 30)
        will = json.dumps({"sid": self.sid, "role": self.role, "status": "offline", "ts": time.time()})
        client.will_set(self.t("presence", self.sid), payload=will.encode(), qos=1, retain=True)
        client.on_connect = self._on_connect
        client.on_disconnect = self._on_disconnect
        client.on_message = self._on_message
        return client

    def _schedule(self, coro) -> None:
        if self._loop is None or not self._loop.is_running() or self._shutting_down:
            coro.close()  # avoid 'coroutine was never awaited' noise on shutdown
            return
        asyncio.run_coroutine_threadsafe(coro, self._loop)

    def _on_connect(self, *args) -> None:
        self._schedule(self._handle_connected())

    def _on_disconnect(self, *args) -> None:
        self._schedule(self._handle_disconnected())

    def _on_message(self, client, userdata, msg) -> None:
        self._schedule(self._handle_message(msg.topic, msg.payload))

    async def _handle_connected(self) -> None:
        self._connected = True
        if self.role == "master":
            self._client.subscribe(self.t("#"), qos=self.conf["qos"])
        else:
            for topic in (self.t("ctrl", self.sid), self.t("rpc", "req", self.sid),
                          self.t("rpc", "resp", self.sid), self.t("master"), self.t("zones")):
                self._client.subscribe(topic, qos=self.conf["qos"])
        await self._announce()
        if self._ready is not None:
            self._ready.set()

    async def _handle_disconnected(self) -> None:
        self._connected = False

    async def _announce(self) -> None:
        now = time.time()
        await self._publish(self.t("registry", self.sid),
                            {"sid": self.sid, "parent_id": self.parent_id,
                             "role": self.role, "joined_at": now}, retain=True)
        await self._publish(self.t("presence", self.sid),
                            {"sid": self.sid, "role": self.role, "status": "online", "ts": now}, retain=True)
        if self.role == "master":
            await self._publish(self.t("master"),
                                {"sid": self.sid, "role": "master", "ts": now}, retain=True)
            await self._publish_zones()
        else:
            await self._publish(self.t("hb", self.sid),
                                {"sid": self.sid, "parent_id": self.parent_id,
                                 "role": self.role, "state": self.status.get("state", "idle"), "ts": now})

    async def _publish(self, topic: str, payload: Any, retain: bool = False) -> None:
        await self._publish_raw(topic, json.dumps(payload, ensure_ascii=False).encode(), retain)

    async def _publish_raw(self, topic: str, data: bytes, retain: bool = False) -> None:
        if self._client is None:
            return
        self._client.publish(topic, data, qos=self.conf["qos"], retain=retain)

    # ---------- background loops ----------

    async def _heartbeat_loop(self) -> None:
        while True:
            await asyncio.sleep(self.conf["hb_interval"])
            if self._connected:
                await self._publish(self.t("hb", self.sid),
                                    {"sid": self.sid, "parent_id": self.parent_id,
                                     "role": self.role, "state": self.status.get("state", "idle"),
                                     "ts": time.time()})

    async def _sweep_loop(self) -> None:
        while True:
            await asyncio.sleep(max(1.0, self.conf["hb_timeout"] / 2))
            now = time.time()
            for sid, info in list(self.registry.items()):
                if sid == self.sid:
                    continue
                if info.get("status") == "online" and now - info.get("last_seen", 0) > self.conf["hb_timeout"]:
                    self._mark_offline(sid, "heartbeat_timeout")

    # ---------- message routing ----------

    async def _handle_message(self, topic: str, payload: bytes) -> None:
        parts = topic.split("/")
        if not parts or parts[0] != self.root:
            return
        rel = parts[1:]
        try:
            data = json.loads(payload.decode("utf-8"))
        except Exception:
            data = {}
        try:
            if len(rel) == 2 and rel[0] == "registry":
                await self._on_registry(rel[1], data)
            elif len(rel) == 2 and rel[0] == "presence":
                await self._on_presence(rel[1], data)
            elif len(rel) == 2 and rel[0] == "hb":
                await self._on_hb(rel[1], data)
            elif len(rel) == 2 and rel[0] == "status":
                await self._on_status(rel[1], data)
            elif len(rel) == 2 and rel[0] == "conflict":
                await self._on_conflict(rel[1], data)
            elif len(rel) == 2 and rel[0] == "ctrl" and rel[1] == self.sid:
                await self._on_ctrl(data)
            elif len(rel) == 2 and rel[0] == "ctrl_ack" and self.role == "master":
                await self._on_ctrl_ack(rel[1], data)
            elif len(rel) == 3 and rel[0] == "rpc" and rel[1] == "req" and rel[2] == self.sid:
                await self._on_rpc_request(data)
            elif len(rel) == 3 and rel[0] == "rpc" and rel[1] == "resp" and rel[2] == self.sid:
                await self._on_rpc_response(data)
            elif len(rel) == 1 and rel[0] == "master":
                if data.get("sid") and data["sid"] != self.sid:
                    self.master_sid = data["sid"]
            elif len(rel) == 1 and rel[0] == "zones":
                self.zone_snapshot = data
                if self.role != "master":
                    # Only non-masters adopt the bus snapshot. The master is the
                    # sole authoritative writer; adopting its own (possibly stale)
                    # retained echo would clobber the local zone registry.
                    self.zones = data.get("zones", {})
        except Exception as e:  # pragma: no cover - defensive
            sys.stderr.write(f"mux route error on {topic}: {e}\n")

    # ---------- topology (master side) ----------

    async def _on_registry(self, sid: str, data: Dict[str, Any]) -> None:
        if sid == self.sid or self.role != "master":
            return
        if not data:
            # retained registry entry was cleared (node left). Keep the record so
            # the master still sees it, but mark it offline.
            info = self.registry.setdefault(sid, {"sid": sid})
            info["status"] = "offline"
            info["offline_reason"] = "unregistered"
            info["last_seen"] = time.time()
            return
        new = sid not in self.registry
        info = dict(data)
        info["last_seen"] = time.time()
        info.setdefault("status", "online")
        self.registry[sid] = info
        if new and info.get("status") == "online":
            await self._push_event({"kind": "slave_joined", "session_id": sid,
                                    "parent_id": info.get("parent_id"), "info": info})

    async def _on_presence(self, sid: str, data: Dict[str, Any]) -> None:
        if sid == self.sid or self.role != "master":
            return
        info = self.registry.setdefault(sid, {"sid": sid, "last_seen": time.time()})
        info["last_seen"] = time.time()
        if not data:
            # retained presence was cleared -> node left; never resurrect online
            info["status"] = "offline"
            info["offline_reason"] = "presence_cleared"
        else:
            info["status"] = data.get("status", info.get("status"))
        if info["status"] == "online":
            await self._push_event({"kind": "slave_joined", "session_id": sid,
                                    "parent_id": info.get("parent_id"), "info": info})
        else:
            await self._push_event({"kind": "slave_left", "session_id": sid,
                                    "reason": data.get("reason", "presence_offline")})
            await self._publish_raw(self.t("registry", sid), b"", retain=True)

    async def _on_hb(self, sid: str, data: Dict[str, Any]) -> None:
        if sid == self.sid or self.role != "master":
            return
        info = self.registry.setdefault(sid, {"sid": sid})
        info["last_seen"] = time.time()
        info["status"] = "online"
        info.setdefault("parent_id", data.get("parent_id"))
        info.setdefault("role", data.get("role", "slave"))
        info["state"] = data.get("state", info.get("state", "unknown"))

    async def _on_status(self, sid: str, data: Dict[str, Any]) -> None:
        if sid == self.sid or self.role != "master":
            return
        info = self.registry.setdefault(sid, {"sid": sid, "last_seen": time.time()})
        info["last_seen"] = time.time()
        for k in ("state", "plan_files", "message", "blocked_reason", "parent_id", "role"):
            if k in data:
                info[k] = data[k]
        await self._push_event({"kind": "status", "session_id": sid, "info": info})

    def _mark_offline(self, sid: str, reason: str) -> None:
        info = self.registry.get(sid)
        if info is None:
            return
        info["status"] = "offline"
        info["offline_reason"] = reason
        if self.events is not None:
            self.events.put_nowait({"kind": "slave_left", "session_id": sid, "reason": reason})

    # ---------- tree helpers (master side) ----------

    def children(self, sid: str) -> List[str]:
        if sid == self.sid:
            # Direct children of the master may register with parent_id=None
            # (they learn the master's sid from the retained master message);
            # attach them under the master. Exclude the master's own entry.
            return sorted(
                n["sid"] for n in self.registry.values()
                if n.get("sid") != self.sid
                and n.get("role") != "master"
                and n.get("parent_id") in (sid, None, "")
            )
        return sorted(n["sid"] for n in self.registry.values() if n.get("parent_id") == sid)

    def subtree(self, sid: str) -> List[str]:
        out: List[str] = []
        stack = [sid]
        while stack:
            cur = stack.pop()
            out.append(cur)
            stack.extend(self.children(cur))
        return out

    def to_tree(self, root: Optional[str] = None) -> Dict[str, Any]:
        root = root or self.sid

        def build(sid: str) -> Dict[str, Any]:
            return {"session_id": sid, "info": self.registry.get(sid, {}),
                    "children": [build(c) for c in self.children(sid)]}

        return build(root)

    async def list_slaves(self) -> Dict[str, Any]:
        return self.to_tree()

    async def wait_events(self, timeout: float = 30.0) -> List[Dict[str, Any]]:
        if self.events is None:
            raise ConfigError("node not started")
        try:
            first = await asyncio.wait_for(self.events.get(), timeout)
        except asyncio.TimeoutError:
            return []
        out = [first]
        while True:
            try:
                out.append(self.events.get_nowait())
            except asyncio.QueueEmpty:
                break
        return out

    # ---------- async rpc ----------

    async def send_rpc(self, target: str, method: str, params: Any = None,
                       timeout: Optional[float] = None) -> str:
        timeout = timeout or self.conf["rpc_timeout"]
        rid = uuid.uuid4().hex
        p = Pending(rid, target, method, params)
        p.expires_at = time.time() + timeout
        p.future = self._loop.create_future()
        self.pending[rid] = p
        await self._publish(self.t("rpc", "req", target), {
            "id": rid, "method": method, "params": params,
            "reply_to": self.t("rpc", "resp", self.sid),
            "from": self.sid, "ts": time.time(),
        })
        return rid

    async def await_result(self, request_id: str, wait: Optional[float] = None) -> Dict[str, Any]:
        p = self.pending.get(request_id)
        if p is None:
            return {"status": "unknown", "request_id": request_id}
        if p.status in ("done", "failed", "cancelled"):
            return {"status": p.status, "result": p.result, "error": p.error,
                    "request_id": request_id, "attempts": p.attempts}
        if p.future is None:
            return {"status": p.status, "request_id": request_id}
        try:
            await asyncio.wait_for(p.future, wait if wait is not None else max(0.0, p.expires_at - time.time()))
        except asyncio.TimeoutError:
            return {"status": "pending", "request_id": request_id,
                    "attempts": p.attempts, "expires_at": p.expires_at}
        return {"status": p.status, "result": p.result, "error": p.error,
                "request_id": request_id, "attempts": p.attempts}

    async def list_pending(self) -> Dict[str, Any]:
        return {rid: p.to_dict() for rid, p in sorted(self.pending.items())}

    async def retry(self, request_id: str) -> Dict[str, Any]:
        p = self.pending.get(request_id)
        if p is None:
            return {"ok": False, "error": f"unknown request {request_id}"}
        if p.status in ("done", "cancelled"):
            return {"ok": False, "error": f"request {request_id} already {p.status}"}
        p.attempts += 1
        p.expires_at = time.time() + self.conf["rpc_timeout"]
        p.future = self._loop.create_future()
        p.status = "pending"
        p.error = None
        await self._publish(self.t("rpc", "req", p.target), {
            "id": p.request_id, "method": p.method, "params": p.params,
            "reply_to": self.t("rpc", "resp", self.sid),
            "from": self.sid, "ts": time.time(),
        })
        return {"ok": True, "request_id": request_id, "attempts": p.attempts}

    async def cancel(self, request_id: str) -> Dict[str, Any]:
        p = self.pending.get(request_id)
        if p is None:
            return {"ok": False, "error": f"unknown request {request_id}"}
        p.status = "cancelled"
        if p.future is not None and not p.future.done():
            p.future.set_result(None)
        return {"ok": True, "request_id": request_id}

    async def _on_rpc_response(self, data: Dict[str, Any]) -> None:
        rid = data.get("id")
        p = self.pending.get(rid)
        if p is None:
            return
        if data.get("ok"):
            p.status = "done"
            p.result = data.get("result")
            p.error = None
        else:
            p.status = "failed"
            p.error = data.get("error") or "rpc error"
        if p.future is not None and not p.future.done():
            p.future.set_result(None)

    async def _on_rpc_request(self, data: Dict[str, Any]) -> None:
        handler = self.handlers.get(data.get("method"))
        reply_to = data.get("reply_to")
        rid = data.get("id")
        if handler is None:
            # No in-process handler: queue for the agent (Codex) to answer via
            # wait_rpc_requests()/rpc_reply(). The pending RPC on the caller side
            # stays pending until it times out or this node answers.
            req = {
                "request_id": rid, "method": data.get("method"),
                "params": data.get("params"), "from": data.get("from"),
                "reply_to": reply_to, "ts": time.time(),
            }
            self._rpc_meta[rid] = {"reply_to": reply_to, "from": data.get("from")}
            if self.rpc_requests is not None:
                self.rpc_requests.put_nowait(req)
            await self._push_event({"kind": "rpc_request", "request_id": rid,
                                    "method": data.get("method"), "from": data.get("from")})
            await self._wake(req)
            return
        try:
            result = handler(data.get("params"))
            if asyncio.iscoroutine(result):
                result = await result
            resp = {"id": rid, "ok": True, "result": result, "ts": time.time()}
        except Exception as e:
            resp = {"id": rid, "ok": False,
                    "error": f"{type(e).__name__}: {e}", "ts": time.time()}
        if reply_to:
            await self._publish(reply_to, resp)

    async def wait_rpc_requests(self, timeout: float = 30.0) -> List[Dict[str, Any]]:
        """Drain queued incoming RPC requests that need the agent to answer."""
        if self.rpc_requests is None:
            raise ConfigError("node not started")
        try:
            first = await asyncio.wait_for(self.rpc_requests.get(), timeout)
        except asyncio.TimeoutError:
            return []
        out = [first]
        while True:
            try:
                out.append(self.rpc_requests.get_nowait())
            except asyncio.QueueEmpty:
                break
        return out

    async def rpc_reply(self, request_id: str, result: Any = None,
                        error: Optional[str] = None) -> Dict[str, Any]:
        """Answer a queued incoming RPC request (see wait_rpc_requests)."""
        meta = self._rpc_meta.pop(request_id, None)
        if meta is None:
            return {"ok": False, "error": f"unknown or already answered request {request_id}"}
        reply_to = meta.get("reply_to")
        if error:
            resp = {"id": request_id, "ok": False, "result": None,
                    "error": error, "ts": time.time()}
        else:
            resp = {"id": request_id, "ok": True, "result": result, "ts": time.time()}
        if reply_to:
            await self._publish(reply_to, resp)
        return {"ok": True, "request_id": request_id}

    async def _handle_ping(self, params: Any = None) -> Dict[str, Any]:
        return {"pong": True, "sid": self.sid, "role": self.role, "ts": time.time()}

    def register_handler(self, method: str, fn: Callable[[Any], Any]) -> None:
        self.handlers[method] = fn

    # ---------- control (master -> slave) ----------

    async def send_control(self, target: str, kind: str, payload: Any = None,
                           request_id: Optional[str] = None) -> str:
        rid = request_id or uuid.uuid4().hex
        await self._publish(self.t("ctrl", target), {
            "kind": kind, "payload": payload or {}, "from": self.sid,
            "request_id": rid, "ts": time.time(),
        })
        return rid

    async def pull_queued(self) -> Dict[str, Any]:
        """Non-blocking drain of everything queued for this node.

        Returns whatever has already arrived (control messages, incoming RPC
        requests, events) without waiting. The agent calls this at turn
        boundaries / after a tmux wake to see what the master sent. Messages
        remain queued until consumed here or by the blocking wait_* tools, so
        nothing is lost.
        """
        out: Dict[str, List[Dict[str, Any]]] = {"control": [], "rpc_requests": [], "events": []}
        for q, key in ((self.ctrl_queue, "control"),
                       (self.rpc_requests, "rpc_requests"),
                       (self.events, "events")):
            if q is None:
                continue
            while True:
                try:
                    out[key].append(q.get_nowait())
                except asyncio.QueueEmpty:
                    break
        return out

    async def wait_control(self, timeout: float = 30.0) -> Dict[str, Any]:
        """Block until a control message from the master arrives (or timeout).

        The mesh is asynchronous: the master may send a control message at any
        time, so the agent must NOT poll. This method waits INSIDE the call and
        returns a self-describing envelope, so "no message yet" is never
        confused with a real control message:

          {"received": true,  "message": {kind, payload, from, request_id, ts}}
          {"received": false, "reason": "timeout", "waited": <seconds>}
        """
        if self.ctrl_queue is None:
            raise ConfigError("node not started")
        try:
            msg = await asyncio.wait_for(self.ctrl_queue.get(), timeout)
        except asyncio.TimeoutError:
            return {"received": False, "reason": "timeout", "waited": timeout}
        return {"received": True, "message": msg}

    async def _on_ctrl(self, data: Dict[str, Any]) -> None:
        if self.ctrl_queue is not None:
            self.ctrl_queue.put_nowait(data)
        # ACK to the master. Prefer the learned master sid; fall back to the
        # control message's `from` field (always the master) so the master
        # reliably gets the ack even if the retained `{root}/master` message
        # has not arrived yet.
        ack_target = self.master_sid or data.get("from")
        if ack_target:
            await self._publish(self.t("ctrl_ack", ack_target), {
                "request_id": data.get("request_id"), "ok": True,
                "from": self.sid, "ts": time.time(),
            })
        # Push notification to the agent (tmux injection) so it wakes up and
        # calls pull_queued()/wait_control() instead of blocking forever.
        await self._wake(data)

    async def _wake(self, data: Dict[str, Any]) -> None:
        """Best-effort push notification: run the wake hook (if any) off-loop."""
        if self.wake_hook is None:
            return
        try:
            await asyncio.to_thread(self.wake_hook, data)
        except Exception as e:  # pragma: no cover - defensive
            sys.stderr.write(f"mux wake hook failed: {e}\n")

    async def _on_ctrl_ack(self, sid: str, data: Dict[str, Any]) -> None:
        # session_id = the slave that acked (topic suffix is the master that
        # received the ack; `from` identifies the actual acking slave).
        ack_sid = data.get("from") or sid
        info = self.registry.setdefault(ack_sid, {"sid": ack_sid})
        info["last_ctrl_ack"] = data
        await self._push_event({"kind": "ctrl_ack", "session_id": ack_sid, "ack": data})

    # ---------- status (slave -> master) ----------

    async def report_status(self, state: str, plan_files: Optional[List[str]] = None,
                            message: str = "", blocked_reason: str = "") -> Dict[str, Any]:
        self.status = {
            "sid": self.sid, "parent_id": self.parent_id, "role": self.role,
            "state": state, "plan_files": list(plan_files or []),
            "message": message, "blocked_reason": blocked_reason, "ts": time.time(),
        }
        await self._publish(self.t("status", self.sid), self.status, retain=True)
        return self.status

    # ---------- zone locks (master owned, broker authoritative) ----------

    async def zone_acquire(self, path: str, owner: Optional[str] = None,
                           force: bool = False) -> Dict[str, Any]:
        owner = owner or self.sid
        z = self.zones.get(path)
        if z and z.get("owner") and z["owner"] != owner and not force:
            if owner not in z.setdefault("queued", []):
                z["queued"].append(owner)
            await self._publish_zones()
            return {"ok": False, "path": path, "owner": z["owner"],
                    "queued": True, "note": "queue behind current owner"}
        self.zones[path] = {"owner": owner, "queued": z.get("queued", []) if z else []}
        await self._publish_zones()
        return {"ok": True, "path": path, "owner": owner}

    async def zone_release(self, path: str, owner: Optional[str] = None) -> Dict[str, Any]:
        z = self.zones.get(path)
        if not z:
            return {"ok": False, "error": f"no zone {path}"}
        if owner and z.get("owner") != owner:
            return {"ok": False, "error": f"zone {path} owned by {z.get('owner')}"}
        if z.get("queued"):
            nxt = z["queued"].pop(0)
            self.zones[path] = {"owner": nxt, "queued": z["queued"]}
            released = {"ok": True, "path": path, "next_owner": nxt}
        else:
            del self.zones[path]
            released = {"ok": True, "path": path, "next_owner": None}
        await self._publish_zones()
        return released

    async def list_zones(self) -> Dict[str, Any]:
        return {"zones": self.zones, "updated": time.time()}

    async def _publish_zones(self) -> None:
        await self._publish(self.t("zones"), {"zones": self.zones, "updated": time.time()}, retain=True)

    async def get_zone_snapshot(self) -> Dict[str, Any]:
        return self.zone_snapshot

    async def _push_event(self, ev: Dict[str, Any]) -> None:
        if self.events is not None:
            self.events.put_nowait(ev)

    # ---------- conflict feedback (all nodes report; master learns) ----------

    def _load_conflicts(self) -> Dict[str, Dict[str, Any]]:
        f = Path(self.config_dir, "conflicts.json")
        if not f.exists():
            return {}
        try:
            entries = json.loads(f.read_text())
            if isinstance(entries, list):
                return {e["id"]: e for e in entries if isinstance(e, dict) and e.get("id")}
        except Exception:
            pass
        return {}

    def _persist_conflicts(self) -> None:
        try:
            Path(self.config_dir, "conflicts.json").write_text(
                json.dumps(sorted(self.conflicts.values(), key=lambda e: e.get("ts", 0)),
                           ensure_ascii=False, indent=2) + "\n")
        except Exception as e:  # pragma: no cover - defensive
            sys.stderr.write(f"mux: could not persist conflicts: {e}\n")

    async def report_conflict(self, files: Optional[List[str]] = None,
                              description: str = "", severity: str = "medium",
                              suggestion: str = "", zone: Optional[str] = None) -> Dict[str, Any]:
        """Report a conflict (or conflict risk) to the master. The master records it,
        persists it to <config_dir>/conflicts.json and may use it to mark high-risk
        zones / serialize work, so coordination improves over time."""
        if severity not in ("low", "medium", "high", "critical"):
            severity = "medium"
        entry = {
            "id": uuid.uuid4().hex,
            "sid": self.sid,
            "files": list(files or []),
            "zone": zone,
            "description": description,
            "severity": severity,
            "suggestion": suggestion,
            "ts": time.time(),
        }
        await self._publish(self.t("conflict", self.sid), entry, retain=True)
        # local record too (a node should remember what it reported)
        with self._conflict_lock:
            self.conflicts[entry["id"]] = entry
        return {"ok": True, **entry}

    async def _on_conflict(self, sid: str, data: Dict[str, Any]) -> None:
        if self.role != "master":
            return
        if not data.get("id") and not data.get("description"):
            return  # empty retained (cleared) or malformed payload
        cid = data.get("id") or uuid.uuid4().hex
        entry = dict(data)
        entry["id"] = cid
        entry.setdefault("ts", time.time())
        with self._conflict_lock:
            self.conflicts[cid] = entry
        self._persist_conflicts()
        await self._publish_conflicts()
        await self._push_event({"kind": "conflict_reported", "session_id": sid,
                                "conflict": entry, "id": cid})

    async def _publish_conflicts(self) -> None:
        entries = sorted(self.conflicts.values(), key=lambda e: e.get("ts", 0), reverse=True)
        await self._publish(self.t("conflicts"),
                            {"conflicts": entries, "updated": time.time()}, retain=True)

    async def list_conflicts(self, limit: int = 50) -> Dict[str, Any]:
        """Master: return recorded conflict reports (newest first)."""
        entries = sorted(self.conflicts.values(), key=lambda e: e.get("ts", 0), reverse=True)
        return {"conflicts": entries[: max(1, int(limit))], "total": len(entries)}

    async def risk_zones(self) -> Dict[str, Any]:
        """Master: aggregate conflict history into per-path risk zones.

        A path is risky when conflicts were reported against it (files or zone).
        The master should serialize work on high-count paths."""
        agg: Dict[str, Dict[str, Any]] = {}
        with self._conflict_lock:
            entries = list(self.conflicts.values())
        for e in entries:
            paths = list(e.get("files") or [])
            if e.get("zone"):
                paths.append(e["zone"])
            sev = e.get("severity", "medium")
            ts = e.get("ts", 0)
            for pa in paths:
                if not pa:
                    continue
                a = agg.setdefault(pa, {"count": 0, "severities": set(), "last": 0.0})
                a["count"] += 1
                a["severities"].add(sev)
                a["last"] = max(a["last"], ts or 0)
        ranked = sorted(agg.items(), key=lambda kv: (-kv[1]["count"], -kv[1]["last"]))
        return {"risk_zones": [{"path": pa, "count": v["count"],
                                "severities": sorted(v["severities"]), "last": v["last"]}
                               for pa, v in ranked]}


# --------------------------------------------------------------------------
# CLI
# --------------------------------------------------------------------------

def _build_parser() -> argparse.ArgumentParser:
    p = argparse.ArgumentParser(prog="mux_rpc.py", description="agent-mux MQTT async RPC core")
    sub = p.add_subparsers(dest="cmd", required=True)

    common = argparse.ArgumentParser(add_help=False)
    common.add_argument("--config", default=DEFAULT_CONFIG_DIR, help="config dir (default ~/mqtt)")
    common.add_argument("--session-id", default=None, help="codex session id (default $CODEX_THREAD_ID)")
    common.add_argument("--root", default=None, help="topic root override (default: config dir minus home)")

    sp = sub.add_parser("master", parents=[common], help="run a master daemon")
    sp = sub.add_parser("slave", parents=[common], help="run a slave daemon")
    sp.add_argument("--parent-id", default=None, help="parent (master/slave) session id -> tree topology")

    for name in ("slaves", "topology", "rpc", "ping", "control", "zones", "zone-acquire", "zone-release"):
        sp = sub.add_parser(name, parents=[common], help=f"one-shot master utility: {name}")
        if name in ("rpc", "ping"):
            sp.add_argument("target")
            sp.add_argument("method", nargs="?", default="ping")
            sp.add_argument("params", nargs="?", default=None)
            sp.add_argument("--timeout", type=float, default=None)
        if name == "control":
            sp.add_argument("target")
            sp.add_argument("kind")
            sp.add_argument("payload", nargs="?", default=None)
        if name in ("zone-acquire", "zone-release"):
            sp.add_argument("path")
            sp.add_argument("--owner", default=None)
    return p


async def _cli_op(node: MuxNode, args: argparse.Namespace) -> Any:
    await node.wait_ready(15)
    await asyncio.sleep(0.6)  # let retained registry/zones/master messages arrive
    cmd = args.cmd
    if cmd in ("slaves", "topology"):
        res = await node.list_slaves()
    elif cmd == "rpc":
        params = json.loads(args.params) if args.params else None
        rid = await node.send_rpc(args.target, args.method, params, timeout=args.timeout)
        res = await node.await_result(rid, wait=args.timeout)
        res["request_id"] = rid
    elif cmd == "ping":
        rid = await node.send_rpc(args.target, "ping", timeout=args.timeout)
        res = await node.await_result(rid, wait=args.timeout)
        res["request_id"] = rid
    elif cmd == "control":
        payload = json.loads(args.payload) if args.payload else {}
        rid = await node.send_control(args.target, args.kind, payload)
        res = {"ok": True, "request_id": rid}
    elif cmd == "zones":
        res = await node.list_zones()
    elif cmd == "zone-acquire":
        res = await node.zone_acquire(args.path, owner=args.owner)
    elif cmd == "zone-release":
        res = await node.zone_release(args.path, owner=args.owner)
    else:  # pragma: no cover
        res = {"error": f"unknown command {cmd}"}
    await node._shutdown()
    return res


def main(argv: Optional[List[str]] = None) -> int:
    args = _build_parser().parse_args(argv)
    role = "slave" if args.cmd == "slave" else ("master" if args.cmd == "master" else "master")
    parent = getattr(args, "parent_id", None)
    try:
        node = MuxNode(role=role, config_dir=args.config, session_id=args.session_id,
                       parent_id=parent, root=args.root)
    except ConfigError as e:
        sys.stderr.write(f"error: {e}\n")
        return 2
    node.start()
    if args.cmd in ("master", "slave"):
        try:
            node.call(node.wait_ready(15))
            print(json.dumps({"ok": True, **node.identity()}, ensure_ascii=False))
            while True:
                time.sleep(3600)
        except KeyboardInterrupt:
            node.stop()
        return 0
    try:
        result = node.call(_cli_op(node, args), timeout=120)
        print(json.dumps(result, ensure_ascii=False, indent=2))
        return 0
    except Exception as e:
        sys.stderr.write(f"error: {e}\n")
        node.stop()
        return 1


if __name__ == "__main__":
    sys.exit(main())
