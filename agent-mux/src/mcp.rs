//! MCP (Model Context Protocol) stdio server for agent-mux.
//!
//! JSON-RPC 2.0 over stdin/stdout (one JSON object per line). All logs go to
//! stderr so they never corrupt the wire protocol. The node is created lazily:
//! the agent calls `mux_init` after the master/slave skill loads, or the
//! process auto-initializes when `--role` is given and a session id is known.

use crate::config::{load_config, resolve_session_id, Config};
use crate::node::Node;
use crate::tmux::TmuxWake;
use anyhow::Result;
use serde_json::{json, Value};
use std::sync::{Arc, Mutex};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

pub const SERVER_NAME: &str = "agent-mux";

/// Global (per-process) node handle. `None` until `mux_init`/auto-init.
pub fn global_node() -> &'static Mutex<Option<Arc<Node>>> {
    static NODE: Mutex<Option<Arc<Node>>> = Mutex::new(None);
    &NODE
}

// ---------------------------------------------------------------------------
// result helpers
// ---------------------------------------------------------------------------

fn text_result(v: Value) -> Value {
    let text = serde_json::to_string_pretty(&v).unwrap_or_else(|_| "null".to_string());
    json!({"content": [{"type": "text", "text": text}], "isError": false})
}

fn text_error(msg: impl Into<String>) -> Value {
    json!({"content": [{"type": "text", "text": msg.into()}], "isError": true})
}

// ---------------------------------------------------------------------------
// argument extraction
// ---------------------------------------------------------------------------

fn args_of(params: &Value) -> Value {
    params
        .get("arguments")
        .cloned()
        .filter(|v| v.is_object())
        .unwrap_or_else(|| json!({}))
}

fn arg_str(args: &Value, key: &str) -> Option<String> {
    args.get(key)
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .filter(|s| !s.is_empty())
}

fn arg_str_or<'a>(args: &'a Value, key: &str, default: &'a str) -> String {
    arg_str(args, key).unwrap_or_else(|| default.to_string())
}

fn arg_f64(args: &Value, key: &str, default: f64) -> f64 {
    args.get(key).and_then(|v| v.as_f64()).unwrap_or(default)
}

fn arg_i64(args: &Value, key: &str, default: i64) -> i64 {
    args.get(key).and_then(|v| v.as_i64()).unwrap_or(default)
}

fn arg_bool(args: &Value, key: &str, default: bool) -> bool {
    args.get(key).and_then(|v| v.as_bool()).unwrap_or(default)
}

fn arg_str_array(args: &Value, key: &str) -> Option<Vec<String>> {
    args.get(key)
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|x| x.as_str().map(|s| s.to_string()))
                .collect()
        })
}

fn arg_value(args: &Value, key: &str) -> Option<Value> {
    args.get(key).cloned().filter(|v| !v.is_null())
}

fn require_node() -> Result<Arc<Node>, String> {
    let g = global_node().lock().unwrap();
    match g.as_ref() {
        Some(n) => Ok(n.clone()),
        None => Err(
            "agent-mux node not initialized: call mux_init(role='master'|'slave', \
             session_id=<your Codex session id>) first (after the skill loads)"
                .to_string(),
        ),
    }
}

// ---------------------------------------------------------------------------
// tool handlers
// ---------------------------------------------------------------------------

async fn handle_mux_init(args: &Value) -> Result<Value, String> {
    let role = arg_str(args, "role").ok_or("mux_init: role is required ('master' or 'slave')")?;
    if role != "master" && role != "slave" {
        return Err(format!("mux_init: invalid role {role:?} (expected master|slave)"));
    }
    let session_id = arg_str(args, "session_id");
    let parent_id = arg_str(args, "parent_id");
    let master_sid = arg_str(args, "master_sid");
    let config_dir = arg_str_or(args, "config_dir", crate::config::DEFAULT_CONFIG_DIR);
    let root = arg_str(args, "root");
    let tmux_pane = arg_str(args, "tmux_pane");

    let sid = resolve_session_id(session_id.as_deref())
        .map_err(|e| e.to_string() + " Ask the agent for its Codex session id; never invent one.")?;
    let conf: Config = load_config(&config_dir, root.as_deref()).map_err(|e| e.to_string())?;

    // Reuse an existing node when role + session id match (e.g. the process
    // auto-initialized at startup with the same session). Reconfiguring instead
    // of stopping/recreating avoids clearing the retained hb/registry on the
    // broker, which would make the master see a spurious offline blip.
    let reuse = {
        let g = global_node().lock().unwrap();
        g.as_ref()
            .filter(|n| n.role == role && n.sid == sid)
            .map(|n| n.clone())
    };
    if let Some(old) = reuse {
        old.reconfigure(parent_id, master_sid).await;
        let mut out = old.identity().await;
        if let Some(m) = out.as_object_mut() {
            m.insert("ok".to_string(), json!(true));
            m.insert("reused".to_string(), json!(true));
        }
        return Ok(out);
    }

    // Different identity: stop any previous node before replacing it.
    if let Some(old) = global_node().lock().unwrap().take() {
        old.stop().await;
    }

    let wake = TmuxWake::detect(tmux_pane).map(|pane| Arc::new(TmuxWake::new(pane)));
    let node = Node::start(
        &role,
        &sid,
        parent_id,
        master_sid,
        &conf.root,
        &config_dir,
        &conf,
        wake,
    )
    .await
    .map_err(|e| {
        format!(
            "could not connect to MQTT broker: {e}. Is the broker running? \
             See agent-mux-master/scripts/docker-compose.yml (docker compose up -d)."
        )
    })?;
    *global_node().lock().unwrap() = Some(node.clone());
    let mut out = node.identity().await;
    if let Some(m) = out.as_object_mut() {
        m.insert("ok".to_string(), json!(true));
    }
    Ok(out)
}

async fn handle_mux_status(args: &Value) -> Result<Value, String> {
    let node = require_node()?;
    let _ = args;
    let ident = node.identity().await;
    let (known_slaves, pending, zones) = {
        let s = node.state.lock().await;
        (
            if node.role == "master" { s.registry.len() } else { 0 },
            s.pending.len(),
            s.zones.len(),
        )
    };
    let mut out = ident;
    if let Some(m) = out.as_object_mut() {
        m.insert("known_slaves".to_string(), json!(known_slaves));
        m.insert("pending_rpcs".to_string(), json!(pending));
        m.insert("zones".to_string(), json!(zones));
    }
    Ok(out)
}

async fn handle_topology(args: &Value) -> Result<Value, String> {
    let node = require_node()?;
    let _ = args;
    Ok(node.topology().await)
}

async fn handle_wait_events(args: &Value) -> Result<Value, String> {
    let node = require_node()?;
    let timeout = arg_f64(args, "timeout", 30.0);
    Ok(Value::Array(node.wait_events(timeout).await))
}

async fn handle_mux_pull(args: &Value) -> Result<Value, String> {
    let node = require_node()?;
    let _ = args;
    Ok(node.pull_queued().await)
}

async fn handle_wait_control(args: &Value) -> Result<Value, String> {
    let node = require_node()?;
    let timeout = arg_f64(args, "timeout", 30.0);
    Ok(node.wait_control(timeout).await)
}

async fn handle_wait_rpc_requests(args: &Value) -> Result<Value, String> {
    let node = require_node()?;
    let timeout = arg_f64(args, "timeout", 30.0);
    Ok(node.wait_rpc_requests(timeout).await)
}

async fn handle_send_rpc(args: &Value) -> Result<Value, String> {
    let node = require_node()?;
    let target = arg_str(args, "target").ok_or("send_rpc: target is required")?;
    let method = arg_str(args, "method").ok_or("send_rpc: method is required")?;
    let params = arg_value(args, "params");
    let timeout = arg_value(args, "timeout").and_then(|v| v.as_f64());
    let rid = node
        .send_rpc(&target, &method, params, timeout)
        .await
        .map_err(|e| e.to_string())?;
    Ok(json!({"ok": true, "request_id": rid, "target": target, "method": method}))
}

async fn handle_get_result(args: &Value) -> Result<Value, String> {
    let node = require_node()?;
    let request_id = arg_str(args, "request_id").ok_or("get_result: request_id is required")?;
    let wait = arg_value(args, "wait").and_then(|v| v.as_f64());
    Ok(node.await_result(&request_id, wait).await)
}

async fn handle_ping(args: &Value) -> Result<Value, String> {
    let node = require_node()?;
    let target = arg_str(args, "target").ok_or("ping: target is required")?;
    let wait = arg_f64(args, "wait", 5.0);
    let rid = node
        .send_rpc(&target, "ping", None, None)
        .await
        .map_err(|e| e.to_string())?;
    Ok(node.await_result(&rid, Some(wait)).await)
}

async fn handle_list_pending(args: &Value) -> Result<Value, String> {
    let node = require_node()?;
    let _ = args;
    Ok(node.list_pending().await)
}

async fn handle_retry(args: &Value) -> Result<Value, String> {
    let node = require_node()?;
    let request_id = arg_str(args, "request_id").ok_or("retry: request_id is required")?;
    Ok(node.retry(&request_id).await)
}

async fn handle_cancel(args: &Value) -> Result<Value, String> {
    let node = require_node()?;
    let request_id = arg_str(args, "request_id").ok_or("cancel: request_id is required")?;
    Ok(node.cancel(&request_id).await)
}

async fn handle_rpc_reply(args: &Value) -> Result<Value, String> {
    let node = require_node()?;
    let request_id = arg_str(args, "request_id").ok_or("rpc_reply: request_id is required")?;
    let result = arg_value(args, "result");
    let error = arg_str(args, "error");
    Ok(node.rpc_reply(&request_id, result, error).await)
}

async fn handle_send_control(args: &Value) -> Result<Value, String> {
    let node = require_node()?;
    let target = arg_str(args, "target").ok_or("send_control: target is required")?;
    let kind = arg_str(args, "kind").ok_or("send_control: kind is required")?;
    let payload = arg_value(args, "payload");
    let rid = node
        .send_control(&target, &kind, payload)
        .await
        .map_err(|e| e.to_string())?;
    Ok(json!({"ok": true, "request_id": rid, "target": target, "kind": kind}))
}

async fn handle_report_status(args: &Value) -> Result<Value, String> {
    let node = require_node()?;
    let state = arg_str(args, "state").ok_or("report_status: state is required")?;
    let plan_files = arg_str_array(args, "plan_files");
    let message = arg_str_or(args, "message", "");
    let blocked_reason = arg_str_or(args, "blocked_reason", "");
    Ok(node
        .report_status(&state, plan_files, &message, &blocked_reason)
        .await)
}

async fn handle_list_zones(args: &Value) -> Result<Value, String> {
    let node = require_node()?;
    let _ = args;
    Ok(node.list_zones().await)
}

async fn handle_zone_acquire(args: &Value) -> Result<Value, String> {
    let node = require_node()?;
    let path = arg_str(args, "path").ok_or("zone_acquire: path is required")?;
    let owner = arg_str(args, "owner");
    let force = arg_bool(args, "force", false);
    Ok(node.zone_acquire(&path, owner, force).await)
}

async fn handle_zone_release(args: &Value) -> Result<Value, String> {
    let node = require_node()?;
    let path = arg_str(args, "path").ok_or("zone_release: path is required")?;
    let owner = arg_str(args, "owner");
    Ok(node.zone_release(&path, owner).await)
}

async fn handle_get_zone_snapshot(args: &Value) -> Result<Value, String> {
    let node = require_node()?;
    let _ = args;
    Ok(node.get_zone_snapshot().await)
}

async fn handle_report_conflict(args: &Value) -> Result<Value, String> {
    let node = require_node()?;
    let files = arg_str_array(args, "files");
    let description = arg_str_or(args, "description", "");
    let severity = arg_str_or(args, "severity", "medium");
    let suggestion = arg_str_or(args, "suggestion", "");
    let zone = arg_str(args, "zone");
    Ok(node
        .report_conflict(files, &description, &severity, &suggestion, zone)
        .await)
}

async fn handle_list_conflicts(args: &Value) -> Result<Value, String> {
    let node = require_node()?;
    let limit = arg_i64(args, "limit", 50);
    Ok(node.list_conflicts(limit).await)
}

async fn handle_risk_zones(args: &Value) -> Result<Value, String> {
    let node = require_node()?;
    let _ = args;
    Ok(node.risk_zones().await)
}

// ---------------------------------------------------------------------------
// tool registry + schemas
// ---------------------------------------------------------------------------

struct Tool {
    name: &'static str,
    description: &'static str,
    schema: Value,
}

fn tools() -> Vec<Tool> {
    vec![
        Tool {
            name: "mux_init",
            description: "Initialize the agent-mux node and connect to the MQTT broker. \
Must be called once after the master/slave skill loads; the role is decided by the skill \
(agent-mux-master -> 'master', agent-mux-slave -> 'slave'). session_id defaults to \
$CODEX_THREAD_ID; if that is unset the agent must provide it (ask the user for the Codex \
session id, never invent one). parent_id (slave only) makes the mesh a tree. tmux_pane is \
optional (auto-detected). Returns the node identity.",
            schema: json!({
                "type": "object",
                "properties": {
                    "role": {"type": "string", "description": "'master' or 'slave'"},
                    "session_id": {"type": ["string", "null"], "description": "Codex session id (default $CODEX_THREAD_ID)"},
                    "parent_id": {"type": ["string", "null"], "description": "Parent node session id (slave tree)"},
                    "config_dir": {"type": ["string", "null"], "description": "Config dir holding mqtt.conf (default ~/mqtt)"},
                    "root": {"type": ["string", "null"], "description": "MQTT topic root override"},
                    "master_sid": {"type": ["string", "null"], "description": "Known master session id (slave only)"},
                    "tmux_pane": {"type": ["string", "null"], "description": "tmux pane id for wake injection (auto-detected)"}
                },
                "required": ["role"]
            }),
        },
        Tool {
            name: "mux_status",
            description: "Return node identity plus a compact summary of known slaves, pending RPCs and zones.",
            schema: json!({"type": "object", "properties": {}}),
        },
        Tool {
            name: "topology",
            description: "Return the slave tree (session ids + parent ids) known to the master.",
            schema: json!({"type": "object", "properties": {}}),
        },
        Tool {
            name: "wait_events",
            description: "Wait for mesh events; blocks until at least one arrives (or timeout). Returns the queued \
events as a list ([] on timeout). Master events: slave_joined, slave_left, status, ctrl_ack, rpc_request, \
conflict_reported. Slave events: rpc_request. Call ONCE when you are ready to react; do not busy-loop.",
            schema: json!({
                "type": "object",
                "properties": {"timeout": {"type": ["number", "null"], "description": "seconds to wait (default 30)"}}
            }),
        },
        Tool {
            name: "mux_pull",
            description: "Non-blocking: return all messages already queued for this node. Returns \
{\"control\": [...], \"rpc_requests\": [...], \"events\": [...]} without waiting. Call this at turn \
boundaries, or when a tmux wake tells you the master sent something. Messages stay queued until consumed \
here or by the blocking wait_* tools, so nothing is lost.",
            schema: json!({"type": "object", "properties": {}}),
        },
        Tool {
            name: "wait_control",
            description: "Wait for the next control message from the master (blocks inside the call). The mesh is \
asynchronous: master messages can arrive at any time, so do NOT poll. Call this ONCE when you genuinely need the \
master's input; it blocks until a control message arrives or the timeout elapses. Returns \
{\"received\": true, \"message\": {kind, payload, from, request_id, ts}} or \
{\"received\": false, \"reason\": \"timeout\", \"waited\": <seconds>}.",
            schema: json!({
                "type": "object",
                "properties": {"timeout": {"type": ["number", "null"], "description": "seconds to wait (default 30)"}}
            }),
        },
        Tool {
            name: "wait_rpc_requests",
            description: "Wait for incoming RPC requests; blocks until at least one (or timeout). Each item has \
request_id / method / params / from. Answer with rpc_reply(). Returns [] on timeout. Call ONCE when ready to \
answer; do not busy-loop.",
            schema: json!({
                "type": "object",
                "properties": {"timeout": {"type": ["number", "null"], "description": "seconds to wait (default 30)"}}
            }),
        },
        Tool {
            name: "send_rpc",
            description: "Send an async RPC request to target (session id) and return a request id. Non-blocking: \
the result arrives later via get_result() / list_pending().",
            schema: json!({
                "type": "object",
                "properties": {
                    "target": {"type": "string"},
                    "method": {"type": "string"},
                    "params": {"description": "arbitrary JSON params"},
                    "timeout": {"type": ["number", "null"], "description": "expiry seconds (default from config)"}
                },
                "required": ["target", "method"]
            }),
        },
        Tool {
            name: "get_result",
            description: "Return the result of a previously sent RPC; wait (seconds) if still pending.",
            schema: json!({
                "type": "object",
                "properties": {
                    "request_id": {"type": "string"},
                    "wait": {"type": ["number", "null"], "description": "seconds to wait if pending"}
                },
                "required": ["request_id"]
            }),
        },
        Tool {
            name: "ping",
            description: "Send an RPC 'ping' to target and wait for the reply (liveness check).",
            schema: json!({
                "type": "object",
                "properties": {
                    "target": {"type": "string"},
                    "wait": {"type": ["number", "null"], "description": "seconds to wait (default 5)"}
                },
                "required": ["target"]
            }),
        },
        Tool {
            name: "list_pending",
            description: "List pending RPC requests the master has sent but not yet completed.",
            schema: json!({"type": "object", "properties": {}}),
        },
        Tool {
            name: "retry",
            description: "Re-publish a pending/failed RPC request (retry it).",
            schema: json!({
                "type": "object",
                "properties": {"request_id": {"type": "string"}},
                "required": ["request_id"]
            }),
        },
        Tool {
            name: "cancel",
            description: "Cancel a pending RPC request.",
            schema: json!({
                "type": "object",
                "properties": {"request_id": {"type": "string"}},
                "required": ["request_id"]
            }),
        },
        Tool {
            name: "rpc_reply",
            description: "Answer an incoming RPC request previously seen via wait_rpc_requests().",
            schema: json!({
                "type": "object",
                "properties": {
                    "request_id": {"type": "string"},
                    "result": {"description": "arbitrary JSON result"},
                    "error": {"type": ["string", "null"]}
                },
                "required": ["request_id"]
            }),
        },
        Tool {
            name: "send_control",
            description: "Send a control message (master -> slave). kind is free-form, e.g. 'pause'.",
            schema: json!({
                "type": "object",
                "properties": {
                    "target": {"type": "string"},
                    "kind": {"type": "string"},
                    "payload": {"description": "arbitrary JSON payload"}
                },
                "required": ["target", "kind"]
            }),
        },
        Tool {
            name: "report_status",
            description: "Report this slave's status to the master (state + touched files + message). Call with \
state='planning' / 'ready' / 'working' / 'blocked' / 'done' and plan_files = the concrete files you intend to \
modify when you are ready to coordinate, so the master can schedule work and avoid conflicts.",
            schema: json!({
                "type": "object",
                "properties": {
                    "state": {"type": "string"},
                    "plan_files": {"type": ["array", "null"], "items": {"type": "string"}},
                    "message": {"type": ["string", "null"]},
                    "blocked_reason": {"type": ["string", "null"]}
                },
                "required": ["state"]
            }),
        },
        Tool {
            name: "list_zones",
            description: "List the zone-lock registry (which paths are owned by which session).",
            schema: json!({"type": "object", "properties": {}}),
        },
        Tool {
            name: "zone_acquire",
            description: "Acquire the lock for a path/zone (default owner = this node).",
            schema: json!({
                "type": "object",
                "properties": {
                    "path": {"type": "string"},
                    "owner": {"type": ["string", "null"]},
                    "force": {"type": ["boolean", "null"], "description": "steal the lock (default false)"}
                },
                "required": ["path"]
            }),
        },
        Tool {
            name: "zone_release",
            description: "Release the lock for a path/zone (only the owner may release).",
            schema: json!({
                "type": "object",
                "properties": {
                    "path": {"type": "string"},
                    "owner": {"type": ["string", "null"]}
                },
                "required": ["path"]
            }),
        },
        Tool {
            name: "get_zone_snapshot",
            description: "Return the last retained zone snapshot observed on the bus.",
            schema: json!({"type": "object", "properties": {}}),
        },
        Tool {
            name: "report_conflict",
            description: "Report a conflict (or a conflict risk) to the master. Call this when your edits collide \
with another slave's work, or when you detect a high-risk overlap. files = concrete paths involved, zone = an \
optional shared path/zone name, severity = low|medium|high|critical, suggestion = how the master should adjust \
(e.g. 'serialize zone X'). The master records and persists the report and uses it to mark risk zones and \
serialize work, so coordination improves over time.",
            schema: json!({
                "type": "object",
                "properties": {
                    "files": {"type": ["array", "null"], "items": {"type": "string"}},
                    "description": {"type": ["string", "null"]},
                    "severity": {"type": ["string", "null"], "description": "low|medium|high|critical"},
                    "suggestion": {"type": ["string", "null"]},
                    "zone": {"type": ["string", "null"]}
                }
            }),
        },
        Tool {
            name: "list_conflicts",
            description: "Master: list recorded conflict reports (newest first).",
            schema: json!({
                "type": "object",
                "properties": {"limit": {"type": ["integer", "null"], "description": "max entries (default 50)"}}
            }),
        },
        Tool {
            name: "risk_zones",
            description: "Master: aggregate conflict history into per-path risk zones. Paths with more conflict \
reports rank higher; the master should serialize work on high-count paths and treat them as conflict-risk zones.",
            schema: json!({"type": "object", "properties": {}}),
        },
    ]
}

// ---------------------------------------------------------------------------
// JSON-RPC dispatch
// ---------------------------------------------------------------------------

fn rpc_response(id: &Value, result: Value) -> Value {
    json!({"jsonrpc": "2.0", "id": id.clone(), "result": result})
}

fn rpc_error(id: &Value, code: i64, message: &str) -> Value {
    json!({"jsonrpc": "2.0", "id": id.clone(), "error": {"code": code, "message": message}})
}

async fn dispatch_tool(name: &str, args: &Value) -> Value {
    let res: Result<Value, String> = match name {
        "mux_init" => handle_mux_init(args).await,
        "mux_status" => handle_mux_status(args).await,
        "topology" => handle_topology(args).await,
        "wait_events" => handle_wait_events(args).await,
        "mux_pull" => handle_mux_pull(args).await,
        "wait_control" => handle_wait_control(args).await,
        "wait_rpc_requests" => handle_wait_rpc_requests(args).await,
        "send_rpc" => handle_send_rpc(args).await,
        "get_result" => handle_get_result(args).await,
        "ping" => handle_ping(args).await,
        "list_pending" => handle_list_pending(args).await,
        "retry" => handle_retry(args).await,
        "cancel" => handle_cancel(args).await,
        "rpc_reply" => handle_rpc_reply(args).await,
        "send_control" => handle_send_control(args).await,
        "report_status" => handle_report_status(args).await,
        "list_zones" => handle_list_zones(args).await,
        "zone_acquire" => handle_zone_acquire(args).await,
        "zone_release" => handle_zone_release(args).await,
        "get_zone_snapshot" => handle_get_zone_snapshot(args).await,
        "report_conflict" => handle_report_conflict(args).await,
        "list_conflicts" => handle_list_conflicts(args).await,
        "risk_zones" => handle_risk_zones(args).await,
        other => return text_error(format!("unknown tool: {other}")),
    };
    match res {
        Ok(v) => text_result(v),
        Err(e) => text_error(e),
    }
}

async fn handle_message(msg: Value) -> Option<Value> {
    let method = msg.get("method").and_then(|m| m.as_str()).unwrap_or("");
    let id = msg.get("id").cloned().unwrap_or(Value::Null);
    let is_notification = id.is_null();

    match method {
        "initialize" => {
            let protocol_version = msg
                .get("params")
                .and_then(|p| p.get("protocolVersion"))
                .and_then(|v| v.as_str())
                .unwrap_or("2025-03-26")
                .to_string();
            let result = json!({
                "protocolVersion": protocol_version,
                "capabilities": {"tools": {}},
                "serverInfo": {"name": SERVER_NAME, "version": env!("CARGO_PKG_VERSION")}
            });
            Some(rpc_response(&id, result))
        }
        "notifications/initialized" | "notifications/cancelled" => None,
        "notifications/exit" => None, // handled by the loop before dispatch
        "tools/list" => {
            let tools: Vec<Value> = tools()
                .into_iter()
                .map(|t| {
                    json!({
                        "name": t.name,
                        "description": t.description,
                        "inputSchema": t.schema,
                    })
                })
                .collect();
            Some(rpc_response(&id, json!({"tools": tools})))
        }
        "tools/call" => {
            let name = msg
                .get("params")
                .and_then(|p| p.get("name"))
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let params = msg.get("params").cloned().unwrap_or_else(|| json!({}));
            let args = args_of(&params);
            let result = dispatch_tool(&name, &args).await;
            Some(rpc_response(&id, result))
        }
        _ if is_notification => None,
        _ => Some(rpc_error(&id, -32601, &format!("Method not found: {method}"))),
    }
}

// ---------------------------------------------------------------------------
// server loop
// ---------------------------------------------------------------------------

pub async fn run(initial: Option<Arc<Node>>) {
    if let Some(n) = initial {
        *global_node().lock().unwrap() = Some(n);
    }
    let stdin = tokio::io::stdin();
    let mut reader = BufReader::new(stdin);
    let stdout = tokio::io::stdout();
    let mut writer = tokio::io::BufWriter::new(stdout);
    let mut line = String::new();
    let mut exiting = false;

    loop {
        line.clear();
        let n = match reader.read_line(&mut line).await {
            Ok(n) => n,
            Err(e) => {
                eprintln!("agent-mux: stdin error: {e}");
                break;
            }
        };
        if n == 0 {
            // EOF: codex closed the pipe
            break;
        }
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let msg: Value = match serde_json::from_str(trimmed) {
            Ok(v) => v,
            Err(e) => {
                eprintln!("agent-mux: bad json-rpc line: {e}");
                continue;
            }
        };
        if msg.get("method").and_then(|m| m.as_str()) == Some("notifications/exit") {
            exiting = true;
        }
        if let Some(resp) = handle_message(msg).await {
            let out = serde_json::to_string(&resp).unwrap_or_else(|_| "{}".to_string());
            if writer.write_all(out.as_bytes()).await.is_err() {
                break;
            }
            if writer.write_all(b"\n").await.is_err() {
                break;
            }
            if writer.flush().await.is_err() {
                break;
            }
        }
        if exiting {
            break;
        }
    }

    // graceful shutdown: publish hb offline flag, clear retained registry, stop the node
    if let Some(n) = global_node().lock().unwrap().take() {
        n.stop().await;
    }
    let _ = writer.flush().await;
}

/// Expose the tool list for tests.
#[allow(dead_code)]
pub fn tool_names() -> Vec<String> {
    tools().into_iter().map(|t| t.name.to_string()).collect()
}
