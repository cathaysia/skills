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
    pub connected: bool,
    pub subscribed: bool,
    pub shutting_down: bool,
    pub master_sid: Option<String>,
    pub last_state_change: f64,
}

impl State {
    fn new(master_sid: Option<String>, config_dir: &str) -> Self {
        State {
            registry: HashMap::new(),
            pending: HashMap::new(),
            rpc_meta: HashMap::new(),
            ctrl_queue: VecDeque::new(),
            events: VecDeque::new(),
            rpc_requests: VecDeque::new(),
            zones: HashMap::new(),
            zone_snapshot: Value::Null,
            watches: HashMap::new(),
            watch_events: VecDeque::new(),
            status: Value::Null,
            conflicts: load_conflicts_from_disk(config_dir).unwrap_or_default(),
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
                {
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
                        s.events.push_back(json!({
                            "kind": "slave_left",
                            "session_id": sid,
                            "reason": "heartbeat_timeout"
                        }));
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
                }
                if !left.is_empty() {
                    task_node.events_notify.notify_one();
                    task_node.wake();
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
        s.events.push_back(ev);
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
            Self::push_event(
                &mut s,
                json!({"kind": "slave_joined", "session_id": sid, "parent_id": parent, "info": info}),
            );
            let wake = self.wake.clone();
            drop(s);
            self.events_notify.notify_one();
            if let Some(w) = wake {
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
            Self::push_event(
                &mut s,
                json!({"kind": "slave_left", "session_id": sid, "reason": reason}),
            );
            s.watches.retain(|_, w| {
                w.get("watcher_sid").and_then(|v| v.as_str()) != Some(sid)
            });
            drop(s);
            self.publish(&self.topic(&["registry", sid]), None, true).await;
            self.events_notify.notify_one();
            self.wake();
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
            Self::push_event(
                &mut s,
                json!({"kind": "slave_joined", "session_id": sid, "parent_id": parent, "info": info}),
            );
            let wake = self.wake.clone();
            drop(s);
            self.events_notify.notify_one();
            if let Some(w) = wake {
                w.wake();
            }
        }
    }

    async fn on_status(self: &Arc<Node>, sid: &str, data: &Value) {
        if sid == self.sid || self.role != "master" {
            return;
        }
        let mut s = self.state.lock().await;
        let info = s
            .registry
            .entry(sid.to_string())
            .or_insert_with(|| json!({"sid": sid, "last_seen": json!(now_ts())}));
        info["last_seen"] = json!(now_ts());
        for k in ["state", "plan_files", "message", "blocked_reason", "parent_id", "role"] {
            if let Some(v) = data.get(k) {
                info[k] = v.clone();
            }
        }
        let ev = json!({"kind": "status", "session_id": sid, "info": info.clone()});
        Self::push_event(&mut s, ev);
        let wake = self.wake.clone();
        drop(s);
        self.events_notify.notify_one();
        if let Some(w) = wake {
            w.wake();
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
        {
            let mut s = self.state.lock().await;
            let info = s
                .registry
                .entry(ack_sid.clone())
                .or_insert_with(|| json!({"sid": ack_sid}));
            info["last_ctrl_ack"] = data.clone();
            Self::push_event(
                &mut s,
                json!({"kind": "ctrl_ack", "session_id": ack_sid, "ack": data.clone()}),
            );
        }
        self.events_notify.notify_one();
        self.wake();
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
        let req = json!({
            "request_id": rid,
            "method": method,
            "params": data.get("params").cloned().unwrap_or(Value::Null),
            "from": data.get("from").cloned().unwrap_or(Value::Null),
            "reply_to": reply_to,
            "ts": now_ts(),
        });
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
            Self::push_event(
                &mut s,
                json!({"kind": "rpc_request", "request_id": rid, "method": method,
                       "from": data.get("from").cloned().unwrap_or(Value::Null)}),
            );
        }
        self.rpc_notify.notify_one();
        self.events_notify.notify_one();
        self.wake();
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

    pub async fn report_status(&self, state: &str, plan_files: Option<Vec<String>>, message: &str, blocked_reason: &str) -> Value {
        let st = json!({
            "sid": self.sid,
            "parent_id": self.parent_id(),
            "role": self.role,
            "state": state,
            "plan_files": plan_files.unwrap_or_default(),
            "message": message,
            "blocked_reason": blocked_reason,
            "ts": now_ts(),
        });
        {
            let mut s = self.state.lock().await;
            s.status = st.clone();
        }
        self.publish(&self.topic(&["status", self.sid.as_str()]), Some(st.clone()), true).await;
        st
    }

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
        let mut events = Vec::new();
        while let Some(v) = s.events.pop_front() {
            events.push(v);
        }
        let mut watch = Vec::new();
        while let Some(v) = s.watch_events.pop_front() {
            watch.push(v);
        }
        json!({"control": control, "rpc_requests": rpc, "events": events, "watch": watch})
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

    // ---- zones ----

    pub async fn zone_acquire(&self, path: &str, owner: Option<String>, force: bool) -> Value {
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
                    return json!({"ok": false, "path": path, "owner": cur2, "queued": true, "note": "queue behind current owner"});
                }
            }
            let queued = s.zones.get(path).and_then(|z| z.get("queued")).cloned().unwrap_or(json!([]));
            s.zones.insert(path.to_string(), json!({"owner": owner, "queued": queued}));
        }
        self.publish_zones().await;
        json!({"ok": true, "path": path, "owner": owner})
    }

    pub async fn zone_release(&self, path: &str, owner: Option<String>) -> Value {
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
        json!({"ok": true, "path": path, "next_owner": next_owner})
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
        {
            let mut s = self.state.lock().await;
            s.conflicts.insert(cid.clone(), entry.clone());
            Self::push_event(
                &mut s,
                json!({"kind": "conflict_reported", "session_id": sid, "conflict": entry.clone(), "id": cid}),
            );
        }
        self.persist_conflicts().await;
        self.publish_conflicts().await;
        self.events_notify.notify_one();
        self.wake();
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
        let mut agg: HashMap<String, (i64, std::collections::BTreeSet<String>, f64)> = HashMap::new();
        for e in s.conflicts.values() {
            let mut paths: Vec<String> = Vec::new();
            if let Some(files) = e.get("files").and_then(|v| v.as_array()) {
                for f in files {
                    if let Some(p) = f.as_str() {
                        if !p.is_empty() {
                            paths.push(p.to_string());
                        }
                    }
                }
            }
            if let Some(z) = e.get("zone").and_then(|v| v.as_str()) {
                if !z.is_empty() {
                    paths.push(z.to_string());
                }
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
        let mut ranked: Vec<(&String, &(i64, std::collections::BTreeSet<String>, f64))> = agg.iter().collect();
        ranked.sort_by(|a, b| {
            let ca = b.1 .0.cmp(&a.1 .0);
            if ca != std::cmp::Ordering::Equal {
                return ca;
            }
            b.1 .2.partial_cmp(&a.1 .2).unwrap_or(std::cmp::Ordering::Equal)
        });
        let out: Vec<Value> = ranked
            .iter()
            .map(|(pa, (count, sevs, last))| {
                json!({"path": pa, "count": count, "severities": sevs.iter().cloned().collect::<Vec<_>>(), "last": last})
            })
            .collect();
        json!({"risk_zones": out})
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
            let _ = std::fs::write(path, text + "\n");
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
