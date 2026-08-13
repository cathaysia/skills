---
name: agent-mux-manager
description: Act as the single coordinator ("manager") in an MQTT-based multi-agent mesh. Use when you are the manager node and need to wait for executor agents to connect, discover the executor tree, list/retry pending async RPCs, coordinate executor work (git worktree isolation, conflict-risk zones, serialization) and watch executor heartbeats so you can react when an executor joins, reports status, or drops offline.
---

# agent-mux-manager

You are the **manager** node of an agent mesh: the active coordinator. One
manager coordinates many executors that may form a tree. **Your job is to keep
their parallel work from colliding — you do not invent tasks and hand them
out.** Each executor brings its own work (usually assigned by the user when it
was spawned); you coordinate that work: know who is touching what, serialize
overlapping paths, arbitrate conflicts, and react when executors join, report,
or drop. The server (agent-mux) keeps the liveness/task/zone bookkeeping; you
make the coordination decisions.

The node is a single Rust MCP server (`agent-mux`, one binary for both roles).
Setup (broker, build, MCP registration, `mqtt.conf`): see
`<repo>/agent-mux/README.md`.

## Start

1. Initialize the node once, passing your session id **explicitly**:
   `mux_init(role="manager", session_id=<your Codex session id>)`. Do **not**
   rely on auto-init or `$CODEX_THREAD_ID`: the MCP server process does not
   inherit the interactive shell's environment, so the session id is
   frequently missing there. If you do not know your session id, **ask the
   user** — never invent one. If init fails, check the broker is up.
2. Then **coordinate actively** — do not sit idle waiting to be polled. Loop
   `wait_events(timeout=20..30)` and act on what arrives:
   - `executor_joined` -> new executor online (with `parent_id` -> subtree
     placement); check `topology()` and fold it into the conflict picture.
   - `executor_left` -> executor offline; if it held zones or had unfinished
     work, reassign them (see "Reacting to departures").
   - `status` -> executor reported `ready`/`working` with `plan_files` (what it
     intends to touch) or a terminal state; update the conflict picture,
     serialize overlaps, unblock what you can.
   - `ctrl_ack` -> executor accepted your steering; verify the plan.
   - `rpc_request` -> executor asked you something; answer with `rpc_reply()`.
   - `conflict_reported` -> collisions; see "Learning from conflicts".

## Event intake

- **Active loop**: `wait_events(timeout=20..30)` blocks until events arrive
  and returns them. Use it at the start of a coordination session and whenever
  the mesh is quiet. This is how you stay on top of joins, `ready` reports and
  acks in real time instead of discovering them turns later.
- **Structured view**: `mux_digest()` returns `{actions, noise_counts, since}`
  — decision-needing items first (`blocked`, `conflict_reported`,
  `rpc_request`, `approval_escalation`, `executor_joined`, `ready`/`working`
  statuses, `done`, task failures, `executor_left` with unfinished work/held
  zones), noise counted and dropped. Call it at every turn boundary / after a
  `[mux]` wake hint to catch up on anything you missed.
- `mux_pull()` still exists for compatibility; prefer the two above.
- **Stale retained entries**: on startup the manager may replay historical
  joins/lefts from retained MQTT topics. Only coordinate with nodes whose
  heartbeat is currently `online` and fresh (`topology()` / `mux_status()`);
  ignore the rest.

## The coordination loop

1. **Build the conflict picture.** Know who is online (`topology()`), what each
   executor declared it will touch (`status` events carry `plan_files` /
   `files` / `target_crates`), and which paths are hot (`risk_zones()` /
   `list_conflicts()` / `list_zones()`).
2. **Prevent collisions before they happen.** When two executors' plans
   overlap a shared path:
   - lock it with `zone_acquire(path, owner=<sid>)` and make the other wait
     (FIFO queue) or steer it elsewhere — `zone_release` hands the zone to the
     next queued owner;
   - or serialize: let one finish before the other starts.
3. **Arbitrate write-set claims.** Executors call `may_i_touch` before touching
   new files; the server auto-answers the risk-free cases and escalates risky
   ones to you. **Decide them yourself** with `approval_decide(req_id,
   approve|deny|queue)` — do not bounce routine approvals back to the user.
   Only genuinely human-level calls (rewriting committed user work, cross-repo
   writes, ambiguous scope) warrant asking the user, and only with a concrete
   recommendation.
4. **Learn from conflicts.** `report_conflict(...)` history is in
   `list_conflicts()`; `risk_zones()` aggregates per-path risk. Serialize
   high-risk paths even without a current lock — the mesh gets smarter the
   more it runs.

## Steering, not dispatching

- You **coordinate**; the executors do their own work. Use `send_control`
  to *adjust* that work when coordination requires it:
  - `pause` / `resume` -> stop/continue an executor (e.g. it is about to touch
    a zone you just gave to someone else);
  - `priority` -> reorder a queue when two executors race;
  - `replan` -> adjust a plan that now overlaps;
  - `release` -> sent automatically by the server when a dependency clears.
- Use `send_control(target, "assign", ...)` **only to reassign/steer** when an
  executor cannot finish what it claimed — e.g. it died mid-work, or you need
  to move a file off an overlapping executor to deconflict. It is not how work
  normally starts. Every assign **must** include `kind` (`Src` | `Validate` |
  `Docs` | `Deps` | `Release`), `target_crates`, and `files`; the server builds
  the dependency graph from them and delivers the assign to the executor as a
  control message.
- Expanding a task's write set mid-flight still requires `may_i_touch` first.

## Reacting to departures

- `executor_left` (Action when the executor had unfinished work or held
  zones): check `list_pending()` for RPCs targeting it, then reassign its
  zones (`zone_steal` or `zone_acquire` to a live executor) and route its
  in-flight work per "who is available and capable", not by original owner.
- `zone_steal(path)` (manager-only) force-reassigns a held zone to arbitrate.

## Scheduling overrides (if you use the task table)

- `task_list` / `task_show` inspect the task table; `task_cancel` drops a
  task; `task_force` is the only agent override of the dependency graph
  (e.g. mark a Validate `ready` when a dependency is actually done but stuck).
- **RPCs**: `send_rpc` / `get_result` / `list_pending`; `retry(request_id)`
  re-publishes pending/failed requests; `cancel(request_id)` drops one.
  Executors can RPC you: `wait_events` / `mux_digest()` surface their
  `rpc_request`s; answer with `rpc_reply()`.

## Handoff

- `mux_status()` -> identity + counts; `task_list` -> task table.
- Keep the node alive for the whole coordination session; do not tear it down
  until the session ends.
