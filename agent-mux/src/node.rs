//! MQTT mesh node: one process == one node (master or slave).
//!
//! Mirrors the Python reference (`mux_rpc.py`): registry/heartbeat
//! topics, async RPC with a pending registry, control/ack, status, zone locks
//! and conflict feedback. All mutable state lives behind a tokio Mutex shared
//! between the MQTT event-loop task, background tasks (heartbeat/sweep/
//! watchdog) and MCP tool handlers.

use crate::config::{now_ts, Config};
use crate::wake::Wake;
use anyhow::{anyhow, Result};
use rumqttc::{AsyncClient, Event, EventLoop, LastWill, MqttOptions, Packet, QoS};
use serde_json::{json, Map, Value};
use std::collections::{HashMap, VecDeque};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::{Mutex, Notify};
use tokio::task::JoinHandle;

pub struct Pending {
    pub request_id: String,
    pub target: String,
    pub method: String,
    pub params: Value,
    pub status: String,
    pub attempts: u32,
    pub created_at: f64,
    pub expires_at: f64,
    pub result: Value,
    pub error: Value,
    pub notify: Arc<Notify>,
}

impl Pending {
    fn to_json(&self) -> Value {
        json!({
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
        })
    }
}

#[derive(Clone)]
pub struct RpcMeta {
    pub reply_to: Option<String>,
    #[allow(dead_code)]
    pub from: Option<String>,
}

pub struct State {
    pub registry: HashMap<String, Value>,
    pub pending: HashMap<String, Pending>,
    pub rpc_meta: HashMap<String, RpcMeta>,
    pub ctrl_queue: VecDeque<Value>,
    pub events: VecDeque<Value>,
    pub rpc_requests: VecDeque<Value>,
    pub zones: HashMap<String, Value>,
    pub zone_snapshot: Value,
    /// Master-held event subscriptions: watch_id -> {watch_id, watcher_sid,
    /// kind, filter, ttl, created_at, expires_at}. Only the master stores these;
    /// slaves just publish watch/reg and receive watch/evt/{sid}.
    pub watches: HashMap<String, Value>,
    /// Events the master routed back to this watcher (drained by mux_pull into
    /// the `watch` array).
    pub watch_events: VecDeque<Value>,
    pub status: Value,
    pub conflicts: HashMap<String, Value>,
    /// Task table (master): task id -> task json (`id`, `kind`,
    /// `target_crates`, `files`, `owner`, `state`, `depends_on`,
    /// `created_at`, `updated_at`). Written by `assign` control messages and
    /// updated from slaves' `report_status(task=...)`.
    pub tasks: HashMap<String, Value>,
    /// Stable insertion order of task ids (master).
    pub task_order: Vec<String>,
    /// Approval escalations awaiting the master agent's `approval_decide`
    /// (master). Entries: `req_id`, `files`, `owner`, `level`, `reason`,
    /// `reply_to`, `ts`.
    pub escalations: VecDeque<Value>,
    /// Approval decision trace (master): auto-approvals and
    /// `approval_decide` outcomes. Used for the digest trace and revocation.
    pub approvals: HashMap<String, Value>,
    /// High-water mark for `mux_digest`'s incremental `since` filter
    /// (persisted so restarts don't replay noise).
    pub last_digest_ts: f64,
    /// Monotonic event sequence number (persisted).
    pub event_seq: u64,
    pub connected: bool,
    pub subscribed: bool,
    pub shutting_down: bool,
    pub master_sid: Option<String>,
    pub last_state_change: f64,
}

impl State {
    fn new(master_sid: Option<String>, config_dir: &str) -> Self {
        // Persisted coordination state (tasks / zones / conflicts / digest
        // high-water mark) is restored on restart so scheduling decisions and
        // the digest `since` increment survive a node restart.
        let (tasks, task_order, zones, conflicts, escalations, approvals, last_digest_ts, event_seq) =
            load_state_from_disk(config_dir);
        State {
            registry: HashMap::new(),
            pending: HashMap::new(),
            rpc_meta: HashMap::new(),
            ctrl_queue: VecDeque::new(),
            events: VecDeque::new(),
            rpc_requests: VecDeque::new(),
            zones,
            zone_snapshot: Value::Null,
            watches: HashMap::new(),
            watch_events: VecDeque::new(),
            status: Value::Null,
            conflicts,
            tasks,
            task_order,
            escalations,
            approvals,
            last_digest_ts,
            event_seq,
            connected: false,
            subscribed: false,
            shutting_down: false,
            master_sid,
            last_state_change: now_ts(),
        }
    }
}

#[derive(Clone, Copy)]
enum QueueKind {
    Events,
    Ctrl,
    RpcRequests,
}

impl QueueKind {
    fn get<'a>(&self, s: &'a mut State) -> &'a mut VecDeque<Value> {
        match self {
            QueueKind::Events => &mut s.events,
            QueueKind::Ctrl => &mut s.ctrl_queue,
            QueueKind::RpcRequests => &mut s.rpc_requests,
        }
    }
}

pub struct Node {
    pub role: String,
    pub sid: String,
    pub parent_id: std::sync::Mutex<Option<String>>,
    pub root: String,
    pub config_dir: String,
    pub conf: Config,
    pub state: Arc<Mutex<State>>,
    pub ctrl_notify: Arc<Notify>,
    pub events_notify: Arc<Notify>,
    pub rpc_notify: Arc<Notify>,
    pub watch_notify: Arc<Notify>,
    pub ready_notify: Arc<Notify>,
    client: Option<AsyncClient>,
    tasks: std::sync::Mutex<Vec<JoinHandle<()>>>,
    pub wake: Option<Arc<dyn Wake>>,
}

fn qos(v: u8) -> QoS {
    match v {
        0 => QoS::AtMostOnce,
        2 => QoS::ExactlyOnce,
        _ => QoS::AtLeastOnce,
    }
}

impl Node {
    /// Create the node: build the MQTT client, spawn the event-loop/heartbeat/
    /// sweep/watchdog tasks, then wait until the broker connection is up.
    pub async fn start(
        role: &str,
        sid: &str,
        parent_id: Option<String>,
        master_sid: Option<String>,
        root: &str,
        config_dir: &str,
        conf: &Config,
        wake: Option<Arc<dyn Wake>>,
    ) -> Result<Arc<Node>> {
        let cid = format!(
            "mux-{}-{}-{}-{}",
            role,
            &sid[..sid.len().min(8)],
            std::process::id(),
            uuid::Uuid::new_v4().simple().to_string().get(0..6).unwrap_or("x")
        );
        let mut opts = MqttOptions::new(cid, conf.host.clone(), conf.port);
        opts.set_keep_alive(Duration::from_secs(conf.keepalive));
        opts.set_clean_session(true);
        opts.set_request_channel_capacity(1024);
        // LWT: abrupt loss publishes an offline flag on the retained hb topic.
        let hb_topic = format!("{}/hb/{}", root.trim_matches('/'), sid);
        let will_payload = json!({"sid": sid, "role": role, "status": "offline",
                                  "reason": "connection_lost", "ts": now_ts()});
        let will = LastWill::new(
            hb_topic,
            serde_json::to_vec(&will_payload).unwrap_or_default(),
            qos(conf.qos),
            true,
        );
        opts.set_last_will(will);
        let (client, eventloop) = AsyncClient::new(opts, 100);

        let state = State::new(master_sid, config_dir);
        let node = Arc::new(Node {
            role: role.to_string(),
            sid: sid.to_string(),
            parent_id: std::sync::Mutex::new(parent_id),
            root: root.trim_matches('/').to_string(),
            config_dir: config_dir.to_string(),
            conf: conf.clone(),
            state: Arc::new(Mutex::new(state)),
            ctrl_notify: Arc::new(Notify::new()),
            events_notify: Arc::new(Notify::new()),
            rpc_notify: Arc::new(Notify::new()),
            watch_notify: Arc::new(Notify::new()),
            ready_notify: Arc::new(Notify::new()),
            client: Some(client),
            tasks: std::sync::Mutex::new(Vec::new()),
            wake,
        });
        Self::spawn_event_loop_thread(node.clone(), eventloop);
        if node.role == "slave" {
            node.spawn_heartbeat();
        } else {
            node.spawn_sweep();
        }
        node.spawn_watchdog();
        if let Err(e) = node.wait_ready(15.0).await {
            node.stop().await;
            return Err(e);
        }
        Ok(node)
    }

    // ---- topics ----

    pub fn topic(&self, parts: &[&str]) -> String {
        let mut v: Vec<&str> = vec![self.root.as_str()];
        v.extend(parts.iter().copied().filter(|p| !p.is_empty()));
        v.join("/")
    }

    // ---- publish ----

    async fn publish(&self, topic: &str, payload: Option<Value>, retain: bool) {
        let Some(client) = &self.client else { return };
        let bytes = match payload {
            Some(v) => serde_json::to_vec(&v).unwrap_or_default(),
            None => Vec::new(),
        };
        if let Err(e) = client
            .publish(topic.to_string(), qos(self.conf.qos), retain, bytes)
            .await
        {
            eprintln!("agent-mux: publish {topic} failed: {e}");
        }
    }

    async fn publish_zones(&self) {
        let zones = self.zones_obj().await;
        self.publish(
            &self.topic(&["zones"]),
            Some(json!({"zones": zones, "updated": now_ts()})),
            true,
        )
        .await;
    }

    async fn zones_obj(&self) -> Value {
        let s = self.state.lock().await;
        let mut m = Map::new();
        for (k, v) in &s.zones {
            m.insert(k.clone(), v.clone());
        }
        Value::Object(m)
    }

    // ---- identity ----

    pub fn parent_id(&self) -> Option<String> {
        self.parent_id.lock().unwrap().clone()
    }

    /// Update role parameters (parent id / master sid) on an already-started
    /// node and re-announce so the retained registry carries the new parent.
    /// Used when `mux_init` matches the auto-initialized node instead of
    /// stopping/recreating it (which would clear retained hb/registry and make
    /// the master see a spurious offline blip).
    pub async fn reconfigure(self: &Arc<Node>, parent_id: Option<String>, master_sid: Option<String>) {
        {
            let mut p = self.parent_id.lock().unwrap();
            *p = parent_id;
        }
        {
            let mut s = self.state.lock().await;
            if let Some(m) = master_sid {
                s.master_sid = Some(m);
            }
        }
        self.announce().await;
    }

    pub async fn identity(&self) -> Value {
        let s = self.state.lock().await;
        json!({
            "role": self.role,
            "session_id": self.sid,
            "parent_id": self.parent_id(),
            "root": self.root,
            "config_dir": self.config_dir,
            "broker": format!("{}:{}", self.conf.host, self.conf.port),
            "master_session_id": s.master_sid,
            "connected": s.connected,
            "status": s.status,
            "wake": self.wake.as_ref().map(|w| w.name()).unwrap_or("none"),
        })
    }

    // ---- background tasks ----

    /// The rumqttc `EventLoop` is not `Sync`, so it cannot live inside
    /// `Arc<Node>` (which is shared across `tokio::spawn`). A dedicated
    /// `std::thread` owns the event loop and runs its own current-thread tokio
    /// runtime; the `AsyncClient` sender kept in the node drives it.
    fn spawn_event_loop_thread(node: Arc<Node>, mut eventloop: EventLoop) {
        std::thread::spawn(move || {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("agent-mux: build event-loop runtime");
            rt.block_on(async move {
                loop {
                    match eventloop.poll().await {
                        Ok(Event::Incoming(Packet::ConnAck(_))) => node.on_connected().await,
                        Ok(Event::Incoming(Packet::SubAck(_))) => {
                            // Broker has registered our subscriptions: only now can
                            // peers' publishes be guaranteed to reach us.
                            node.state.lock().await.subscribed = true;
                            node.ready_notify.notify_one();
                        }
                        Ok(Event::Incoming(Packet::Publish(p))) => {
                            let topic = p.topic.clone();
                            let payload = p.payload.to_vec();
                            node.on_message(&topic, &payload).await;
                        }
                        Ok(_) => {}
                        Err(e) => {
                            let shutting = {
                                let mut s = node.state.lock().await;
                                if s.connected {
                                    s.connected = false;
                                    s.last_state_change = now_ts();
                                }
                                s.shutting_down
                            };
                            if shutting {
                                break;
                            }
                            eprintln!("agent-mux: mqtt error: {e}");
                            tokio::time::sleep(Duration::from_millis(500)).await;
                        }
                    }
                }
            });
        });
    }

    fn spawn_heartbeat(self: &Arc<Node>) {
        let node = self.clone();
        let task_node = node.clone();
        let handle = tokio::spawn(async move {
            loop {
                tokio::time::sleep(Duration::from_secs_f64(task_node.conf.hb_interval.max(0.1))).await;
                if task_node.state.lock().await.shutting_down {
                    break;
                }
                if task_node.state.lock().await.connected {
                    let st = task_node.state.lock().await.status.clone();
                    let state = st.get("state").and_then(|v| v.as_str()).unwrap_or("idle");
                    task_node.publish(
                        &task_node.topic(&["hb", task_node.sid.as_str()]),
                        Some(json!({
                            "sid": task_node.sid,
                            "parent_id": task_node.parent_id(),
                            "role": task_node.role,
                            "state": state,
                            "status": "online",
                            "ts": now_ts(),
                        })),
                        true,
                    )
                    .await;
                }
            }
        });
        node.tasks.lock().unwrap().push(handle);
    }

    fn spawn_sweep(self: &Arc<Node>) {
        let node = self.clone();
        let task_node = node.clone();
        let handle = tokio::spawn(async move {
            loop {
                tokio::time::sleep(Duration::from_secs_f64((task_node.conf.hb_timeout / 2.0).max(1.0)))
                    .await;
                if task_node.state.lock().await.shutting_down {
                    break;
                }
                let now = now_ts();
                let mut left: Vec<(String, String)> = Vec::new();
                let mut has_action = !task_node.conf.digest_mode;
                let retry: Vec<(String, String, String, Value)> = {
                    let mut s = task_node.state.lock().await;
                    let offline: Vec<String> = s
                        .registry
                        .iter()
                        .filter(|(sid, info)| {
                            **sid != task_node.sid
                                && info.get("status").and_then(|v| v.as_str()) == Some("online")
                                && now - info.get("last_seen").and_then(|v| v.as_f64()).unwrap_or(0.0)
                                    > task_node.conf.hb_timeout
                        })
                        .map(|(k, _)| k.clone())
                        .collect();
                    for sid in &offline {
                        if let Some(info) = s.registry.get_mut(sid) {
                            info["status"] = json!("offline");
                            info["offline_reason"] = json!("heartbeat_timeout");
                        }
                        let ev = json!({
                            "kind": "slave_left",
                            "session_id": sid,
                            "reason": "heartbeat_timeout"
                        });
                        if task_node.wake_for(&s, &ev) {
                            has_action = true;
                        }
                        Node::push_event(&mut s, ev);
                        left.push((sid.clone(), "heartbeat_timeout".to_string()));
                    }
                    // Master cleanup: drop watches of gone slaves and expire
                    // watches whose ttl has passed.
                    if !offline.is_empty() {
                        s.watches.retain(|_, w| {
                            w.get("watcher_sid").and_then(|v| v.as_str())
                                .map(|ws| !offline.iter().any(|sid| sid == ws))
                                .unwrap_or(true)
                        });
                    }
                    s.watches.retain(|_, w| {
                        let exp = w.get("expires_at").and_then(|v| v.as_f64()).unwrap_or(0.0);
                        exp == 0.0 || exp > now
                    });
                    // A5: auto-retry expired pending RPCs with linear backoff
                    // (`rpc_timeout * attempts`). Re-publishing happens after
                    // the lock is released, so `s` is not held across await.
                    s.pending
                        .iter_mut()
                        .filter(|(_, p)| p.status == "pending" && p.expires_at > 0.0 && p.expires_at < now)
                        .map(|(rid, p)| {
                            p.attempts += 1;
                            p.expires_at = now + task_node.conf.rpc_timeout * p.attempts as f64;
                            p.notify = Arc::new(Notify::new());
                            (
                                rid.clone(),
                                p.target.clone(),
                                p.method.clone(),
                                p.params.clone(),
                            )
                        })
                        .collect()
                };
                if !left.is_empty() {
                    task_node.events_notify.notify_one();
                    if has_action {
                        task_node.wake();
                    }
                }
                for (rid, target, method, params) in retry {
                    task_node
                        .publish(
                            &task_node.topic(&["rpc", "req", target.as_str()]),
                            Some(json!({
                                "id": rid,
                                "method": method,
                                "params": params,
                                "reply_to": task_node.topic(&["rpc", "resp", task_node.sid.as_str()]),
                                "from": task_node.sid,
                                "ts": now_ts(),
                            })),
                            false,
                        )
                        .await;
                }
            }
        });
        node.tasks.lock().unwrap().push(handle);
    }

    /// Liveness watchdog: auto-exit when the parent (codex) dies, or when the
    /// MQTT link stays down too long. Runs in a background task so it never
    /// blocks codex.
    fn spawn_watchdog(self: &Arc<Node>) {
        let node = self.clone();
        let task_node = node.clone();
        let handle = tokio::spawn(async move {
            let ppid = unsafe { libc::getppid() };
            let exit_after = (task_node.conf.hb_timeout * 4.0).max(60.0);
            loop {
                tokio::time::sleep(Duration::from_secs(2)).await;
                if task_node.state.lock().await.shutting_down {
                    break;
                }
                let parent_alive = unsafe { libc::kill(ppid as i32, 0) == 0 };
                if !parent_alive {
                    eprintln!("agent-mux: parent process ({ppid}) exited; cleaning up and exiting");
                    task_node.stop().await;
                    std::process::exit(0);
                }
                let (connected, last_change) = {
                    let s = task_node.state.lock().await;
                    (s.connected, s.last_state_change)
                };
                if !connected && now_ts() - last_change > exit_after {
                    eprintln!("agent-mux: MQTT link down for too long; exiting");
                    task_node.stop().await;
                    std::process::exit(0);
                }
            }
        });
        node.tasks.lock().unwrap().push(handle);
    }

    // ---- connection handling ----

    async fn on_connected(self: &Arc<Node>) {
        {
            let mut s = self.state.lock().await;
            s.connected = true;
            s.subscribed = false;
            s.last_state_change = now_ts();
        }
        if let Some(client) = &self.client {
            if self.role == "master" {
                let t = self.topic(&["#"]);
                if let Err(e) = client.subscribe(t.clone(), qos(self.conf.qos)).await {
                    eprintln!("agent-mux: subscribe {t} failed: {e}");
                }
            } else {
                let subs = [
                    self.topic(&["ctrl", self.sid.as_str()]),
                    self.topic(&["rpc", "req", self.sid.as_str()]),
                    self.topic(&["rpc", "resp", self.sid.as_str()]),
                    self.topic(&["master"]),
                    self.topic(&["zones"]),
                    self.topic(&["watch", "evt", self.sid.as_str()]),
                ];
                for t in subs {
                    if let Err(e) = client.subscribe(t.clone(), qos(self.conf.qos)).await {
                        eprintln!("agent-mux: subscribe {t} failed: {e}");
                    }
                }
            }
        }
        self.announce().await;
        self.ready_notify.notify_one();
    }

    async fn announce(self: &Arc<Node>) {
        let now = now_ts();
        self.publish(
            &self.topic(&["registry", self.sid.as_str()]),
            Some(json!({"sid": self.sid, "parent_id": self.parent_id(), "role": self.role, "joined_at": now})),
            true,
        )
        .await;
        if self.role == "master" {
            self.publish(
                &self.topic(&["master"]),
                Some(json!({"sid": self.sid, "role": "master", "ts": now})),
                true,
            )
            .await;
            self.publish_zones().await;
        } else {
            let st = self.state.lock().await.status.clone();
            let state = st.get("state").and_then(|v| v.as_str()).unwrap_or("idle");
            self.publish(
                &self.topic(&["hb", self.sid.as_str()]),
                Some(json!({"sid": self.sid, "parent_id": self.parent_id(), "role": self.role,
                            "state": state, "status": "online", "ts": now})),
                true,
            )
            .await;
        }
    }

    pub async fn wait_ready(&self, timeout: f64) -> Result<()> {
        let deadline = Instant::now() + Duration::from_secs_f64(timeout.max(0.0));
        loop {
            {
                let s = self.state.lock().await;
                if s.connected && s.subscribed {
                    return Ok(());
                }
            }
            let fut = self.ready_notify.notified();
            if Instant::now() >= deadline {
                return Err(anyhow!("timeout waiting for MQTT connection"));
            }
            tokio::select! {
                _ = fut => {}
                _ = tokio::time::sleep_until(tokio::time::Instant::from_std(deadline)) => {
                    return Err(anyhow!("timeout waiting for MQTT connection"));
                }
            }
        }
    }

    // ---- message routing ----

    async fn on_message(self: &Arc<Node>, topic: &str, payload: &[u8]) {
        let parts: Vec<&str> = topic.split('/').collect();
        if parts.is_empty() || parts[0] != self.root {
            return;
        }
        let rel = &parts[1..];
        let data: Value = serde_json::from_slice(payload).unwrap_or(Value::Null);
        let data_obj_empty = match &data {
            Value::Object(m) => m.is_empty(),
            _ => true,
        };
        if rel.len() == 2 {
            match rel[0] {
                "registry" => self.on_registry(rel[1], &data, data_obj_empty).await,
                "hb" => self.on_hb(rel[1], &data, data_obj_empty).await,
                "status" => self.on_status(rel[1], &data).await,
                "conflict" => self.on_conflict(rel[1], &data).await,
                "watch" if rel[1] == "reg" => self.on_watch_reg(&data).await,
                "ctrl" if rel[1] == self.sid => self.on_ctrl(&data).await,
                "ctrl_ack" if self.role == "master" => self.on_ctrl_ack(rel[1], &data).await,
                _ => {}
            }
        } else if rel.len() == 3 {
            if rel[0] == "rpc" {
                if rel[1] == "req" && rel[2] == self.sid {
                    self.on_rpc_request(&data).await;
                } else if rel[1] == "resp" && rel[2] == self.sid {
                    self.on_rpc_response(&data).await;
                }
            } else if rel[0] == "watch" && rel[1] == "evt" && rel[2] == self.sid {
                self.on_watch_evt(&data).await;
            }
        } else if rel.len() == 1 {
            match rel[0] {
                "master" => {
                    if let Some(sid) = data.get("sid").and_then(|v| v.as_str()) {
                        if sid != self.sid {
                            self.state.lock().await.master_sid = Some(sid.to_string());
                        }
                    }
                }
                "zones" => {
                    let bus_zones: Option<serde_json::Map<String, Value>> = data
                        .get("zones")
                        .and_then(|v| v.as_object())
                        .cloned();
                    self.state.lock().await.zone_snapshot = data.clone();
                    if self.role == "master" {
                        // Master is authoritative for zone ownership and never
                        // adopts owners from the bus: a stale retained echo (e.g.
                        // from a previous run) must not block a fresh acquire.
                        // It only merges queued waiters into zones it already
                        // manages, so a release can hand the zone to the next
                        // slave that queued behind it.
                        if let Some(bus) = bus_zones {
                            let mut s = self.state.lock().await;
                            for (k, v) in bus {
                                if let Some(local) = s.zones.get_mut(&k) {
                                    let entry = local.clone();
                                    let mut q: Vec<Value> = entry
                                        .get("queued")
                                        .and_then(|x| x.as_array())
                                        .cloned()
                                        .unwrap_or_default();
                                    if let Some(qv) = v.get("queued").and_then(|x| x.as_array()) {
                                        for item in qv {
                                            if !q.iter().any(|x| x == item) {
                                                q.push(item.clone());
                                            }
                                        }
                                    }
                                    if let Some(obj) = local.as_object_mut() {
                                        obj.insert("queued".to_string(), Value::Array(q));
                                    }
                                }
                            }
                        }
                    } else if let Some(z) = bus_zones {
                        let mut zones = HashMap::new();
                        for (k, v) in z {
                            zones.insert(k.clone(), v.clone());
                        }
                        self.state.lock().await.zones = zones;
                    }
                }
                _ => {}
            }
        }
    }

    fn push_event(s: &mut State, ev: Value) {
        // Every event carries a monotonic-ish timestamp + sequence number so
        // `mux_digest` can filter incrementally (`since`) and restarts don't
        // replay noise (the high-water mark is persisted).
        let mut ev = ev;
        if ev.get("ts").is_none()
            && let Some(o) = ev.as_object_mut()
        {
            o.insert("ts".to_string(), json!(now_ts()));
        }
        s.event_seq += 1;
        s.events.push_back(ev);
    }

    /// Whether this event should wake the agent: in digest mode only Action
    /// events do (empty wakes are dropped); in legacy mode every event wakes
    /// as before (opt-out / grayscale rollback).
    fn wake_for(&self, s: &State, ev: &Value) -> bool {
        wake_needed(self.conf.digest_mode, classify_event(s, ev))
    }

    async fn on_registry(self: &Arc<Node>, sid: &str, data: &Value, empty: bool) {
        if sid == self.sid || self.role != "master" {
            return;
        }
        let mut s = self.state.lock().await;
        if empty {
            let info = s
                .registry
                .entry(sid.to_string())
                .or_insert_with(|| json!({"sid": sid}));
            info["status"] = json!("offline");
            info["offline_reason"] = json!("unregistered");
            info["last_seen"] = json!(now_ts());
            s.watches.retain(|_, w| {
                w.get("watcher_sid").and_then(|v| v.as_str()) != Some(sid)
            });
            return;
        }
        let new = !s.registry.contains_key(sid);
        let mut info = data.clone();
        info["last_seen"] = json!(now_ts());
        if info.get("status").is_none() {
            info["status"] = json!("online");
        }
        let online = info.get("status").and_then(|v| v.as_str()) == Some("online");
        let parent = info.get("parent_id").cloned().unwrap_or(Value::Null);
        s.registry.insert(sid.to_string(), info.clone());
        if new && online {
            let ev = json!({"kind": "slave_joined", "session_id": sid, "parent_id": parent, "info": info});
            let wake_up = self.wake_for(&s, &ev);
            Self::push_event(&mut s, ev);
            let wake = self.wake.clone();
            drop(s);
            self.events_notify.notify_one();
            if wake_up
                && let Some(w) = wake
            {
                w.wake();
            }
            return;
        }
    }

    /// Liveness: the single source of truth for slave presence.
    ///
    /// A non-empty online payload refreshes the slave's registry entry. An
    /// empty payload (retained hb cleared) or `status == "offline"` (graceful
    /// shutdown flag or LWT after abrupt loss) marks the slave offline
    /// immediately and clears the retained registry entry. The offline ts
    /// comes from the payload (not now) so a stale tombstone cannot refresh
    /// last_seen for a node that is actually gone.
    async fn on_hb(self: &Arc<Node>, sid: &str, data: &Value, empty: bool) {
        if sid == self.sid || self.role != "master" {
            return;
        }
        let offline = empty || data.get("status").and_then(|v| v.as_str()) == Some("offline");
        if offline {
            let ts = data.get("ts").and_then(|v| v.as_f64()).unwrap_or_else(now_ts);
            let reason = data
                .get("reason")
                .and_then(|v| v.as_str())
                .unwrap_or("hb_offline")
                .to_string();
            let mut s = self.state.lock().await;
            let info = s.registry.entry(sid.to_string()).or_insert_with(|| json!({"sid": sid}));
            info["status"] = json!("offline");
            info["offline_reason"] = json!(reason);
            info["last_seen"] = json!(ts);
            let ev = json!({"kind": "slave_left", "session_id": sid, "reason": reason});
            let wake_up = self.wake_for(&s, &ev);
            Self::push_event(&mut s, ev);
            s.watches.retain(|_, w| {
                w.get("watcher_sid").and_then(|v| v.as_str()) != Some(sid)
            });
            drop(s);
            self.publish(&self.topic(&["registry", sid]), None, true).await;
            self.events_notify.notify_one();
            if wake_up {
                self.wake();
            }
            return;
        }
        let joined = {
            let mut s = self.state.lock().await;
            let info = s.registry.entry(sid.to_string()).or_insert_with(|| json!({"sid": sid}));
            let was_online = info.get("status").and_then(|v| v.as_str()) == Some("online");
            info["last_seen"] = json!(now_ts());
            info["status"] = json!("online");
            if info.get("parent_id").is_none() {
                if let Some(p) = data.get("parent_id") {
                    info["parent_id"] = p.clone();
                }
            }
            if info.get("role").is_none() {
                if let Some(r) = data.get("role") {
                    info["role"] = r.clone();
                }
            }
            let cur = info.get("state").cloned().unwrap_or(json!("unknown"));
            info["state"] = data.get("state").cloned().unwrap_or(cur);
            !was_online
        };
        if joined {
            let mut s = self.state.lock().await;
            let info = s.registry.get(sid).cloned().unwrap_or_else(|| json!({"sid": sid}));
            let parent = info.get("parent_id").cloned().unwrap_or(Value::Null);
            let ev = json!({"kind": "slave_joined", "session_id": sid, "parent_id": parent, "info": info});
            let wake_up = self.wake_for(&s, &ev);
            Self::push_event(&mut s, ev);
            let wake = self.wake.clone();
            drop(s);
            self.events_notify.notify_one();
            if wake_up
                && let Some(w) = wake
            {
                w.wake();
            }
        }
    }

    async fn on_status(self: &Arc<Node>, sid: &str, data: &Value) {
        if sid == self.sid || self.role != "master" {
            return;
        }
        let mut wake_up = !self.conf.digest_mode;
        let mut task_changed = false;
        let mut releases: Vec<(String, Value)> = Vec::new();
        {
            let mut s = self.state.lock().await;
            let info = s
                .registry
                .entry(sid.to_string())
                .or_insert_with(|| json!({"sid": sid, "last_seen": json!(now_ts())}));
            info["last_seen"] = json!(now_ts());
            for k in [
                "state",
                "plan_files",
                "message",
                "blocked_reason",
                "parent_id",
                "role",
                "task_id",
                "task_kind",
                "target_crates",
                "files",
            ] {
                if let Some(v) = data.get(k) {
                    info[k] = v.clone();
                }
            }
            let ev = json!({"kind": "status", "session_id": sid, "info": info.clone()});
            if self.wake_for(&s, &ev) {
                wake_up = true;
            }
            Self::push_event(&mut s, ev);
            // A2: a status carrying a task id updates the task table. The
            // reported state drives the task state machine (working -> Working,
            // done -> Done, error/failed -> Failed); blocked/conflict stay
            // Working because the owner is still on the task, just waiting.
            if let Some(tid) = data.get("task_id").and_then(|v| v.as_str()) {
                if let Some(task) = s.tasks.get_mut(tid) {
                    let new_state = task_state_from_status(
                        data.get("state").and_then(|v| v.as_str()).unwrap_or(""),
                    );
                    let old_state = task.get("state").and_then(|v| v.as_str()).unwrap_or("");
                    if new_state != old_state {
                        task["state"] = json!(new_state);
                        task["updated_at"] = json!(now_ts());
                        let tev = json!({
                            "kind": "task",
                            "task_id": tid,
                            "state": new_state,
                            "owner": sid,
                            "ts": now_ts(),
                        });
                        if self.wake_for(&s, &tev) {
                            wake_up = true;
                        }
                        Self::push_event(&mut s, tev);
                        task_changed = true;
                    }
                }
                // A2: recompute readiness (promote Scheduled tasks whose deps
                // cleared / global slot freed) and auto-release their owners.
                releases = recompute_tasks(&mut s);
                if !releases.is_empty() {
                    task_changed = true;
                }
            }
        }
        if task_changed {
            self.persist_state().await;
        }
        self.events_notify.notify_one();
        if wake_up {
            self.wake();
        }
        for (owner, payload) in releases {
            let _ = self.send_control(&owner, "release", Some(payload)).await;
        }
    }

    async fn on_ctrl(self: &Arc<Node>, data: &Value) {
        {
            let mut s = self.state.lock().await;
            s.ctrl_queue.push_back(data.clone());
        }
        self.ctrl_notify.notify_one();
        let ack_target = {
            let s = self.state.lock().await;
            s.master_sid.clone().or_else(|| {
                data.get("from").and_then(|v| v.as_str()).map(|x| x.to_string())
            })
        };
        if let Some(t) = ack_target {
            self.publish(
                &self.topic(&["ctrl_ack", t.as_str()]),
                Some(json!({
                    "request_id": data.get("request_id").cloned().unwrap_or(Value::Null),
                    "ok": true,
                    "from": self.sid,
                    "ts": now_ts(),
                })),
                false,
            )
            .await;
        }
        self.wake();
    }

    async fn on_ctrl_ack(self: &Arc<Node>, sid: &str, data: &Value) {
        let ack_sid = data
            .get("from")
            .and_then(|v| v.as_str())
            .unwrap_or(sid)
            .to_string();
        // A ctrl_ack is an echo (Noise): in digest mode it must not wake the
        // agent, only advance the digest counters.
        let mut wake_up = !self.conf.digest_mode;
        {
            let mut s = self.state.lock().await;
            let info = s
                .registry
                .entry(ack_sid.clone())
                .or_insert_with(|| json!({"sid": ack_sid}));
            info["last_ctrl_ack"] = data.clone();
            let ev = json!({"kind": "ctrl_ack", "session_id": ack_sid, "ack": data.clone()});
            if self.wake_for(&s, &ev) {
                wake_up = true;
            }
            Self::push_event(&mut s, ev);
        }
        self.events_notify.notify_one();
        if wake_up {
            self.wake();
        }
    }

    async fn on_rpc_request(self: &Arc<Node>, data: &Value) {
        let rid = data.get("id").and_then(|v| v.as_str()).unwrap_or("").to_string();
        let method = data.get("method").and_then(|v| v.as_str()).unwrap_or("").to_string();
        let reply_to = data.get("reply_to").and_then(|v| v.as_str()).map(|x| x.to_string());
        if method == "ping" {
            let resp = json!({
                "id": rid,
                "ok": true,
                "result": {"pong": true, "sid": self.sid, "role": self.role, "ts": now_ts()},
                "ts": now_ts(),
            });
            if let Some(rt) = reply_to {
                self.publish(&rt, Some(resp), false).await;
            }
            return;
        }
        // Zone ownership is master-decided: a slave asks via the `zone_request`
        // RPC and the master node answers against its authoritative zone
        // registry (grant when free / FIFO queue when held / release when the
        // requester owns it). It is a mechanical lock decision, so it is not
        // queued for the master agent — the master's registry is the decider.
        if self.role == "master" && method == "zone_request" {
            let params = data.get("params").cloned().unwrap_or(Value::Null);
            let path = params
                .get("path")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let release = params.get("release").and_then(|v| v.as_bool()).unwrap_or(false);
            let requester = data.get("from").and_then(|v| v.as_str()).unwrap_or("").to_string();
            let result = if path.is_empty() || requester.is_empty() {
                json!({"ok": false, "error": "zone_request: path and requester are required"})
            } else if release {
                self.zone_release(&path, Some(requester.clone())).await
            } else {
                self.zone_acquire(&path, Some(requester.clone()), false).await
            };
            let resp = json!({"id": rid, "ok": true, "result": result, "ts": now_ts()});
            if let Some(rt) = reply_to {
                self.publish(&rt, Some(resp), false).await;
            }
            return;
        }
        // A3: `may_i_touch` is a mechanical five-level impact check. The master
        // node auto-answers the risk-free cases (never-touched + no zone +
        // no conflict history, or same-owner repeat) and escalates everything
        // risky into the approval queue for the master agent, which arbitrates
        // with `approval_decide`. Escalated requests are NOT answered here.
        if self.role == "master" && method == "may_i_touch" {
            let params = data.get("params").cloned().unwrap_or(Value::Null);
            let files: Vec<String> = params
                .get("files")
                .and_then(|v| v.as_array())
                .map(|arr| {
                    arr.iter()
                        .filter_map(|x| x.as_str().map(|s| s.to_string()))
                        .collect()
                })
                .unwrap_or_default();
            let requester = data.get("from").and_then(|v| v.as_str()).unwrap_or("").to_string();
            let mut wake_up = !self.conf.digest_mode;
            let mut answer: Option<Value> = None;
            let mut state_changed = false;
            {
                let mut s = self.state.lock().await;
                match check_may_i_touch(&s, &files, &requester) {
                    MayIResult::AutoApproved => {
                        let rec = json!({
                            "req_id": rid.clone(),
                            "files": files,
                            "owner": requester,
                            "level": 0,
                            "reason": "auto",
                            "auto": true,
                            "approved": true,
                            "ts": now_ts(),
                        });
                        s.approvals.insert(rid.clone(), rec.clone());
                        state_changed = true;
                        // Trace stays visible in the digest but never wakes.
                        Self::push_event(&mut s, json!({"kind": "approval_trace", "approval": rec}));
                        answer = Some(json!({"approved": true, "auto": true, "req_id": rid}));
                    }
                    MayIResult::Denied => {
                        answer = Some(json!({
                            "approved": false,
                            "auto": false,
                            "reason": "file claimed by another owner"
                        }));
                    }
                    MayIResult::Escalated { level } => {
                        let entry = json!({
                            "req_id": rid.clone(),
                            "files": files,
                            "owner": requester,
                            "level": level,
                            "reason": format!("may_i_touch escalation level {level}"),
                            "reply_to": reply_to.clone(),
                            "ts": now_ts(),
                        });
                        s.escalations.push_back(entry.clone());
                        state_changed = true;
                        let ev = json!({
                            "kind": "approval_escalation",
                            "req_id": rid,
                            "owner": requester,
                            "level": level,
                            "files": files,
                        });
                        if self.wake_for(&s, &ev) {
                            wake_up = true;
                        }
                        Self::push_event(&mut s, ev);
                    }
                }
            }
            if state_changed {
                self.persist_state().await;
            }
            if let Some(ans) = answer {
                let resp = json!({"id": rid, "ok": true, "result": ans, "ts": now_ts()});
                if let Some(rt) = reply_to {
                    self.publish(&rt, Some(resp), false).await;
                }
            }
            if wake_up {
                self.events_notify.notify_one();
                self.wake();
            }
            return;
        }
        let req = json!({
            "request_id": rid,
            "method": method,
            "params": data.get("params").cloned().unwrap_or(Value::Null),
            "from": data.get("from").cloned().unwrap_or(Value::Null),
            "reply_to": reply_to,
            "ts": now_ts(),
        });
        let mut wake_up = !self.conf.digest_mode;
        {
            let mut s = self.state.lock().await;
            s.rpc_meta.insert(
                rid.clone(),
                RpcMeta {
                    reply_to,
                    from: data.get("from").and_then(|v| v.as_str()).map(|x| x.to_string()),
                },
            );
            s.rpc_requests.push_back(req.clone());
            let ev = json!({"kind": "rpc_request", "request_id": rid, "method": method,
                           "from": data.get("from").cloned().unwrap_or(Value::Null)});
            if self.wake_for(&s, &ev) {
                wake_up = true;
            }
            Self::push_event(&mut s, ev);
        }
        self.rpc_notify.notify_one();
        self.events_notify.notify_one();
        if wake_up {
            self.wake();
        }
    }

    async fn on_rpc_response(self: &Arc<Node>, data: &Value) {
        let rid = data.get("id").and_then(|v| v.as_str()).unwrap_or("").to_string();
        let mut s = self.state.lock().await;
        let Some(p) = s.pending.get_mut(&rid) else { return };
        let ok = data.get("ok").and_then(|v| v.as_bool()).unwrap_or(false);
        if ok {
            p.status = "done".to_string();
            p.result = data.get("result").cloned().unwrap_or(Value::Null);
            p.error = Value::Null;
        } else {
            p.status = "failed".to_string();
            p.error = data.get("error").cloned().unwrap_or(json!("rpc error"));
        }
        let notify = p.notify.clone();
        drop(s);
        notify.notify_one();
    }

    // ---- queue wait helpers ----

    async fn wait_queue(state: &Mutex<State>, kind: QueueKind, notify: &Notify, timeout: f64) -> Vec<Value> {
        let deadline = Instant::now() + Duration::from_secs_f64(timeout.max(0.0));
        loop {
            {
                let mut s = state.lock().await;
                let q = kind.get(&mut s);
                if !q.is_empty() {
                    let mut out = Vec::new();
                    while let Some(v) = q.pop_front() {
                        out.push(v);
                    }
                    return out;
                }
            }
            if Instant::now() >= deadline {
                return Vec::new();
            }
            // Register the notification future BEFORE re-checking the queue so a
            // notify that lands between the check and registration cannot be lost.
            // The MCP loop is sequential (one waiter per queue), so notify_one's
            // stored permit semantics also make a missed wakeup self-healing.
            let fut = notify.notified();
            {
                let mut s = state.lock().await;
                let q = kind.get(&mut s);
                if !q.is_empty() {
                    let mut out = Vec::new();
                    while let Some(v) = q.pop_front() {
                        out.push(v);
                    }
                    return out;
                }
            }
            tokio::select! {
                _ = fut => {}
                _ = tokio::time::sleep_until(tokio::time::Instant::from_std(deadline)) => {
                    return Vec::new();
                }
            }
        }
    }

    pub async fn wait_events(&self, timeout: f64) -> Vec<Value> {
        Self::wait_queue(&self.state, QueueKind::Events, &self.events_notify, timeout).await
    }

    // ---- async rpc ----

    pub async fn send_rpc(&self, target: &str, method: &str, params: Option<Value>, timeout: Option<f64>) -> Result<String> {
        let timeout = timeout.unwrap_or(self.conf.rpc_timeout);
        let rid = uuid::Uuid::new_v4().simple().to_string();
        let now = now_ts();
        let p = Pending {
            request_id: rid.clone(),
            target: target.to_string(),
            method: method.to_string(),
            params: params.clone().unwrap_or(Value::Null),
            status: "pending".to_string(),
            attempts: 1,
            created_at: now,
            expires_at: now + timeout,
            result: Value::Null,
            error: Value::Null,
            notify: Arc::new(Notify::new()),
        };
        {
            let mut s = self.state.lock().await;
            s.pending.insert(rid.clone(), p);
        }
        self.publish(
            &self.topic(&["rpc", "req", target]),
            Some(json!({
                "id": rid,
                "method": method,
                "params": params.unwrap_or(Value::Null),
                "reply_to": self.topic(&["rpc", "resp", self.sid.as_str()]),
                "from": self.sid,
                "ts": now,
            })),
            false,
        )
        .await;
        Ok(rid)
    }

    pub async fn await_result(&self, request_id: &str, wait: Option<f64>) -> Value {
        let (notify, base_timeout) = {
            let s = self.state.lock().await;
            match s.pending.get(request_id) {
                None => return json!({"status": "unknown", "request_id": request_id}),
                Some(p) => {
                    if p.status != "pending" {
                        return json!({"status": p.status, "result": p.result, "error": p.error,
                                      "request_id": request_id, "attempts": p.attempts});
                    }
                    let t = (p.expires_at - now_ts()).max(0.0);
                    (p.notify.clone(), t)
                }
            }
        };
        let timeout = wait.unwrap_or(base_timeout);
        let deadline = Instant::now() + Duration::from_secs_f64(timeout.max(0.0));
        loop {
            let fut = notify.notified();
            {
                let s = self.state.lock().await;
                if let Some(p) = s.pending.get(request_id) {
                    if p.status != "pending" {
                        return json!({"status": p.status, "result": p.result, "error": p.error,
                                      "request_id": request_id, "attempts": p.attempts});
                    }
                }
            }
            if Instant::now() >= deadline {
                let s = self.state.lock().await;
                if let Some(p) = s.pending.get(request_id) {
                    return json!({"status": "pending", "request_id": request_id,
                                  "attempts": p.attempts, "expires_at": p.expires_at});
                }
                return json!({"status": "unknown", "request_id": request_id});
            }
            tokio::select! {
                _ = fut => {}
                _ = tokio::time::sleep_until(tokio::time::Instant::from_std(deadline)) => {
                    let s = self.state.lock().await;
                    if let Some(p) = s.pending.get(request_id) {
                        return json!({"status": "pending", "request_id": request_id,
                                      "attempts": p.attempts, "expires_at": p.expires_at});
                    }
                    return json!({"status": "unknown", "request_id": request_id});
                }
            }
        }
    }

    pub async fn list_pending(&self) -> Value {
        let s = self.state.lock().await;
        let mut keys: Vec<&String> = s.pending.keys().collect();
        keys.sort();
        let mut m = Map::new();
        for k in keys {
            m.insert(k.clone(), s.pending[k].to_json());
        }
        Value::Object(m)
    }

    pub async fn retry(&self, request_id: &str) -> Value {
        let (target, method, params, rid, attempts) = {
            let mut s = self.state.lock().await;
            let Some(p) = s.pending.get_mut(request_id) else {
                return json!({"ok": false, "error": format!("unknown request {request_id}")});
            };
            if p.status == "done" || p.status == "cancelled" {
                return json!({"ok": false, "error": format!("request {request_id} already {}", p.status)});
            }
            p.attempts += 1;
            p.expires_at = now_ts() + self.conf.rpc_timeout;
            p.status = "pending".to_string();
            p.error = Value::Null;
            p.notify = Arc::new(Notify::new());
            (p.target.clone(), p.method.clone(), p.params.clone(), request_id.to_string(), p.attempts)
        };
        self.publish(
            &self.topic(&["rpc", "req", target.as_str()]),
            Some(json!({
                "id": rid,
                "method": method,
                "params": params,
                "reply_to": self.topic(&["rpc", "resp", self.sid.as_str()]),
                "from": self.sid,
                "ts": now_ts(),
            })),
            false,
        )
        .await;
        json!({"ok": true, "request_id": request_id, "attempts": attempts})
    }

    pub async fn cancel(&self, request_id: &str) -> Value {
        let mut s = self.state.lock().await;
        let Some(p) = s.pending.get_mut(request_id) else {
            return json!({"ok": false, "error": format!("unknown request {request_id}")});
        };
        p.status = "cancelled".to_string();
        let notify = p.notify.clone();
        drop(s);
        notify.notify_one();
        json!({"ok": true, "request_id": request_id})
    }

    pub async fn rpc_reply(&self, request_id: &str, result: Option<Value>, error: Option<String>) -> Value {
        let meta = {
            let mut s = self.state.lock().await;
            s.rpc_meta.remove(request_id)
        };
        match meta {
            None => json!({"ok": false, "error": format!("unknown or already answered request {request_id}")}),
            Some(m) => {
                let resp = match error {
                    Some(e) => json!({"id": request_id, "ok": false, "result": Value::Null, "error": e, "ts": now_ts()}),
                    None => json!({"id": request_id, "ok": true, "result": result.unwrap_or(Value::Null), "ts": now_ts()}),
                };
                if let Some(rt) = m.reply_to {
                    self.publish(&rt, Some(resp), false).await;
                }
                json!({"ok": true, "request_id": request_id})
            }
        }
    }

    // ---- control / status ----

    pub async fn send_control(&self, target: &str, kind: &str, payload: Option<Value>) -> Result<String> {
        let rid = uuid::Uuid::new_v4().simple().to_string();
        self.publish(
            &self.topic(&["ctrl", target]),
            Some(json!({
                "kind": kind,
                "payload": payload.unwrap_or(json!({})),
                "from": self.sid,
                "request_id": rid,
                "ts": now_ts(),
            })),
            false,
        )
        .await;
        Ok(rid)
    }

    /// Publish a status report (the only 4-state discipline: `blocked` /
    /// `conflict` / `done` / `error` — no ticks, no echoes). Slaves attach the
    /// optional task identity (`task_id`, `task_kind`) and their declared write
    /// set (`target_crates`, `files`) so the master's task table and the
    /// `may_i_touch` impact check have real data.
    #[allow(clippy::too_many_arguments)] // status carries optional task identity + declared write set
    pub async fn report_status(
        &self,
        state: &str,
        plan_files: Option<Vec<String>>,
        message: &str,
        blocked_reason: &str,
        task_id: Option<String>,
        task_kind: Option<String>,
        target_crates: Option<Vec<String>>,
        files: Option<Vec<String>>,
    ) -> Value {
        let st = json!({
            "sid": self.sid,
            "parent_id": self.parent_id(),
            "role": self.role,
            "state": state,
            "plan_files": plan_files.unwrap_or_default(),
            "message": message,
            "blocked_reason": blocked_reason,
            "task_id": task_id,
            "task_kind": task_kind,
            "target_crates": target_crates.unwrap_or_default(),
            "files": files.unwrap_or_default(),
            "ts": now_ts(),
        });
        {
            let mut s = self.state.lock().await;
            s.status = st.clone();
        }
        self.publish(&self.topic(&["status", self.sid.as_str()]), Some(st.clone()), true).await;
        st
    }

    /// Legacy `mux_pull`: still drains `control` / `rpc_requests` / `watch`
    /// as before, but the raw `events` array is replaced by the digest shape
    /// `{actions, noise_counts}` so old skills keep working and the new
    /// `mux_digest` consumers get the same classification.
    pub async fn pull_queued(&self) -> Value {
        let mut s = self.state.lock().await;
        let mut control = Vec::new();
        while let Some(v) = s.ctrl_queue.pop_front() {
            control.push(v);
        }
        let mut rpc = Vec::new();
        while let Some(v) = s.rpc_requests.pop_front() {
            rpc.push(v);
        }
        let (actions, ack, tick) = drain_events(&mut s, 0.0);
        let mut watch = Vec::new();
        while let Some(v) = s.watch_events.pop_front() {
            watch.push(v);
        }
        json!({"control": control, "rpc_requests": rpc, "actions": actions,
               "noise_counts": {"ack": ack, "tick": tick}, "watch": watch})
    }

    /// A1: `mux_digest`. Drain queued events into `{actions, noise_counts}`
    /// and advance the persisted high-water mark. Pass the previous call's
    /// `since` to consume incrementally — events already consumed by an
    /// earlier digest are dropped without counting, so a restart never
    /// replays noise. `actions` are decision-first sorted (blocked /
    /// conflict / rpc_request / approval escalation before informational
    /// `done`); priority is internal only and never exposed.
    pub async fn digest(&self, since: Option<f64>) -> Value {
        let since = since.unwrap_or(0.0);
        let (actions, ack, tick, high_water) = {
            let mut s = self.state.lock().await;
            let (a, ac, tk) = drain_events(&mut s, since);
            s.last_digest_ts = now_ts();
            (a, ac, tk, s.last_digest_ts)
        };
        self.persist_state().await;
        json!({"actions": actions, "noise_counts": {"ack": ack, "tick": tick}, "since": high_water})
    }

    pub async fn wait_control(&self, timeout: f64) -> Value {
        let items = Self::wait_queue(&self.state, QueueKind::Ctrl, &self.ctrl_notify, timeout).await;
        if items.is_empty() {
            json!({"received": false, "reason": "timeout", "waited": timeout})
        } else {
            json!({"received": true, "message": items[0]})
        }
    }

    pub async fn wait_rpc_requests(&self, timeout: f64) -> Value {
        let items = Self::wait_queue(&self.state, QueueKind::RpcRequests, &self.rpc_notify, timeout).await;
        Value::Array(items)
    }

    // ---- A2: task table + dependency scheduling ----

    /// A2: `task_list` — the full task table in stable insertion order.
    pub async fn task_list(&self) -> Value {
        let s = self.state.lock().await;
        let tasks: Vec<Value> = s
            .task_order
            .iter()
            .filter_map(|id| s.tasks.get(id).cloned())
            .collect();
        json!({"tasks": tasks, "total": tasks.len()})
    }

    /// A2: `task_show` — a single task by id.
    pub async fn task_show(&self, task_id: &str) -> Value {
        let s = self.state.lock().await;
        match s.tasks.get(task_id) {
            Some(t) => json!({"ok": true, "task": t}),
            None => json!({"ok": false, "error": format!("unknown task {task_id}")}),
        }
    }

    /// A2: `task_cancel` (master only). Remove the task from the table and
    /// recompute readiness — a cancelled dependency may unblock a queued
    /// Validate, which is then auto-released.
    pub async fn task_cancel(&self, task_id: &str) -> Value {
        if self.role != "master" {
            return json!({"ok": false, "error": "task_cancel is master-only"});
        }
        let releases = {
            let mut s = self.state.lock().await;
            if !s.tasks.contains_key(task_id) {
                return json!({"ok": false, "error": format!("unknown task {task_id}")});
            }
            s.tasks.remove(task_id);
            s.task_order.retain(|id| id != task_id);
            recompute_tasks(&mut s)
        };
        self.persist_state().await;
        self.send_releases(releases).await;
        json!({"ok": true, "task_id": task_id, "cancelled": true})
    }

    /// A2: `task_force` (master only) — the agent's explicit override of the
    /// dependency graph. Sets the task state directly, then recomputes
    /// readiness (so forcing a dependency to `Done` releases its Validate).
    pub async fn task_force(&self, task_id: &str, state: &str) -> Value {
        if self.role != "master" {
            return json!({"ok": false, "error": "task_force is master-only"});
        }
        if !TASK_STATES.contains(&state) {
            return json!({"ok": false, "error": format!(
                "invalid state {state}; must be one of {}", TASK_STATES.join(", ")
            )});
        }
        let releases = {
            let mut s = self.state.lock().await;
            let Some(task) = s.tasks.get_mut(task_id) else {
                return json!({"ok": false, "error": format!("unknown task {task_id}")});
            };
            let owner = task.get("owner").cloned().unwrap_or(Value::Null);
            task["state"] = json!(state);
            task["updated_at"] = json!(now_ts());
            Node::push_event(
                &mut s,
                json!({"kind": "task", "task_id": task_id, "state": state,
                       "owner": owner, "forced": true, "ts": now_ts()}),
            );
            recompute_tasks(&mut s)
        };
        self.persist_state().await;
        self.send_releases(releases).await;
        json!({"ok": true, "task_id": task_id, "state": state, "forced": true})
    }

    /// A2: `assign` control (master only). Creates a task from the payload —
    /// which must carry `kind`, `target_crates` and `files` (missing any is
    /// an error). Dependencies are computed at assign time
    /// (`compute_depends_on`); the task starts `Ready` when it has no
    /// dependencies and a free global-serial slot, else `Scheduled`.
    /// `recompute_tasks` then auto-releases Validate tasks whose deps cleared
    /// — the agent is never asked to babysit readiness.
    pub async fn assign_task(&self, target: &str, payload: Value) -> Result<Value, String> {
        if self.role != "master" {
            return Err("assign_task is master-only".to_string());
        }
        let kind = payload.get("kind").and_then(|v| v.as_str()).unwrap_or("");
        let target_crates = payload.get("target_crates").cloned();
        let files = payload.get("files").cloned();
        if kind.is_empty() || target_crates.is_none() || files.is_none() {
            return Err(
                "assign payload must include kind, target_crates and files".to_string(),
            );
        }
        if !task_kind_valid(kind) {
            return Err(format!(
                "invalid task kind {kind}; must be one of {}",
                TASK_KINDS.join(", ")
            ));
        }
        let task_id = uuid::Uuid::new_v4().simple().to_string();
        let (releases, final_state) = {
            let mut s = self.state.lock().await;
            let depends_on = compute_depends_on(&s, kind, target_crates.as_ref().unwrap());
            let task = json!({
                "id": task_id,
                "kind": kind,
                "target_crates": target_crates.unwrap_or(json!([])),
                "files": files.unwrap_or(json!([])),
                "owner": target,
                "state": "Scheduled",
                "depends_on": depends_on,
                "created_at": now_ts(),
                "updated_at": now_ts(),
            });
            s.tasks.insert(task_id.clone(), task);
            s.task_order.push(task_id.clone());
            let rel = recompute_tasks(&mut s);
            let st = s
                .tasks
                .get(&task_id)
                .and_then(|t| t.get("state"))
                .and_then(|v| v.as_str())
                .unwrap_or("Scheduled")
                .to_string();
            (rel, st)
        };
        self.persist_state().await;
        self.send_releases(releases).await;
        Ok(json!({
            "ok": true,
            "task_id": task_id,
            "kind": kind,
            "owner": target,
            "state": final_state,
        }))
    }

    /// Send auto-release control messages (deduplicated by `recompute_tasks`,
    /// which returns each release at most once).
    async fn send_releases(&self, releases: Vec<(String, Value)>) {
        for (owner, payload) in releases {
            let _ = self.send_control(&owner, "release", Some(payload)).await;
        }
    }

    // ---- A3: approval arbitration ----

    /// A3: `approval_decide` (master only). The master agent answers an
    /// escalated `may_i_touch` request: `approve` / `deny` answer the
    /// requester via the stored `reply_to` and record a trace in
    /// `s.approvals`; `queue` keeps the escalation pending without answering.
    pub async fn approval_decide(&self, req_id: &str, decision: &str) -> Value {
        if self.role != "master" {
            return json!({"ok": false, "error": "approval_decide is master-only"});
        }
        if !matches!(decision, "approve" | "deny" | "queue") {
            return json!({"ok": false, "error": "decision must be approve|deny|queue"});
        }
        let decided = {
            let mut s = self.state.lock().await;
            let pos = s
                .escalations
                .iter()
                .position(|e| e.get("req_id").and_then(|v| v.as_str()) == Some(req_id));
            let Some(pos) = pos else {
                return json!({"ok": false, "error": format!("unknown or already decided request {req_id}")});
            };
            if decision == "queue" {
                return json!({"ok": true, "req_id": req_id, "decision": "queue",
                              "note": "escalation stays queued"});
            }
            let entry = s.escalations.remove(pos).unwrap();
            let approved = decision == "approve";
            let reply_to = entry
                .get("reply_to")
                .and_then(|v| v.as_str())
                .map(|x| x.to_string());
            let rec = json!({
                "req_id": req_id,
                "files": entry.get("files").cloned().unwrap_or(Value::Null),
                "owner": entry.get("owner").cloned().unwrap_or(Value::Null),
                "level": entry.get("level").cloned().unwrap_or(Value::Null),
                "decision": decision,
                "approved": approved,
                "ts": now_ts(),
            });
            s.approvals.insert(req_id.to_string(), rec.clone());
            Some((reply_to, approved, rec))
        };
        self.persist_state().await;
        if let Some((reply_to, approved, rec)) = decided {
            if let Some(rt) = reply_to {
                self.publish(
                    &rt,
                    Some(json!({
                        "id": req_id,
                        "ok": true,
                        "result": {"approved": approved, "req_id": req_id},
                        "ts": now_ts(),
                    })),
                    false,
                )
                .await;
            }
            return json!({"ok": true, "req_id": req_id, "approved": approved, "trace": rec});
        }
        json!({"ok": false, "error": "internal approval_decide error"})
    }

    // ---- zones ----

    pub async fn zone_acquire(&self, path: &str, owner: Option<String>, force: bool) -> Value {
        // Zone ownership is decided by the master only. A slave must not lock a
        // zone itself, and must not declare an owner; it asks the master with
        // `zone_request` instead.
        if self.role != "master" {
            return json!({
                "ok": false,
                "error": "zone_acquire is master-only: zone ownership is decided by the master; use zone_request to ask"
            });
        }
        let owner = owner.unwrap_or_else(|| self.sid.clone());
        {
            let mut s = self.state.lock().await;
            if let Some(z) = s.zones.get(path) {
                let cur = z.get("owner").and_then(|v| v.as_str()).unwrap_or("").to_string();
                if !cur.is_empty() && cur != owner && !force {
                    let mut queued = z.get("queued").and_then(|v| v.as_array()).cloned().unwrap_or_default();
                    if !queued.iter().any(|v| v.as_str() == Some(owner.as_str())) {
                        queued.push(json!(owner));
                        s.zones.insert(path.to_string(), json!({"owner": cur, "queued": queued}));
                    }
                    let cur2 = cur.clone();
                    drop(s);
                    self.publish_zones().await;
                    self.persist_state().await;
                    return json!({"ok": false, "path": path, "owner": cur2, "queued": true, "note": "queue behind current owner"});
                }
            }
            let queued = s.zones.get(path).and_then(|z| z.get("queued")).cloned().unwrap_or(json!([]));
            s.zones.insert(path.to_string(), json!({"owner": owner, "queued": queued}));
        }
        self.publish_zones().await;
        self.persist_state().await;
        json!({"ok": true, "path": path, "owner": owner})
    }

    pub async fn zone_release(&self, path: &str, owner: Option<String>) -> Value {
        // Releasing is also master-only: a slave relinquishes a zone through
        // `zone_request(path, release=true)` so the master stays authoritative.
        if self.role != "master" {
            return json!({
                "ok": false,
                "error": "zone_release is master-only: ask the master with zone_request(path, release=true)"
            });
        }
        let mut next_owner: Option<String> = None;
        {
            let mut s = self.state.lock().await;
            let Some(z) = s.zones.get(path) else {
                return json!({"ok": false, "error": format!("no zone {path}")});
            };
            let cur = z.get("owner").and_then(|v| v.as_str()).unwrap_or("").to_string();
            if let Some(o) = &owner {
                if *o != cur {
                    return json!({"ok": false, "error": format!("zone {path} owned by {cur}")});
                }
            }
            let mut queued = z.get("queued").and_then(|v| v.as_array()).cloned().unwrap_or_default();
            if !queued.is_empty() {
                let nxt = queued.remove(0);
                next_owner = nxt.as_str().map(|x| x.to_string());
                s.zones.insert(path.to_string(), json!({"owner": nxt, "queued": queued}));
            } else {
                s.zones.remove(path);
            }
        }
        self.publish_zones().await;
        // Master-produced event: a zone got unlocked (possibly handed to the
        // next queued owner). Early returns above already bailed on failure,
        // so reaching here means the release succeeded. Watchers are matched
        // and routed to {root}/watch/evt/{watcher_sid} below.
        self.emit_watch_events(
            "zone_released",
            json!({"path": path, "next_owner": next_owner.clone(), "ts": now_ts()}),
        )
        .await;
        self.persist_state().await;
        json!({"ok": true, "path": path, "next_owner": next_owner})
    }

    /// A4: `zone_steal` (master only). Force zone ownership to the master
    /// session id, breaking any deadlock. Publishes the authoritative registry
    /// and notifies watchers; used to arbitrate a stuck zone.
    pub async fn zone_steal(&self, path: &str) -> Value {
        if self.role != "master" {
            return json!({"ok": false, "error": "zone_steal is master-only"});
        }
        {
            let mut s = self.state.lock().await;
            let queued = s
                .zones
                .get(path)
                .and_then(|z| z.get("queued"))
                .cloned()
                .unwrap_or(json!([]));
            s.zones
                .insert(path.to_string(), json!({"owner": self.sid, "queued": queued}));
        }
        self.publish_zones().await;
        self.emit_watch_events(
            "zone_released",
            json!({"path": path, "next_owner": self.sid, "ts": now_ts()}),
        )
        .await;
        self.persist_state().await;
        json!({"ok": true, "path": path, "owner": self.sid, "stolen": true})
    }

    /// Slave: ask the master for zone ownership. The master's registry decides:
    /// it grants the zone when free, FIFO-queues this node behind the current
    /// owner, or (when `release` is true) releases the zone to the next queued
    /// owner if this node currently owns it. Returns the async RPC request id;
    /// await the outcome with `await_result` / `get_result`.
    pub async fn zone_request(&self, path: &str, release: bool) -> Result<Value, String> {
        let master = {
            let s = self.state.lock().await;
            s.master_sid.clone()
        }
        .ok_or_else(|| "zone_request: no master session id known yet".to_string())?;
        if master == self.sid {
            return Err(
                "zone_request is for slaves; as the master use zone_acquire / zone_release directly"
                    .to_string(),
            );
        }
        let rid = self
            .send_rpc(
                &master,
                "zone_request",
                Some(json!({"path": path, "release": release})),
                None,
            )
            .await
            .map_err(|e| e.to_string())?;
        Ok(json!({
            "ok": true,
            "request_id": rid,
            "target": master,
            "path": path,
            "release": release,
        }))
    }

    // ---- watch (event subscription) ----

    /// Register a watch for a master-produced event (slave side). Publishes
    /// `{root}/watch/reg`; the master stores it and routes matching events to
    /// `{root}/watch/evt/{watcher_sid}`. Returns the generated `watch_id`.
    pub async fn watch_register(&self, kind: &str, filter: Option<Value>, ttl: Option<f64>) -> Value {
        let watch_id = uuid::Uuid::new_v4().simple().to_string();
        let mut reg = json!({
            "watch_id": watch_id,
            "watcher_sid": self.sid,
            "kind": kind,
            "ts": now_ts(),
        });
        if let Some(f) = &filter {
            reg["filter"] = f.clone();
        }
        if let Some(t) = ttl {
            reg["ttl"] = json!(t);
        }
        self.publish(&self.topic(&["watch", "reg"]), Some(reg), false).await;
        json!({
            "ok": true,
            "watch_id": watch_id,
            "kind": kind,
            "filter": filter.unwrap_or_else(|| json!({})),
            "ttl": ttl.unwrap_or(0.0),
        })
    }

    /// Cancel a previously registered watch (slave side). Publishes
    /// `{root}/watch/reg` with `cancel: true`; the master removes the watch.
    pub async fn watch_cancel(&self, watch_id: &str) -> Value {
        self.publish(
            &self.topic(&["watch", "reg"]),
            Some(json!({
                "watch_id": watch_id,
                "watcher_sid": self.sid,
                "cancel": true,
                "ts": now_ts(),
            })),
            false,
        )
        .await;
        json!({"ok": true, "watch_id": watch_id, "canceled": true})
    }

    /// Master: register or cancel a watch from `{root}/watch/reg`.
    async fn on_watch_reg(self: &Arc<Node>, data: &Value) {
        if self.role != "master" {
            return;
        }
        let watch_id = data.get("watch_id").and_then(|v| v.as_str()).unwrap_or("").to_string();
        let watcher_sid = data.get("watcher_sid").and_then(|v| v.as_str()).unwrap_or("").to_string();
        if watch_id.is_empty() || watcher_sid.is_empty() {
            return;
        }
        let mut s = self.state.lock().await;
        if data.get("cancel").and_then(|v| v.as_bool()) == Some(true) {
            s.watches.remove(&watch_id);
            return;
        }
        let Some(kind) = data.get("kind").and_then(|v| v.as_str()) else {
            return;
        };
        let filter = data.get("filter").cloned().unwrap_or_else(|| json!({}));
        let ttl = data.get("ttl").and_then(|v| v.as_f64()).unwrap_or(0.0);
        let expires_at = if ttl > 0.0 { now_ts() + ttl } else { 0.0 };
        s.watches.insert(
            watch_id.clone(),
            json!({
                "watch_id": watch_id,
                "watcher_sid": watcher_sid,
                "kind": kind,
                "filter": filter,
                "ttl": ttl,
                "created_at": now_ts(),
                "expires_at": expires_at,
            }),
        );
    }

    /// Slave: a matched watch event arrived on `{root}/watch/evt/{sid}`. Queue
    /// it (mux_pull returns it under `watch`) and wake the agent so it stops
    /// polling.
    async fn on_watch_evt(self: &Arc<Node>, data: &Value) {
        if self.role != "slave" {
            return;
        }
        {
            let mut s = self.state.lock().await;
            s.watch_events.push_back(data.clone());
        }
        self.watch_notify.notify_one();
        self.wake();
    }

    /// Master: match a produced event against stored watches and publish each
    /// match to `{root}/watch/evt/{watcher_sid}`. Expired watches are dropped.
    async fn emit_watch_events(&self, kind: &str, event: Value) {
        let mut notify: Vec<(String, Value)> = Vec::new();
        {
            let mut s = self.state.lock().await;
            let now = now_ts();
            s.watches.retain(|_, w| {
                let exp = w.get("expires_at").and_then(|v| v.as_f64()).unwrap_or(0.0);
                exp == 0.0 || exp > now
            });
            for w in s.watches.values() {
                if w.get("kind").and_then(|v| v.as_str()) != Some(kind) {
                    continue;
                }
                let filter = w.get("filter").cloned().unwrap_or_else(|| json!({}));
                if !Self::watch_matches(&filter, &event) {
                    continue;
                }
                let sid = w.get("watcher_sid").and_then(|v| v.as_str()).unwrap_or("").to_string();
                let wid = w.get("watch_id").cloned().unwrap_or_else(|| json!(""));
                if !sid.is_empty() {
                    notify.push((sid, wid));
                }
            }
        }
        for (sid, wid) in notify {
            self.publish(
                &self.topic(&["watch", "evt", &sid]),
                Some(json!({
                    "watch_id": wid,
                    "kind": kind,
                    "event": event.clone(),
                    "ts": now_ts(),
                })),
                false,
            )
            .await;
        }
    }

    /// Partial-match a watch `filter` against an event payload. `{"path": "/x"}`
    /// requires the exact path; `{"path_prefix": "/x"}` requires the prefix;
    /// `{}` / absent matches every event of the watch's kind.
    fn watch_matches(filter: &Value, event: &Value) -> bool {
        if let Some(path) = filter.get("path").and_then(|v| v.as_str())
            && event.get("path").and_then(|v| v.as_str()) != Some(path)
        {
            return false;
        }
        if let Some(prefix) = filter.get("path_prefix").and_then(|v| v.as_str()) {
            let p = event.get("path").and_then(|v| v.as_str()).unwrap_or("");
            if !p.starts_with(prefix) {
                return false;
            }
        }
        true
    }

    pub async fn list_zones(&self) -> Value {
        json!({"zones": self.zones_obj().await, "updated": now_ts()})
    }

    pub async fn get_zone_snapshot(&self) -> Value {
        let s = self.state.lock().await;
        s.zone_snapshot.clone()
    }

    // ---- topology ----

    pub async fn topology(&self) -> Value {
        let s = self.state.lock().await;
        let tree = build_tree(&s.registry, &self.sid, &self.sid);
        json!({"role": self.role, "session_id": self.sid, "tree": tree})
    }

    // ---- conflicts ----

    pub async fn report_conflict(
        &self,
        files: Option<Vec<String>>,
        description: &str,
        severity: &str,
        suggestion: &str,
        zone: Option<String>,
    ) -> Value {
        let severity = if matches!(severity, "low" | "medium" | "high" | "critical") {
            severity.to_string()
        } else {
            "medium".to_string()
        };
        let entry = json!({
            "id": uuid::Uuid::new_v4().simple().to_string(),
            "sid": self.sid,
            "files": files.unwrap_or_default(),
            "zone": zone,
            "description": description,
            "severity": severity,
            "suggestion": suggestion,
            "ts": now_ts(),
        });
        self.publish(&self.topic(&["conflict", self.sid.as_str()]), Some(entry.clone()), true).await;
        let id = entry.get("id").cloned().unwrap_or_default();
        {
            let mut s = self.state.lock().await;
            s.conflicts.insert(id.as_str().unwrap_or("").to_string(), entry.clone());
        }
        self.persist_conflicts().await;
        let mut resp = entry.clone();
        if let Some(m) = resp.as_object_mut() {
            m.insert("ok".to_string(), json!(true));
        }
        resp
    }

    async fn on_conflict(self: &Arc<Node>, sid: &str, data: &Value) {
        if self.role != "master" {
            return;
        }
        let has_content = data.get("id").and_then(|v| v.as_str()).map(|s| !s.is_empty()).unwrap_or(false)
            || data.get("description").and_then(|v| v.as_str()).map(|s| !s.is_empty()).unwrap_or(false);
        if !has_content {
            return;
        }
        let cid = data.get("id").and_then(|v| v.as_str()).unwrap_or("").to_string();
        let cid = if cid.is_empty() {
            uuid::Uuid::new_v4().simple().to_string()
        } else {
            cid
        };
        let mut entry = data.clone();
        entry["id"] = json!(cid);
        if entry.get("ts").is_none() {
            entry["ts"] = json!(now_ts());
        }
        let mut wake_up = !self.conf.digest_mode;
        {
            let mut s = self.state.lock().await;
            s.conflicts.insert(cid.clone(), entry.clone());
            let ev = json!({"kind": "conflict_reported", "session_id": sid, "conflict": entry.clone(), "id": cid});
            if self.wake_for(&s, &ev) {
                wake_up = true;
            }
            Self::push_event(&mut s, ev);
        }
        self.persist_conflicts().await;
        self.persist_state().await;
        self.publish_conflicts().await;
        self.events_notify.notify_one();
        if wake_up {
            self.wake();
        }
    }

    pub async fn list_conflicts(&self, limit: i64) -> Value {
        let s = self.state.lock().await;
        let mut entries: Vec<&Value> = s.conflicts.values().collect();
        entries.sort_by(|a, b| {
            let ta = a.get("ts").and_then(|v| v.as_f64()).unwrap_or(0.0);
            let tb = b.get("ts").and_then(|v| v.as_f64()).unwrap_or(0.0);
            tb.partial_cmp(&ta).unwrap_or(std::cmp::Ordering::Equal)
        });
        let total = entries.len();
        let limit = limit.max(1) as usize;
        let out: Vec<Value> = entries.into_iter().take(limit).cloned().collect();
        json!({"conflicts": out, "total": total})
    }

    pub async fn risk_zones(&self) -> Value {
        let s = self.state.lock().await;
        json!({"risk_zones": risk_zones_from(&s)})
    }

    async fn persist_conflicts(&self) {
        let entries = {
            let s = self.state.lock().await;
            let mut v: Vec<&Value> = s.conflicts.values().collect();
            v.sort_by(|a, b| {
                let ta = a.get("ts").and_then(|x| x.as_f64()).unwrap_or(0.0);
                let tb = b.get("ts").and_then(|x| x.as_f64()).unwrap_or(0.0);
                ta.partial_cmp(&tb).unwrap_or(std::cmp::Ordering::Equal)
            });
            v.into_iter().cloned().collect::<Vec<_>>()
        };
        let path = crate::config::expand(&self.config_dir).join("conflicts.json");
        if let Ok(text) = serde_json::to_string_pretty(&entries) {
            write_persisted(&path, &(text + "\n"));
        }
    }

    /// Persist the full coordination state (`<config_dir>/state.json`): task
    /// table, zone registry, conflicts, approval escalations/traces, the
    /// digest high-water mark and the event sequence number. Restart-safe:
    /// scheduling decisions and the digest `since` increment survive a node
    /// restart (see `load_state_from_disk`). No await happens while holding
    /// the state lock.
    async fn persist_state(&self) {
        let snapshot = {
            let s = self.state.lock().await;
            let tasks: Vec<Value> = s
                .task_order
                .iter()
                .filter_map(|id| s.tasks.get(id).cloned())
                .collect();
            let mut zones = Map::new();
            for (k, v) in &s.zones {
                zones.insert(k.clone(), v.clone());
            }
            let mut conflicts: Vec<&Value> = s.conflicts.values().collect();
            conflicts.sort_by(|a, b| {
                let ta = a.get("ts").and_then(|x| x.as_f64()).unwrap_or(0.0);
                let tb = b.get("ts").and_then(|x| x.as_f64()).unwrap_or(0.0);
                ta.partial_cmp(&tb).unwrap_or(std::cmp::Ordering::Equal)
            });
            let escalations: Vec<Value> = s.escalations.iter().cloned().collect();
            let mut approvals: Vec<&Value> = s.approvals.values().collect();
            approvals.sort_by(|a, b| {
                let ta = a.get("ts").and_then(|x| x.as_f64()).unwrap_or(0.0);
                let tb = b.get("ts").and_then(|x| x.as_f64()).unwrap_or(0.0);
                ta.partial_cmp(&tb).unwrap_or(std::cmp::Ordering::Equal)
            });
            json!({
                "tasks": tasks,
                "zones": Value::Object(zones),
                "conflicts": conflicts.to_vec(),
                "escalations": escalations,
                "approvals": approvals.to_vec(),
                "event_seq": s.event_seq,
                "last_digest_ts": s.last_digest_ts,
            })
        };
        let path = crate::config::expand(&self.config_dir).join("state.json");
        if let Ok(text) = serde_json::to_string_pretty(&snapshot) {
            write_persisted(&path, &(text + "\n"));
        }
    }

    async fn publish_conflicts(&self) {
        let entries = {
            let s = self.state.lock().await;
            let mut v: Vec<&Value> = s.conflicts.values().collect();
            v.sort_by(|a, b| {
                let ta = a.get("ts").and_then(|x| x.as_f64()).unwrap_or(0.0);
                let tb = b.get("ts").and_then(|x| x.as_f64()).unwrap_or(0.0);
                tb.partial_cmp(&ta).unwrap_or(std::cmp::Ordering::Equal)
            });
            v.into_iter().cloned().collect::<Vec<_>>()
        };
        self.publish(
            &self.topic(&["conflicts"]),
            Some(json!({"conflicts": entries, "updated": now_ts()})),
            true,
        )
        .await;
    }

    // ---- lifecycle ----

    /// Re-publish the retained registry/hb (used after a same-identity
    /// node raced ahead of us, so the live node owns the retained state).
    pub async fn reannounce(self: &Arc<Node>) {
        self.announce().await;
    }

    /// Graceful shutdown: publish the hb offline flag and clear the retained
    /// registry, then disconnect and abort tasks. `clear_retained` is false
    /// when a live node with the same session id is replacing us (publishing
    /// offline would make the master see a spurious offline blip for a
    /// still-online node).
    pub async fn stop_with(self: &Arc<Node>, clear_retained: bool) {
        {
            let mut s = self.state.lock().await;
            if s.shutting_down {
                return;
            }
            s.shutting_down = true;
        }
        if let Some(client) = &self.client {
            if clear_retained {
                // Graceful leave: publish the offline flag on the retained hb
                // topic so the master sees `slave_left` immediately instead of
                // waiting for hb_timeout. Abrupt loss is covered by the LWT.
                let t = self.topic(&["hb", self.sid.as_str()]);
                let payload = json!({"sid": self.sid, "status": "offline",
                                     "reason": "shutdown", "ts": now_ts()});
                let _ = client.publish(t, qos(self.conf.qos), true,
                                       serde_json::to_vec(&payload).unwrap_or_default()).await;
                let t = self.topic(&["registry", self.sid.as_str()]);
                let _ = client.publish(t, qos(self.conf.qos), true, Vec::new()).await;
            }
            let _ = client.disconnect().await;
        }
        self.finish_stop().await;
    }

    /// Graceful shutdown with retained-state cleanup (default behavior).
    pub async fn stop(self: &Arc<Node>) {
        self.stop_with(true).await;
    }

    /// Finish stop: wait for background tasks to drain.
    async fn finish_stop(self: &Arc<Node>) {
        tokio::time::sleep(Duration::from_millis(300)).await;
        let handles: Vec<JoinHandle<()>> = self.tasks.lock().unwrap().drain(..).collect();
        for h in handles {
            h.abort();
        }
    }

    fn wake(self: &Arc<Node>) {
        if let Some(w) = &self.wake {
            w.wake();
        }
    }
}

// ---- helpers ----

fn load_conflicts_from_disk(config_dir: &str) -> Result<HashMap<String, Value>> {
    let path = crate::config::expand(config_dir).join("conflicts.json");
    if !path.exists() {
        return Ok(HashMap::new());
    }
    let text = std::fs::read_to_string(&path)?;
    let v: Value = serde_json::from_str(&text)?;
    let mut out = HashMap::new();
    if let Some(arr) = v.as_array() {
        for e in arr {
            if let Some(id) = e.get("id").and_then(|x| x.as_str()) {
                out.insert(id.to_string(), e.clone());
            }
        }
    }
    Ok(out)
}

/// Write a persisted JSON file, creating the parent directory when it does
/// not yet exist (a fresh config dir must not silently drop state on the
/// first write).
fn write_persisted(path: &std::path::Path, text: &str) {
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let _ = std::fs::write(path, text);
}

// ---------------------------------------------------------------------------
// state.json persistence
// ---------------------------------------------------------------------------

/// Persisted coordination state restored on startup: task table + order,
/// zone registry, conflicts, approval escalations/traces, the digest
/// high-water mark and the event sequence number.
type PersistedState = (
    HashMap<String, Value>,
    Vec<String>,
    HashMap<String, Value>,
    HashMap<String, Value>,
    VecDeque<Value>,
    HashMap<String, Value>,
    f64,
    u64,
);

/// Restore persisted coordination state (`<config_dir>/state.json`): the task
/// table, zone registry, conflicts, approval escalations/traces and the digest
/// high-water mark. Conflicts merge `state.json` over the legacy
/// `conflicts.json` so neither is lost across versions.
fn load_state_from_disk(config_dir: &str) -> PersistedState {
    let empty = (
        HashMap::new(),
        Vec::new(),
        HashMap::new(),
        HashMap::new(),
        VecDeque::new(),
        HashMap::new(),
        0.0,
        0u64,
    );
    let path = crate::config::expand(config_dir).join("state.json");
    if !path.exists() {
        return empty;
    }
    let Ok(text) = std::fs::read_to_string(&path) else {
        return empty;
    };
    let Ok(v) = serde_json::from_str::<Value>(&text) else {
        return empty;
    };
    let mut tasks: HashMap<String, Value> = HashMap::new();
    let mut task_order: Vec<String> = Vec::new();
    if let Some(arr) = v.get("tasks").and_then(|x| x.as_array()) {
        for t in arr {
            if let Some(id) = t.get("id").and_then(|x| x.as_str()) {
                task_order.push(id.to_string());
                tasks.insert(id.to_string(), t.clone());
            }
        }
    }
    let mut zones: HashMap<String, Value> = HashMap::new();
    if let Some(obj) = v.get("zones").and_then(|x| x.as_object()) {
        for (k, val) in obj {
            zones.insert(k.clone(), val.clone());
        }
    }
    let mut conflicts: HashMap<String, Value> = load_conflicts_from_disk(config_dir).unwrap_or_default();
    if let Some(arr) = v.get("conflicts").and_then(|x| x.as_array()) {
        for e in arr {
            if let Some(id) = e.get("id").and_then(|x| x.as_str()) {
                conflicts.insert(id.to_string(), e.clone());
            }
        }
    }
    let mut escalations: VecDeque<Value> = VecDeque::new();
    if let Some(arr) = v.get("escalations").and_then(|x| x.as_array()) {
        for e in arr {
            escalations.push_back(e.clone());
        }
    }
    let mut approvals: HashMap<String, Value> = HashMap::new();
    if let Some(arr) = v.get("approvals").and_then(|x| x.as_array()) {
        for e in arr {
            if let Some(id) = e.get("req_id").and_then(|x| x.as_str()) {
                approvals.insert(id.to_string(), e.clone());
            }
        }
    }
    let last_digest_ts = v.get("last_digest_ts").and_then(|x| x.as_f64()).unwrap_or(0.0);
    let event_seq = v.get("event_seq").and_then(|x| x.as_u64()).unwrap_or(0);
    (
        tasks,
        task_order,
        zones,
        conflicts,
        escalations,
        approvals,
        last_digest_ts,
        event_seq,
    )
}

// ---------------------------------------------------------------------------
// A1: internal event classification (never exposed in return values)
// ---------------------------------------------------------------------------

/// Whether an event needs the agent's attention. `Action` events wake the
/// agent (one merged hint per batch) and appear in `mux_digest`'s `actions`;
/// `Noise` events are counted (ack/tick) and dropped. This is a private
/// server-internal mechanism — it never appears in any tool return value.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum EventClass {
    Action,
    Noise,
}

/// Classify a queued event. `s` is needed for context (e.g. whether a
/// departing slave still owns unfinished tasks).
fn classify_event(s: &State, ev: &Value) -> EventClass {
    let kind = ev.get("kind").and_then(|v| v.as_str()).unwrap_or("");
    match kind {
        // Decision-worthy: agent must see these.
        "blocked" | "error" | "conflict_reported" | "conflict" | "rpc_request"
        | "approval_escalation" => EventClass::Action,
        // `done` carries acceptance data -> informational action.
        "done" => EventClass::Action,
        // Task transitions: Done/Failed are actionable; progress is noise.
        "task" => match ev.get("state").and_then(|v| v.as_str()) {
            Some("Done") | Some("Failed") => EventClass::Action,
            _ => EventClass::Noise,
        },
        // A slave leaving is only actionable when it still has unfinished work.
        "slave_left" => {
            let sid = ev.get("session_id").and_then(|v| v.as_str()).unwrap_or("");
            if slave_has_unfinished_tasks(s, sid) {
                EventClass::Action
            } else {
                EventClass::Noise
            }
        }
        // Status echoes classify via the reported state.
        "status" => {
            let st = ev
                .get("info")
                .and_then(|i| i.get("state"))
                .and_then(|v| v.as_str())
                .unwrap_or("");
            match st {
                "blocked" | "done" | "error" | "failed" | "conflict" => EventClass::Action,
                _ => EventClass::Noise,
            }
        }
        // ctrl_ack echoes, slave_joined and auto-approval traces are noise.
        _ => EventClass::Noise,
    }
}

/// Wake decision: in digest mode only `Action` events wake the agent (empty
/// wakes are dropped); legacy mode wakes on everything (opt-out / grayscale
/// rollback). Pure so the classifier policy is unit-testable.
fn wake_needed(digest_mode: bool, class: EventClass) -> bool {
    !digest_mode || class == EventClass::Action
}

/// Drain queued events into digest form: `actions` (decision-worthy, sorted
/// decision-first via `action_rank`) plus `noise_counts` (ack/tick). Events
/// older than `since` (when > 0) are dropped without counting — they were
/// already consumed by a previous digest. `mux_digest` and the legacy
/// `mux_pull` share this so both return the same `{actions, noise_counts}`
/// shape.
fn drain_events(s: &mut State, since: f64) -> (Vec<Value>, u64, u64) {
    let mut actions: Vec<Value> = Vec::new();
    let mut ack = 0u64;
    let mut tick = 0u64;
    while let Some(ev) = s.events.pop_front() {
        let ts = ev.get("ts").and_then(|v| v.as_f64()).unwrap_or(0.0);
        if since > 0.0 && ts <= since {
            continue;
        }
        match classify_event(s, &ev) {
            EventClass::Action => actions.push(ev),
            EventClass::Noise => match noise_kind(&ev) {
                Some("ack") => ack += 1,
                Some("tick") => tick += 1,
                _ => {}
            },
        }
    }
    actions.sort_by_key(action_rank);
    (actions, ack, tick)
}

/// Noise attribution for `mux_digest`'s `noise_counts`: `ack` for ctrl
/// acknowledgements, `tick` for progress/echo status and task progress.
fn noise_kind(ev: &Value) -> Option<&'static str> {
    match ev.get("kind").and_then(|v| v.as_str()) {
        Some("ctrl_ack") => Some("ack"),
        Some("status") | Some("task") => Some("tick"),
        _ => None,
    }
}

/// Digest action sort rank: decision-first (blocked / conflict / rpc_request /
/// approval escalation), then informational (done / task completion).
fn action_rank(ev: &Value) -> u8 {
    match ev.get("kind").and_then(|v| v.as_str()) {
        Some("blocked") => 0,
        Some("conflict_reported") | Some("conflict") => 1,
        Some("rpc_request") => 2,
        Some("approval_escalation") => 3,
        Some("task") => {
            if ev.get("state").and_then(|v| v.as_str()) == Some("Failed") {
                4
            } else {
                5
            }
        }
        Some("done") | Some("status") => 5,
        _ => 6,
    }
}

/// Does `sid` own any task that is not finished (not Done/Failed)?
fn slave_has_unfinished_tasks(s: &State, sid: &str) -> bool {
    s.tasks.values().any(|t| {
        t.get("owner").and_then(|v| v.as_str()) == Some(sid)
            && !matches!(
                t.get("state").and_then(|v| v.as_str()),
                Some("Done") | Some("Failed")
            )
    })
}

// ---------------------------------------------------------------------------
// A2: task table + dependency scheduling
// ---------------------------------------------------------------------------

const TASK_KINDS: [&str; 5] = ["Src", "Validate", "Docs", "Deps", "Release"];
const TASK_STATES: [&str; 6] = ["Scheduled", "Ready", "Assigned", "Working", "Done", "Failed"];

fn task_kind_valid(kind: &str) -> bool {
    TASK_KINDS.contains(&kind)
}

/// True when two crate lists overlap (opaque strings; no manifest parsing).
fn crates_intersect(a: &Value, b: &Value) -> bool {
    let Some(av) = a.as_array() else { return false };
    let Some(bv) = b.as_array() else { return false };
    av.iter()
        .filter_map(|x| x.as_str())
        .any(|x| bv.iter().filter_map(|y| y.as_str()).any(|y| y == x))
}

/// Dependency edges for a Validate task: every Src/Deps task whose
/// `target_crates` intersect this task's crates. Computed at assign time and
/// stored in `depends_on`.
fn compute_depends_on(s: &State, kind: &str, target_crates: &Value) -> Vec<String> {
    if kind != "Validate" {
        return Vec::new();
    }
    s.task_order
        .iter()
        .filter_map(|id| s.tasks.get(id))
        .filter(|t| {
            let k = t.get("kind").and_then(|v| v.as_str()).unwrap_or("");
            (k == "Src" || k == "Deps") && crates_intersect(t.get("target_crates").unwrap_or(&Value::Null), target_crates)
        })
        .filter_map(|t| t.get("id").and_then(|v| v.as_str()).map(|x| x.to_string()))
        .collect()
}

/// All `depends_on` tasks are Done.
fn deps_clear(s: &State, task: &Value) -> bool {
    let Some(deps) = task.get("depends_on").and_then(|v| v.as_array()) else {
        return true;
    };
    deps.iter().all(|d| {
        let id = d.as_str().unwrap_or("");
        s.tasks
            .get(id)
            .map(|t| t.get("state").and_then(|v| v.as_str()) == Some("Done"))
            .unwrap_or(true)
    })
}

/// Does the task touch global shared state (Cargo.lock, .git, generated dirs,
/// root manifest)?
fn task_touches_global(task: &Value) -> bool {
    let Some(files) = task.get("files").and_then(|v| v.as_array()) else {
        return false;
    };
    files
        .iter()
        .filter_map(|f| f.as_str())
        .any(is_global_shared_path)
}

/// Global shared state is serial: at most one such task may be Working/Assigned
/// mesh-wide; the rest queue in Scheduled.
fn global_serial_free(s: &State, task: &Value) -> bool {
    if !task_touches_global(task) {
        return true;
    }
    let tid = task.get("id").and_then(|v| v.as_str()).unwrap_or("");
    s.tasks.values().all(|t| {
        t.get("id").and_then(|v| v.as_str()).unwrap_or("") == tid
            || !task_touches_global(t)
            || !matches!(
                t.get("state").and_then(|v| v.as_str()),
                Some("Working") | Some("Assigned")
            )
    })
}

/// Global shared state paths: Cargo.lock anywhere, .git paths, generated
/// directories and root manifests.
fn is_global_shared_path(path: &str) -> bool {
    let p = path.trim_start_matches("./");
    let comps: Vec<&str> = p.split('/').collect();
    let name = comps.last().copied().unwrap_or("");
    if name == "Cargo.lock" {
        return true;
    }
    if comps.iter().any(|c| *c == ".git" || c.starts_with(".git/")) {
        return true;
    }
    if comps.len() == 1
        && matches!(name, "Cargo.toml" | "package.json" | "go.mod" | "pyproject.toml" | "pom.xml")
    {
        return true;
    }
    comps
        .iter()
        .any(|c| matches!(*c, "target" | "build" | "dist" | "generated" | "node_modules"))
}

/// Map a slave-reported status state onto a task state. `blocked` /
/// `conflict` stay `Working`: the owner is still on the task, just waiting —
/// only a hard error/failure moves the task to `Failed`.
fn task_state_from_status(status: &str) -> &'static str {
    match status {
        "done" => "Done",
        "error" | "failed" => "Failed",
        _ => "Working",
    }
}

/// A2: promote Scheduled tasks whose dependencies cleared / global slot freed
/// (Scheduled -> Ready) and collect the auto-release control messages for
/// Validate tasks that just became Ready. Idempotent: a task promotes at most
/// once, so a Validate task is released exactly once. Reads and mutates under
/// the caller's state lock.
fn recompute_tasks(s: &mut State) -> Vec<(String, Value)> {
    let mut releases: Vec<(String, Value)> = Vec::new();
    let ids: Vec<String> = s.task_order.clone();
    for id in ids {
        let (owner, kind, promote) = {
            let Some(t) = s.tasks.get(&id) else { continue };
            let promote = t.get("state").and_then(|v| v.as_str()) == Some("Scheduled")
                && deps_clear(s, t)
                && global_serial_free(s, t);
            (
                t.get("owner").and_then(|v| v.as_str()).unwrap_or("").to_string(),
                t.get("kind").and_then(|v| v.as_str()).unwrap_or("").to_string(),
                promote,
            )
        };
        if !promote {
            continue;
        }
        if let Some(t) = s.tasks.get_mut(&id) {
            t["state"] = json!("Ready");
            t["updated_at"] = json!(now_ts());
        }
        let tev = json!({
            "kind": "task",
            "task_id": id,
            "state": "Ready",
            "owner": owner,
            "ts": now_ts(),
        });
        Node::push_event(s, tev);
        if kind == "Validate" {
            releases.push((
                owner,
                json!({"task_id": id, "reason": "deps_ready", "kind": "validate"}),
            ));
        }
    }
    releases
}

/// Stable per-file path components.
fn path_components(p: &str) -> Vec<String> {
    p.trim_start_matches("./")
        .split('/')
        .filter(|c| !c.is_empty())
        .map(|c| c.to_string())
        .collect()
}

/// Parent directory of a file path (`a/b/c.rs` -> `a/b`; `x.rs` -> "").
fn parent_dir(p: &str) -> String {
    let comps = path_components(p);
    if comps.len() <= 1 {
        String::new()
    } else {
        comps[..comps.len() - 1].join("/")
    }
}

/// First `n` path components joined (`a/b/c.rs`, 2 -> `a/b`).
fn top_components(p: &str, n: usize) -> String {
    let comps = path_components(p);
    let m = n.min(comps.len());
    if m == 0 {
        String::new()
    } else {
        comps[..m].join("/")
    }
}

/// True when `p` follows a workspace-crate source layout:
/// `<container>/<crate>/src/...` (e.g. `crates/a/src/lib.rs`). Used to
/// distinguish "different crate in the same workspace" (level 3) from "same
/// root crate, different module" (level 2 / auto-approvable).
fn workspace_crate_src(p: &str) -> bool {
    let comps = path_components(p);
    comps.len() >= 3 && comps[2] == "src"
}


// ---------------------------------------------------------------------------
// A3: five-level `may_i_touch` impact check
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum MayIResult {
    AutoApproved,
    Denied,
    Escalated { level: u8 },
}

/// Five-level impact check (order 1 -> 5). Auto-approval is limited to
/// never-touched files with no zone claim and no conflict history, or a
/// same-owner repeat. Everything risky escalates to the master agent via
/// `approval_decide`.
fn check_may_i_touch(s: &State, files: &[String], owner: &str) -> MayIResult {
    if files.is_empty() {
        return MayIResult::AutoApproved;
    }
    // Level 1: an exact file is claimed by another active owner (zone or task).
    for f in files {
        if let Some(z) = s.zones.get(f) {
            let zowner = z.get("owner").and_then(|v| v.as_str()).unwrap_or("");
            if !zowner.is_empty() && zowner != owner {
                return MayIResult::Denied;
            }
        }
        for t in s.tasks.values() {
            let towner = t.get("owner").and_then(|v| v.as_str()).unwrap_or("");
            let active = matches!(
                t.get("state").and_then(|v| v.as_str()),
                Some("Assigned") | Some("Working")
            );
            if active && towner != owner && task_has_file(t, f) {
                return MayIResult::Denied;
            }
        }
    }
    // Level 4: global shared state is never auto-approved (force serial).
    if files.iter().any(|f| is_global_shared_path(f)) {
        return MayIResult::Escalated { level: 4 };
    }
    // Same-owner repeat: everything requested is already claimed by this owner.
    if files.iter().all(|f| owned_by(s, f, owner)) {
        return MayIResult::AutoApproved;
    }
    // Level 2: same module/crate (same parent dir or same top-2 prefix).
    for f in files {
        if other_claimed_paths(s, owner).iter().any(|g| {
            g != f && (parent_dir(g) == parent_dir(f) || top_components(g, 2) == top_components(f, 2))
        }) {
            return MayIResult::Escalated { level: 2 };
        }
    }
    // Level 3: workspace dependency neighbor. Two files are neighbors only
    // when they live in *different crates* of the same workspace container
    // (`<container>/<crate>/src/...`) — e.g. `crates/a/src/..` vs
    // `crates/b/src/..`. A flat `src/` tree is a single crate, so same-top-dir
    // there is level-2 territory, not a workspace neighbor; brand-new files
    // elsewhere in the root crate stay auto-approvable.
    for f in files {
        if other_claimed_paths(s, owner).iter().any(|g| {
            g != f
                && top_components(g, 1) == top_components(f, 1)
                && top_components(g, 2) != top_components(f, 2)
                && workspace_crate_src(g)
                && workspace_crate_src(f)
        }) {
            return MayIResult::Escalated { level: 3 };
        }
    }
    // Level 5: conflict history (risk zones) covers the requested path.
    for f in files {
        if conflict_paths(s).iter().any(|c| f == c || f.starts_with(&format!("{c}/")) || c.starts_with(&format!("{f}/"))) {
            return MayIResult::Escalated { level: 5 };
        }
    }
    MayIResult::AutoApproved
}

fn task_has_file(t: &Value, f: &str) -> bool {
    t.get("files")
        .and_then(|v| v.as_array())
        .map(|arr| arr.iter().filter_map(|x| x.as_str()).any(|x| x == f))
        .unwrap_or(false)
}

/// Is every requested file already claimed by `owner` (their active task files
/// or zones)?
fn owned_by(s: &State, f: &str, owner: &str) -> bool {
    if s
        .zones
        .get(f)
        .map(|z| z.get("owner").and_then(|v| v.as_str()) == Some(owner))
        .unwrap_or(false)
    {
        return true;
    }
    s.tasks.values().any(|t| {
        t.get("owner").and_then(|v| v.as_str()) == Some(owner)
            && task_has_file(t, f)
    })
}

/// All paths claimed by owners other than `owner` (active task files + zones).
fn other_claimed_paths(s: &State, owner: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for (path, z) in &s.zones {
        let zowner = z.get("owner").and_then(|v| v.as_str()).unwrap_or("");
        if !zowner.is_empty() && zowner != owner {
            out.push(path.clone());
        }
    }
    for t in s.tasks.values() {
        let towner = t.get("owner").and_then(|v| v.as_str()).unwrap_or("");
        let active = matches!(
            t.get("state").and_then(|v| v.as_str()),
            Some("Assigned") | Some("Working")
        );
        if active
            && towner != owner
            && let Some(files) = t.get("files").and_then(|v| v.as_array())
        {
            for f in files {
                if let Some(fs) = f.as_str() {
                    out.push(fs.to_string());
                }
            }
        }
    }
    out
}

/// Paths with conflict history (from `s.conflicts`, files + zones).
fn conflict_paths(s: &State) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for e in s.conflicts.values() {
        if let Some(files) = e.get("files").and_then(|v| v.as_array()) {
            for f in files {
                if let Some(p) = f.as_str()
                    && !p.is_empty()
                {
                    out.push(p.to_string());
                }
            }
        }
        if let Some(z) = e.get("zone").and_then(|v| v.as_str())
            && !z.is_empty()
        {
            out.push(z.to_string());
        }
    }
    out
}

/// Aggregate conflict history into ranked risk zones (shared by `risk_zones()`
/// and the level-5 check). Reads state without locking; callers hold the lock.
type RiskAggEntry = (i64, std::collections::BTreeSet<String>, f64);

fn risk_zones_from(s: &State) -> Vec<Value> {
    let mut agg: HashMap<String, RiskAggEntry> = HashMap::new();
    for e in s.conflicts.values() {
        let mut paths: Vec<String> = Vec::new();
        if let Some(files) = e.get("files").and_then(|v| v.as_array()) {
            for f in files {
                if let Some(p) = f.as_str()
                    && !p.is_empty()
                {
                    paths.push(p.to_string());
                }
            }
        }
        if let Some(z) = e.get("zone").and_then(|v| v.as_str())
            && !z.is_empty()
        {
            paths.push(z.to_string());
        }
        let sev = e.get("severity").and_then(|v| v.as_str()).unwrap_or("medium").to_string();
        let ts = e.get("ts").and_then(|v| v.as_f64()).unwrap_or(0.0);
        for pa in paths {
            let a = agg.entry(pa).or_insert((0, std::collections::BTreeSet::new(), 0.0));
            a.0 += 1;
            a.1.insert(sev.clone());
            a.2 = a.2.max(ts);
        }
    }
    let mut ranked: Vec<(&String, &RiskAggEntry)> = agg.iter().collect();
    ranked.sort_by(|a, b| {
        let ca = b.1 .0.cmp(&a.1 .0);
        if ca != std::cmp::Ordering::Equal {
            return ca;
        }
        b.1 .2.partial_cmp(&a.1 .2).unwrap_or(std::cmp::Ordering::Equal)
    });
    ranked
        .iter()
        .map(|(pa, (count, sevs, last))| {
            json!({"path": pa, "count": count, "severities": sevs.iter().cloned().collect::<Vec<_>>(), "last": last})
        })
        .collect()
}

fn build_tree(reg: &HashMap<String, Value>, root_sid: &str, sid: &str) -> Value {
    let children: Vec<String> = if sid == root_sid {
        let mut v: Vec<String> = reg
            .iter()
            .filter(|(k, e)| {
                if *k == sid {
                    return false;
                }
                if e.get("role").and_then(|r| r.as_str()) == Some("master") {
                    return false;
                }
                match e.get("parent_id").and_then(|p| p.as_str()) {
                    Some(p) => p == sid || p.is_empty(),
                    None => true,
                }
            })
            .map(|(k, _)| k.clone())
            .collect();
        v.sort();
        v
    } else {
        let mut v: Vec<String> = reg
            .iter()
            .filter(|(_, e)| e.get("parent_id").and_then(|p| p.as_str()) == Some(sid))
            .map(|(k, _)| k.clone())
            .collect();
        v.sort();
        v
    };
    json!({
        "session_id": sid,
        "info": reg.get(sid).cloned().unwrap_or(json!({})),
        "children": children.iter().map(|c| build_tree(reg, root_sid, c)).collect::<Vec<_>>(),
    })
}

#[cfg(test)]
mod zone_lock_tests {
    use super::*;
    use crate::config::Config;

    fn test_config() -> Config {
        let mut c = Config::default();
        c.host = "127.0.0.1".to_string();
        c.port = 1883;
        c.rpc_timeout = 10.0;
        c
    }

    async fn spawn(role: &str, sid: &str, root: &str, master_sid: Option<String>) -> Arc<Node> {
        Node::start(
            role,
            sid,
            None,
            master_sid,
            root,
            "/tmp/agent-mux-test",
            &test_config(),
            None,
        )
        .await
        .expect("node start")
    }

    /// Zone ownership is master-only: slaves cannot lock zones themselves and
    /// must ask via the `zone_request` RPC, which the master node answers
    /// against its authoritative registry (grant / FIFO queue / release).
    #[tokio::test]
    #[ignore = "requires a running MQTT broker on 127.0.0.1:1883"]
    async fn zone_ownership_is_master_only() {
        // All three nodes share one topic root so they form a single mesh.
        // Topic roots must be a single path segment (on_message matches
        // parts[0] == root), so use a flat uuid root for the test mesh.
        let root = format!("ztest{}", uuid::Uuid::new_v4().simple());
        let master = spawn("master", "test-master", &root, None).await;
        let slave_a = spawn("slave", "test-slave-a", &root, Some(master.sid.clone())).await;
        let slave_b = spawn("slave", "test-slave-b", &root, Some(master.sid.clone())).await;

        // Slaves cannot lock zones directly.
        let denied = slave_a.zone_acquire("/z", None, false).await;
        assert_eq!(denied["ok"], false, "slave zone_acquire must be rejected: {denied}");
        let denied_release = slave_a.zone_release("/z", None).await;
        assert_eq!(denied_release["ok"], false, "slave zone_release must be rejected");

        // Slave A requests ownership -> master grants it.
        let rid = slave_a
            .zone_request("/z", false)
            .await
            .expect("zone_request")
            .get("request_id")
            .and_then(|v| v.as_str())
            .unwrap()
            .to_string();
        let res = slave_a.await_result(&rid, Some(10.0)).await;
        assert_eq!(res["status"], "done", "request must complete: {res}");
        let result = res["result"].clone();
        assert_eq!(result["ok"], true, "grant should succeed: {result}");
        assert_eq!(result["owner"], "test-slave-a");

        // Slave B requests while A holds -> FIFO-queued.
        let rid2 = slave_b
            .zone_request("/z", false)
            .await
            .expect("zone_request")
            .get("request_id")
            .and_then(|v| v.as_str())
            .unwrap()
            .to_string();
        let res2 = slave_b.await_result(&rid2, Some(10.0)).await;
        assert_eq!(res2["result"]["ok"], false, "held zone must queue: {res2}");
        assert_eq!(res2["result"]["queued"], true);

        // A releases -> handed to the next queued owner (B).
        let rid3 = slave_a
            .zone_request("/z", true)
            .await
            .expect("zone_request")
            .get("request_id")
            .and_then(|v| v.as_str())
            .unwrap()
            .to_string();
        let res3 = slave_a.await_result(&rid3, Some(10.0)).await;
        assert_eq!(res3["result"]["ok"], true, "release should succeed: {res3}");
        assert_eq!(res3["result"]["next_owner"], "test-slave-b");

        // The master's registry is the source of truth: B now owns /z.
        let zones = master.list_zones().await;
        assert_eq!(zones["zones"]["/z"]["owner"], "test-slave-b", "{zones}");

        // The master can still assign a zone to a slave manually.
        let assigned = master
            .zone_acquire("/m", Some("test-slave-a".to_string()), false)
            .await;
        assert_eq!(assigned["ok"], true, "master assign should succeed: {assigned}");
        assert_eq!(assigned["owner"], "test-slave-a");

        master.stop().await;
        slave_a.stop().await;
        slave_b.stop().await;
    }
}

#[cfg(test)]
mod coordination_tests {
    //! Unit / no-broker tests for the coordination redesign
    //! (`docs/coordination-design.md`): A1 event classification + digest, A2
    //! task scheduling + auto-release, A3 five-level `may_i_touch` approvals
    //! and A4 `zone_steal`. Everything here runs without an MQTT broker: the
    //! pure helpers get a bare `State`, and the async methods get a "bare
    //! node" whose `client` is `None` so `publish` no-ops.

    use super::*;
    use crate::config::Config;

    fn temp_dir(tag: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!(
            "agent-mux-coord-{tag}-{}",
            uuid::Uuid::new_v4().simple()
        ))
    }

    fn state() -> State {
        State::new(None, temp_dir("state").to_str().unwrap())
    }

    fn bare_node_at(role: &str, sid: &str, master: Option<&str>, config_dir: &str) -> Node {
        Node {
            role: role.to_string(),
            sid: sid.to_string(),
            parent_id: std::sync::Mutex::new(None),
            root: "testroot".to_string(),
            config_dir: config_dir.to_string(),
            conf: Config::default(),
            state: Arc::new(Mutex::new(State::new(
                master.map(|m| m.to_string()),
                config_dir,
            ))),
            ctrl_notify: Arc::new(Notify::new()),
            events_notify: Arc::new(Notify::new()),
            rpc_notify: Arc::new(Notify::new()),
            watch_notify: Arc::new(Notify::new()),
            ready_notify: Arc::new(Notify::new()),
            client: None,
            tasks: std::sync::Mutex::new(Vec::new()),
            wake: None,
        }
    }

    fn bare_node(role: &str, sid: &str, master: Option<&str>) -> Node {
        bare_node_at(role, sid, master, temp_dir("node").to_str().unwrap())
    }

    #[allow(clippy::too_many_arguments)] // test helper: task table fixture
    fn add_task(
        s: &mut State,
        id: &str,
        kind: &str,
        crates: &[&str],
        files: &[&str],
        owner: &str,
        st: &str,
        deps: &[&str],
    ) {
        let task = json!({
            "id": id,
            "kind": kind,
            "target_crates": crates,
            "files": files,
            "owner": owner,
            "state": st,
            "depends_on": deps,
            "created_at": 0.0,
            "updated_at": 0.0,
        });
        s.tasks.insert(id.to_string(), task);
        s.task_order.push(id.to_string());
    }

    fn dep_refs(deps: &[String]) -> Vec<&str> {
        deps.iter().map(|d| d.as_str()).collect()
    }

    // -----------------------------------------------------------------------
    // A1: event classifier + digest
    // -----------------------------------------------------------------------

    #[test]
    fn classifier_noise_vs_action() {
        let s = state();
        // ctrl_ack echoes are noise and count as `ack`.
        assert_eq!(classify_event(&s, &json!({"kind": "ctrl_ack"})), EventClass::Noise);
        assert_eq!(noise_kind(&json!({"kind": "ctrl_ack"})), Some("ack"));
        // progress / ready echo statuses are noise and count as `tick`.
        for st in ["working", "ready"] {
            assert_eq!(
                classify_event(&s, &json!({"kind": "status", "info": {"state": st}})),
                EventClass::Noise,
                "status {st}"
            );
        }
        assert_eq!(noise_kind(&json!({"kind": "status"})), Some("tick"));
        // task progress is noise; completion / failure are actions.
        assert_eq!(classify_event(&s, &json!({"kind": "task", "state": "Ready"})), EventClass::Noise);
        assert_eq!(noise_kind(&json!({"kind": "task"})), Some("tick"));
        assert_eq!(classify_event(&s, &json!({"kind": "task", "state": "Done"})), EventClass::Action);
        assert_eq!(classify_event(&s, &json!({"kind": "task", "state": "Failed"})), EventClass::Action);
        // slave_joined is noise.
        assert_eq!(classify_event(&s, &json!({"kind": "slave_joined"})), EventClass::Noise);

        // Decision-worthy events are actions.
        for kind in [
            "blocked",
            "conflict_reported",
            "conflict",
            "rpc_request",
            "approval_escalation",
            "done",
        ] {
            assert_eq!(classify_event(&s, &json!({"kind": kind})), EventClass::Action, "{kind}");
        }
        // Status echoes of decision-worthy states are actions too.
        for st in ["blocked", "done", "error", "failed", "conflict"] {
            assert_eq!(
                classify_event(&s, &json!({"kind": "status", "info": {"state": st}})),
                EventClass::Action,
                "status {st}"
            );
        }
    }

    #[test]
    fn slave_left_action_only_with_unfinished_tasks() {
        let mut s = state();
        add_task(&mut s, "t1", "Src", &["crate-x"], &["src/a.rs"], "worker", "Working", &[]);
        // A departing slave that still owns an unfinished task is actionable.
        assert_eq!(
            classify_event(&s, &json!({"kind": "slave_left", "session_id": "worker"})),
            EventClass::Action
        );
        // An idle slave leaving is noise.
        assert_eq!(
            classify_event(&s, &json!({"kind": "slave_left", "session_id": "idle"})),
            EventClass::Noise
        );
        // Once the task is done, the same slave leaving is noise again.
        s.tasks.get_mut("t1").unwrap()["state"] = json!("Done");
        assert_eq!(
            classify_event(&s, &json!({"kind": "slave_left", "session_id": "worker"})),
            EventClass::Noise
        );
    }

    #[test]
    fn wake_policy_drops_empty_noise_wakes_in_digest_mode() {
        // Digest mode: only Action wakes; an all-noise batch is dropped.
        assert!(!wake_needed(true, EventClass::Noise));
        assert!(wake_needed(true, EventClass::Action));
        // Legacy (opt-out / grayscale rollback): every event wakes as before.
        assert!(wake_needed(false, EventClass::Noise));
        assert!(wake_needed(false, EventClass::Action));
    }

    #[tokio::test]
    async fn digest_counts_noise_and_returns_decision_first_actions() {
        let node = bare_node("master", "m", None);
        {
            let mut s = node.state.lock().await;
            Node::push_event(&mut s, json!({"kind": "ctrl_ack", "session_id": "a", "ts": 1.0}));
            Node::push_event(
                &mut s,
                json!({"kind": "status", "session_id": "a", "info": {"state": "working"}, "ts": 2.0}),
            );
            Node::push_event(&mut s, json!({"kind": "done", "session_id": "a", "ts": 3.0}));
            Node::push_event(&mut s, json!({"kind": "blocked", "session_id": "a", "reason": "x", "ts": 4.0}));
        }
        let d = node.digest(None).await;
        assert_eq!(d["noise_counts"]["ack"], 1, "{d}");
        assert_eq!(d["noise_counts"]["tick"], 1, "{d}");
        let actions = d["actions"].as_array().unwrap();
        assert_eq!(actions.len(), 2, "{d}");
        assert_eq!(actions[0]["kind"], "blocked", "decision-worthy first: {d}");
        assert_eq!(actions[1]["kind"], "done", "informational last: {d}");
        let since = d["since"].as_f64().unwrap();
        assert!(since > 0.0);

        // Incremental: everything at/before `since` is consumed, no replay.
        let d2 = node.digest(Some(since)).await;
        assert_eq!(d2["actions"].as_array().unwrap().len(), 0, "{d2}");
        assert_eq!(d2["noise_counts"]["ack"], 0);
        assert_eq!(d2["noise_counts"]["tick"], 0);

        // Legacy mux_pull keeps the same `{actions, noise_counts}` shape.
        let p = node.pull_queued().await;
        assert!(p.get("actions").is_some() && p.get("noise_counts").is_some(), "{p}");
        assert!(p.get("control").is_some() && p.get("rpc_requests").is_some(), "{p}");
        assert!(p.get("watch").is_some(), "{p}");
    }

    // -----------------------------------------------------------------------
    // A2: task table + dependency scheduling
    // -----------------------------------------------------------------------

    #[test]
    fn validate_waits_for_all_src_tasks() {
        let mut s = state();
        add_task(&mut s, "s1", "Src", &["crate-x"], &["src/a.rs"], "A", "Scheduled", &[]);
        add_task(&mut s, "s2", "Src", &["crate-x"], &["src/b.rs"], "B", "Scheduled", &[]);
        let deps = compute_depends_on(&s, "Validate", &json!(["crate-x"]));
        assert_eq!(deps, vec!["s1".to_string(), "s2".to_string()]);
        let dep_refs = dep_refs(&deps);
        add_task(&mut s, "v1", "Validate", &["crate-x"], &["tests/x.rs"], "C", "Scheduled", &dep_refs);

        // First pass: the src tasks (no deps) become Ready, the Validate stays
        // Scheduled until both deps are Done.
        let releases = recompute_tasks(&mut s);
        assert!(releases.is_empty(), "{releases:?}");
        assert_eq!(s.tasks["s1"]["state"], "Ready");
        assert_eq!(s.tasks["s2"]["state"], "Ready");
        assert_eq!(s.tasks["v1"]["state"], "Scheduled", "validate must not start before deps are Done");

        // One dep done is not enough.
        s.tasks.get_mut("s1").unwrap()["state"] = json!("Done");
        let releases = recompute_tasks(&mut s);
        assert!(releases.is_empty(), "{releases:?}");
        assert_eq!(s.tasks["v1"]["state"], "Scheduled");

        // All deps done -> Validate becomes Ready and is auto-released once.
        s.tasks.get_mut("s2").unwrap()["state"] = json!("Done");
        let releases = recompute_tasks(&mut s);
        assert_eq!(releases.len(), 1, "auto-release emitted once: {releases:?}");
        assert_eq!(releases[0].0, "C");
        assert_eq!(releases[0].1["task_id"], "v1");
        assert_eq!(s.tasks["v1"]["state"], "Ready");

        // Idempotent: a second pass releases nothing.
        let releases = recompute_tasks(&mut s);
        assert!(releases.is_empty(), "{releases:?}");
    }

    #[test]
    fn cross_crate_validate_blocked_by_dependency_crate() {
        let mut s = state();
        add_task(&mut s, "b", "Src", &["crate-b"], &["crates/b/src/lib.rs"], "B", "Scheduled", &[]);
        add_task(&mut s, "a", "Src", &["crate-a"], &["crates/a/src/lib.rs"], "A", "Scheduled", &[]);
        // A's Validate depends on crate-a AND crate-b (crate A depends on B).
        let deps = compute_depends_on(&s, "Validate", &json!(["crate-a", "crate-b"]));
        assert!(deps.contains(&"a".to_string()), "{deps:?}");
        assert!(deps.contains(&"b".to_string()), "{deps:?}");
        let dep_refs = dep_refs(&deps);
        add_task(&mut s, "va", "Validate", &["crate-a", "crate-b"], &["tests/a.rs"], "C", "Scheduled", &dep_refs);

        // Even with A's own src Done, B's incomplete work blocks A's Validate.
        s.tasks.get_mut("a").unwrap()["state"] = json!("Done");
        let releases = recompute_tasks(&mut s);
        assert!(releases.is_empty(), "{releases:?}");
        assert_eq!(s.tasks["va"]["state"], "Scheduled", "validate must wait for B (cross-crate dep)");

        s.tasks.get_mut("b").unwrap()["state"] = json!("Done");
        let releases = recompute_tasks(&mut s);
        assert_eq!(releases.len(), 1, "{releases:?}");
        assert_eq!(s.tasks["va"]["state"], "Ready");
    }

    #[test]
    fn global_shared_state_is_serialized() {
        let mut s = state();
        add_task(&mut s, "g1", "Src", &["crate-x"], &["Cargo.lock"], "A", "Working", &[]);
        add_task(&mut s, "g2", "Src", &["crate-y"], &["Cargo.lock"], "B", "Scheduled", &[]);
        add_task(&mut s, "n1", "Src", &["crate-z"], &["src/plain.rs"], "C", "Scheduled", &[]);

        // Non-global task promotes immediately; the second global task queues.
        let releases = recompute_tasks(&mut s);
        assert!(releases.is_empty(), "{releases:?}");
        assert_eq!(s.tasks["n1"]["state"], "Ready");
        assert_eq!(s.tasks["g2"]["state"], "Scheduled", "global slot held by g1");

        // g1 finishes -> the queued global task can start.
        s.tasks.get_mut("g1").unwrap()["state"] = json!("Done");
        recompute_tasks(&mut s);
        assert_eq!(s.tasks["g2"]["state"], "Ready");
    }

    #[test]
    fn global_shared_path_detection() {
        assert!(is_global_shared_path("Cargo.lock"));
        assert!(is_global_shared_path("./Cargo.lock"));
        assert!(is_global_shared_path(".git/config"));
        assert!(is_global_shared_path("target/debug/foo"));
        assert!(is_global_shared_path("generated/api.rs"));
        assert!(is_global_shared_path("Cargo.toml"));
        assert!(!is_global_shared_path("crates/x/Cargo.toml"), "nested manifest is not global");
        assert!(!is_global_shared_path("src/main.rs"));
    }

    #[tokio::test]
    async fn assign_task_validates_payload_and_schedules() {
        let node = bare_node("master", "m", None);

        // Missing fields -> rejected.
        let err = node.assign_task("slave-a", json!({"kind": "Src"})).await.unwrap_err();
        assert!(err.contains("must include"), "{err}");

        // Invalid kind -> rejected.
        let err = node
            .assign_task("slave-a", json!({"kind": "Nope", "target_crates": ["x"], "files": ["a.rs"]}))
            .await
            .unwrap_err();
        assert!(err.contains("invalid task kind"), "{err}");

        // Slaves cannot assign.
        let slave = bare_node("slave", "s", Some("m"));
        assert!(slave
            .assign_task("x", json!({"kind": "Src", "target_crates": ["x"], "files": ["a.rs"]}))
            .await
            .is_err());

        // Valid assign -> task lands in the table. A Src task has no
        // dependencies, so the server auto-releases it to Ready immediately.
        let r = node
            .assign_task("slave-a", json!({"kind": "Src", "target_crates": ["crate-x"], "files": ["src/a.rs"]}))
            .await
            .unwrap();
        assert_eq!(r["ok"], true, "{r}");
        assert_eq!(r["state"], "Ready", "{r}");
        let tid = r["task_id"].as_str().unwrap().to_string();
        assert_eq!(node.task_list().await["total"], 1);
        let show = node.task_show(&tid).await;
        assert_eq!(show["task"]["kind"], "Src");
        assert_eq!(show["task"]["owner"], "slave-a");
    }

    #[tokio::test]
    async fn validate_released_when_src_reports_done() {
        let node = Arc::new(bare_node("master", "m", None));
        let src = node
            .assign_task("slave-a", json!({"kind": "Src", "target_crates": ["crate-x"], "files": ["src/a.rs"]}))
            .await
            .unwrap();
        let src_id = src["task_id"].as_str().unwrap().to_string();
        let val = node
            .assign_task("slave-c", json!({"kind": "Validate", "target_crates": ["crate-x"], "files": ["tests/x.rs"]}))
            .await
            .unwrap();
        let val_id = val["task_id"].as_str().unwrap().to_string();
        assert_eq!(val["state"], "Scheduled", "validate waits for the src task: {val}");

        // slave-a reports `done` for its src task (4-state protocol).
        node.on_status(
            "slave-a",
            &json!({
                "sid": "slave-a",
                "state": "done",
                "task_id": src_id,
                "task_kind": "Src",
                "target_crates": ["crate-x"],
                "files": ["src/a.rs"],
                "ts": now_ts(),
            }),
        )
        .await;

        // The server auto-released the Validate without any agent involvement.
        let show = node.task_show(&val_id).await;
        assert_eq!(show["task"]["state"], "Ready", "{show}");
        let s = node.state.lock().await;
        let ready_events = s
            .events
            .iter()
            .filter(|e| {
                e.get("kind").and_then(|v| v.as_str()) == Some("task")
                    && e.get("task_id").and_then(|v| v.as_str()) == Some(val_id.as_str())
                    && e.get("state").and_then(|v| v.as_str()) == Some("Ready")
            })
            .count();
        assert_eq!(ready_events, 1, "validate released exactly once");
    }

    #[tokio::test]
    async fn task_cancel_and_force_update_schedule() {
        let node = bare_node("master", "m", None);
        let r = node
            .assign_task("slave-a", json!({"kind": "Validate", "target_crates": ["crate-x"], "files": ["tests/x.rs"]}))
            .await
            .unwrap();
        let tid = r["task_id"].as_str().unwrap().to_string();

        // Cancel removes the task.
        let c = node.task_cancel(&tid).await;
        assert_eq!(c["ok"], true, "{c}");
        assert_eq!(node.task_list().await["total"], 0);

        // Invalid forced state is rejected.
        let r2 = node
            .assign_task("slave-a", json!({"kind": "Src", "target_crates": ["crate-x"], "files": ["src/a.rs"]}))
            .await
            .unwrap();
        let tid2 = r2["task_id"].as_str().unwrap().to_string();
        let f = node.task_force(&tid2, "Bogus").await;
        assert_eq!(f["ok"], false, "{f}");

        // Force to Done is the agent's explicit override channel.
        let f2 = node.task_force(&tid2, "Done").await;
        assert_eq!(f2["ok"], true, "{f2}");
        assert_eq!(f2["state"], "Done");
        assert_eq!(node.task_show(&tid2).await["task"]["state"], "Done");
    }

    // -----------------------------------------------------------------------
    // A3: five-level may_i_touch + approval arbitration
    // -----------------------------------------------------------------------

    #[test]
    fn may_i_touch_exact_file_denied() {
        let mut s = state();
        s.zones.insert("src/a.rs".to_string(), json!({"owner": "other", "queued": []}));
        assert_eq!(
            check_may_i_touch(&s, &["src/a.rs".to_string()], "me"),
            MayIResult::Denied
        );
    }

    #[test]
    fn may_i_touch_active_task_file_denied() {
        let mut s = state();
        add_task(&mut s, "t1", "Src", &["crate-x"], &["src/owned.rs"], "other", "Working", &[]);
        assert_eq!(
            check_may_i_touch(&s, &["src/owned.rs".to_string()], "me"),
            MayIResult::Denied
        );
        // A finished task no longer claims the file.
        s.tasks.get_mut("t1").unwrap()["state"] = json!("Done");
        assert_eq!(
            check_may_i_touch(&s, &["src/owned.rs".to_string()], "me"),
            MayIResult::AutoApproved
        );
    }

    #[test]
    fn may_i_touch_same_module_escalates_level_2() {
        let mut s = state();
        add_task(&mut s, "t1", "Src", &["crate-x"], &["src/mod/one.rs"], "other", "Working", &[]);
        assert_eq!(
            check_may_i_touch(&s, &["src/mod/two.rs".to_string()], "me"),
            MayIResult::Escalated { level: 2 }
        );
    }

    #[test]
    fn may_i_touch_workspace_neighbor_escalates_level_3() {
        let mut s = state();
        add_task(&mut s, "t1", "Src", &["crate-a"], &["crates/a/src/lib.rs"], "other", "Working", &[]);
        assert_eq!(
            check_may_i_touch(&s, &["crates/b/src/lib.rs".to_string()], "me"),
            MayIResult::Escalated { level: 3 }
        );
    }

    #[test]
    fn may_i_touch_global_state_escalates_level_4() {
        let s = state();
        assert_eq!(
            check_may_i_touch(&s, &["Cargo.lock".to_string()], "me"),
            MayIResult::Escalated { level: 4 }
        );
    }

    #[test]
    fn may_i_touch_conflict_history_escalates_level_5() {
        let mut s = state();
        s.conflicts.insert(
            "c1".to_string(),
            json!({"id": "c1", "files": ["src/hot.rs"], "severity": "high", "ts": 1.0}),
        );
        assert_eq!(
            check_may_i_touch(&s, &["src/hot.rs".to_string()], "me"),
            MayIResult::Escalated { level: 5 }
        );
        // A path under a conflicted dir also escalates.
        assert_eq!(
            check_may_i_touch(&s, &["src/hot.rs/impl.rs".to_string()], "me"),
            MayIResult::Escalated { level: 5 }
        );
    }

    #[test]
    fn may_i_touch_fresh_file_auto_approves() {
        let s = state();
        assert_eq!(
            check_may_i_touch(&s, &["src/brand_new.rs".to_string()], "me"),
            MayIResult::AutoApproved
        );
        assert_eq!(check_may_i_touch(&s, &[], "me"), MayIResult::AutoApproved);
    }

    #[test]
    fn may_i_touch_same_owner_repeat_auto_approves() {
        let mut s = state();
        add_task(&mut s, "t1", "Src", &["crate-x"], &["src/mine.rs"], "me", "Working", &[]);
        assert_eq!(
            check_may_i_touch(&s, &["src/mine.rs".to_string()], "me"),
            MayIResult::AutoApproved
        );
    }

    #[tokio::test]
    async fn may_i_touch_auto_approves_and_escalates() {
        let node = Arc::new(bare_node("master", "m", None));
        {
            let mut s = node.state.lock().await;
            add_task(&mut s, "t1", "Src", &["crate-x"], &["src/mod/one.rs"], "slave-a", "Working", &[]);
        }
        // Fresh file, no claims -> auto-approved with a trace, never queued.
        node.on_rpc_request(&json!({
            "id": "r-fresh",
            "method": "may_i_touch",
            "from": "slave-b",
            "reply_to": "slave-b",
            "params": {"files": ["src/brand_new.rs"]},
        }))
        .await;
        // Same module as another owner's active task -> escalated (level 2).
        node.on_rpc_request(&json!({
            "id": "r-hot",
            "method": "may_i_touch",
            "from": "slave-b",
            "reply_to": "slave-b",
            "params": {"files": ["src/mod/two.rs"]},
        }))
        .await;
        {
            let s = node.state.lock().await;
            assert_eq!(s.approvals["r-fresh"]["auto"], true);
            assert_eq!(s.approvals["r-fresh"]["approved"], true);
            assert_eq!(s.escalations.len(), 1, "{:?}", s.escalations);
            assert_eq!(s.escalations[0]["req_id"], "r-hot");
            assert_eq!(s.escalations[0]["level"], 2);
        }
        // Digest: only the escalation is an action; the auto-approval trace is
        // noise (and counts as neither ack nor tick).
        let d = node.digest(None).await;
        let actions = d["actions"].as_array().unwrap();
        assert_eq!(actions.len(), 1, "{d}");
        assert_eq!(actions[0]["kind"], "approval_escalation");
        assert_eq!(d["noise_counts"]["ack"], 0);
        assert_eq!(d["noise_counts"]["tick"], 0);
    }

    #[tokio::test]
    async fn approval_decide_answers_escalations() {
        let node = bare_node("master", "m", None);
        let slave = bare_node("slave", "s", Some("m"));
        assert_eq!(slave.approval_decide("r1", "approve").await["ok"], false, "slave must be rejected");

        // Seed an escalation as on_rpc_request would.
        {
            let mut s = node.state.lock().await;
            s.escalations.push_back(json!({
                "req_id": "r1",
                "files": ["src/a.rs"],
                "owner": "slave-a",
                "level": 2,
                "reason": "same module",
                "reply_to": "slave-a",
                "ts": 1.0,
            }));
        }
        // queue leaves it pending.
        let q = node.approval_decide("r1", "queue").await;
        assert_eq!(q["ok"], true, "{q}");
        assert_eq!(q["decision"], "queue");
        {
            let s = node.state.lock().await;
            assert_eq!(s.escalations.len(), 1);
            assert!(s.approvals.is_empty());
        }
        // approve pops it and records a trace.
        let a = node.approval_decide("r1", "approve").await;
        assert_eq!(a["ok"], true, "{a}");
        assert_eq!(a["approved"], true);
        assert_eq!(a["trace"]["level"], 2);
        {
            let s = node.state.lock().await;
            assert!(s.escalations.is_empty());
            assert_eq!(s.approvals["r1"]["decision"], "approve");
        }
        // Unknown / already decided request -> error.
        assert_eq!(node.approval_decide("r2", "approve").await["ok"], false);
    }

    // -----------------------------------------------------------------------
    // A4: zone_steal + A5: persistence
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn zone_steal_is_master_only_and_forces_owner() {
        let node = bare_node("master", "m", None);
        let slave = bare_node("slave", "s", Some("m"));
        assert_eq!(slave.zone_steal("/z").await["ok"], false, "slave zone_steal must be rejected");
        {
            let mut s = node.state.lock().await;
            s.zones.insert("/z".to_string(), json!({"owner": "slave-a", "queued": []}));
        }
        let r = node.zone_steal("/z").await;
        assert_eq!(r["ok"], true, "{r}");
        assert_eq!(r["owner"], "m");
        assert_eq!(r["stolen"], true);
        assert_eq!(node.list_zones().await["zones"]["/z"]["owner"], "m");
    }

    #[tokio::test]
    async fn state_persists_tasks_across_restart() {
        let dir = temp_dir("persist");
        let cfg_dir = dir.to_str().unwrap().to_string();
        let node = bare_node_at("master", "m", None, &cfg_dir);
        let r = node
            .assign_task("slave-a", json!({"kind": "Src", "target_crates": ["crate-x"], "files": ["src/a.rs"]}))
            .await
            .unwrap();
        let tid = r["task_id"].as_str().unwrap().to_string();
        drop(node);

        // A fresh node on the same config dir restores the task table.
        let node2 = bare_node_at("master", "m", None, &cfg_dir);
        let show = node2.task_show(&tid).await;
        assert_eq!(show["ok"], true, "{show}");
        assert_eq!(show["task"]["owner"], "slave-a");
    }
}
