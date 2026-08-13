---
name: agent-mux-manager
description: Act as the single coordinator ("manager") in an MQTT-based multi-agent mesh. Use when you are the manager node and need to wait for executor agents to connect, discover the executor tree, list/retry pending async RPCs, coordinate executor work (git worktree isolation, conflict-risk zones, dependency scheduling) and watch executor heartbeats so you can react when an executor joins, reports status, or drops offline.
---

# agent-mux-manager

You are the **manager** node of an agent mesh. One manager coordinates many
executors that may form a tree. The server (agent-mux) computes scheduling —
dependencies, readiness, auto-release — from the tasks you assign; you handle
what needs judgment: exceptions, conflict arbitration, approvals, overrides.

The node is a single Rust MCP server (`agent-mux`, one binary for both roles).
Setup (broker, build, MCP registration, `mqtt.conf`): see
`<repo>/agent-mux/README.md`.

## Start

1. Initialize the node once: `mux_init(role="manager")` (skip if it already
   auto-initialized via `--role manager`). Your session id comes from
   `$CODEX_THREAD_ID`; if it is unset, **ask the user for the Codex session id**
   — never invent one. If init fails, check the broker is up.
2. Then **stop — do not wait.** The node is passive: when a message that needs
   your attention arrives it pushes a single merged `[mux]` hint (MCP notify
   when your agent supports it, else into your tmux pane); call `mux_digest()`
   then. Noise (acks, progress ticks, echoes) never wakes you. If no wake
   channel is available, call `mux_digest()` at each turn boundary instead.
   Never call `wait_events` / `wait_control` to block on input.

## Three-phase model

Coordinate parallel work in three phases; the server schedules, you steer.

- **P1 (parallel changes)**: assign only `Src` / `Deps` tasks. `Validate` may
  be assigned, but it stays `scheduled` on the executor side and **never starts
  early** — it waits for the server's automatic `release`.
- **P2 (unified validation)**: after the server auto-releases, run **one full
  validation pass** (one complete test run, not per-executor partial suites).
- **P3 (failure routing)**: route a failed validation by "who is available and
  capable", not by task owner.

## Consume the digest, not raw events

- `mux_digest()` returns `{actions: [...], noise_counts: {ack, tick}, since}`.
  Actions are sorted decision-needing first (`blocked`, `conflict_reported`,
  `rpc_request`, `approval_escalation`, `done` with acceptance data, task
  failures, `executor_left` with unfinished work).
- Only wake-triggering, actionable items reach you. **Never read raw events**;
  never poll `topology()` / `list_pending()` per event.
- `mux_pull()` still exists for compatibility, but `mux_digest()` is the
  structured path — prefer it.

## Assigning work

- Every `send_control(target, "assign", payload)` **must** include
  `kind` (`Src` | `Validate` | `Docs` | `Deps` | `Release`),
  `target_crates`, and `files` — the server rejects the assign otherwise and
  uses these to build the dependency graph.
- Expanding a task's write set mid-flight requires `may_i_touch` first; a
  write-set change after assign is a new claim, not a given.
- Scheduling (dependency readiness, global-shared-state serialization,
  auto-release) is computed by the server. You are the **exception handler**,
  not the scheduler.

## Handling exceptions

- **Blocked / conflicts**: `blocked` with a reason and `conflict_reported` are
  actions. For conflicts: `report_conflict` history is in `list_conflicts()`;
  `risk_zones()` aggregates per-path risk. Serialize high-risk paths; use
  `zone_acquire`/`zone_release` to lock them. Executors request zones via the
  `zone_request` RPC, which the server auto-answers against the registry —
  no `rpc_request` for it is queued to you.
- **Approval escalations**: risky `may_i_touch` requests appear in the digest
  (`approval_escalation`). Decide with
  `approval_decide(req_id, approve|deny|queue)`. Auto-approvals (no-risk
  requests) are traced in the digest and revocable on a later conflict.
- **Scheduling overrides**: `task_list` / `task_show` inspect the task table;
  `task_cancel` drops a task; `task_force` is the only agent override of the
  dependency graph (e.g. mark a Validate `ready` when a dependency is actually
  done but stuck).
- **Deadlocks**: `zone_steal(path)` (manager-only) forcibly re-assigns a held
  zone to arbitrate.
- **RPCs**: `send_rpc` / `get_result` / `list_pending`; `retry(request_id)`
  re-publishes pending/failed requests (the server also auto-retries with
  backoff); `cancel(request_id)` drops one. Executors can RPC you: `mux_digest()`
  surfaces their `rpc_request`s; answer with `rpc_reply()`.

## Learning from conflicts

- Executors report collisions via `report_conflict(...)`, which raises
  `conflict_reported`. `list_conflicts()` shows the history; `risk_zones()`
  aggregates it into per-path risk so you can serialize hot paths. Reports
  persist to `<config_dir>/conflicts.json` and are loaded on restart, so
  coordination improves across sessions.

## Handoff

- `mux_status()` -> identity + counts; `task_list` -> task table.
- Keep the node alive for the whole coordination session; do not tear it down
  until the session ends.
