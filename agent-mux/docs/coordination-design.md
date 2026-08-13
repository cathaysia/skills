# agent-mux 协调机制改造方案

> 本文档是 agent-mux 下一轮改造的**决策稿**：描述要解决的问题、设计原则、server 与 skill 的具体改动、实施顺序与验证方式。只做决策，不列备选方案。

---

## 1. 这个方案是为了解决什么问题

本轮改造源于一次真实的多 agent 并行开发（xidl 仓库，1 manager + 多个 executor 分工：改代码、跑测试/BDD、写文档）。流程能跑通，但暴露了三类问题：

### 1.1 无意义的状态同步（多发状态）

executor 把大量"状态"当成消息推给 manager，manager 再原样塞进 LLM 上下文：

- 进度 tick：`working` 反复上报，每次都是自然语言一条；
- 回声：`ready`/`done` 重复报告，`ctrl_ack` 确认又回一条；
- 空 wake：没有新内容也发 `[mux] new message arrived` 提示，一次会话里出现几十次。

结果：token 被消耗、agent 被反复唤醒、每次唤醒都要做一次"其实什么也没发生"的判定。**状态同步的粒度与"需要 agent 决策"的粒度严重不匹配。**

### 1.2 冲突依然发生

尽管有 zone lock 和冲突上报，`crates/xidl-jsonrpc` 上仍然记录了 3 条冲突，根因各不相同：

| 根因 | 具体表现 |
|---|---|
| 单一共享 worktree | fixer 和 test executor 同时改 `tests/protocol_complex.rs`，改代码的和跑测试的没有隔离 |
| 写集合扩张未申报 | fixer 的 plan 中途扩张进 test executor 的领地，分配时算好的边界失效 |
| 依赖边被未完成 WIP 阻塞 | BDD executor 链接 `xidl-jsonrpc` 时被另一个 executor 改到一半的 `session/mod.rs` 卡住（E0515），只能靠 clean-HEAD worktree 绕过，等 fixer 收尾 |
| zone lock 非权威 | fixer "claim" 了 zone，但 `list_zones` 始终是 `{}`，锁没有成为可查询的真相源 |

核心教训：**冲突不是发生在分配时，而是发生在依赖边和写集合随时间变化时**。靠"冲突发生后上报"补救，永远慢半拍。

### 1.3 Agent 读取了每个事件、做了每个决策

- 测试 executor 能不能对还没改完的 src 跑校验？—— 这种机械判定也要 agent 看一遍事件再拍板；
- 某个 RPC 是不是还 pending？—— 也要 agent 主动查；
- 该不该让另一个 executor 开始测试？—— 要靠 manager agent 盯着状态手工放行。

凡是**可计算的调度逻辑**，都不该占用 LLM 上下文。agent 只应该处理真正需要判断的事：异常、冲突仲裁、验收。

### 1.4 三个问题的共同点

> 协调数据应该被 **MCP 结构化消费**（digest、任务状态、审批队列），而不是全部转成自然语言塞给 agent；调度决策应该由 **server 计算**（依赖就绪、自动 release），agent 只在策略无法裁决时介入。

---

## 2. 设计原则

1. **依赖边在分配时计算，不在冲突时补救。** `assign` 时必须声明任务类型、目标 crate、文件集合，server 据此建依赖图，而不是事后发现撞车。
2. **测试等代码。** Validate 任务在所有依赖的 Src/Deps 任务完成前不进入 Ready；就绪由 server 自动放行，不需要 agent 手工盯着。
3. **策略做门，不做决策者。** 自动批准只覆盖"无风险"情形；任何有风险的触碰升级给 agent 仲裁。只靠策略拍板会粒度太粗，只靠 agent 拍板会消耗上下文——两者配合。
4. **数据由 MCP 消费，不塞给 agent。** 事件在 server 内部分类、聚合成 digest，只有 actionable 项才触发唤醒并进入上下文。
5. **优先级是内部机制，不是对外标签。** 事件分类用于"是否唤醒 / 是否进 digest / 噪音归因"，**绝不**在返回值里暴露任何 priority 字段。

---

## 3. server（agent-mux 二进制 / MCP 层）改动

### A1 事件分类 + digest

引入 **server 内部私有** 的事件分类：

```rust
enum EventClass { Action, Noise } // 仅内部使用，不出现在任何返回值
```

- `Action`（需要 agent 注意）：`blocked`（带 reason）、`conflict_reported`、`rpc_request`、`error`、`done`（带验收数据）、`executor_left`（且该 executor 还有未完成任务）、任务状态转换（`Working -> Failed` 等）。
- `Noise`：`ctrl_ack` 回声、纯进度 tick、`ready` 回声、`executor_joined`、空闲/已完成 executor 的 `executor_left`。

对外行为：

- 新 MCP 工具 `mux_digest` → `{ actions: [...], noise_counts: {ack: n, tick: n}, since: <ts> }`。
- `actions` 内部排序规则：**需要决策的在前**（blocked / conflict / rpc_request），信息性的（done）在后。这是排序规则，不是标签。
- 只在有 `Action` 时唤醒 agent；同批多个 action 合并成**一个** hint。空 wake（只有 noise）直接丢弃。
- `mux_pull` 保留作兼容，返回值与 `mux_digest` 同构（actions + counts），旧 skill 不受破坏。

### A2 任务状态机 + 依赖调度

server 持有持久化任务表：

```rust
struct Task {
    id: String,
    kind: TaskKind,          // Src | Validate | Docs | Deps | Release
    target_crates: Vec<String>,
    files: Vec<String>,
    owner: String,           // executor sid
    state: TaskState,        // Scheduled | Ready | Assigned | Working | Done | Failed
    depends_on: Vec<String>, // task ids，由 server 在 assign 时计算
}
```

调度规则（写死，不经过 agent）：

1. **依赖就绪判定**：`Validate(T)` 只有在"所有 `target_crates` 与 T 的 crate/workspace 依赖相交的 `Src`/`Deps` 任务"全部 `Done` 后，才从 `Scheduled` 变为 `Ready`。依赖关系在 assign 时按 crate 依赖图计算好，存进 `depends_on`。
2. **全局共享状态互斥**：触碰 `Cargo.lock`、`.git`、生成目录、根 manifest 的任务，全 mesh 最多一个 `Working`，其余排队。
3. **自动 release**：依赖清空后，server **自动** 向 validate owner 发送 `release` 控制消息。这一步不需要 agent 参与。
4. **assign 强校验**：`assign` 控制消息的 payload 现在必须带 `kind`、`target_crates`、`files`，缺一即拒绝并返回错误。

新增工具：`task_list`、`task_show`、`task_cancel`、`task_force`（`task_force` 是 agent 显式覆盖依赖图的唯一通道）。

### A3 审批门（粒度感知；策略是门，agent 是仲裁者）

`may_i_touch` 升级为五级影响检查，按顺序判定：

| 级别 | 条件 | 结果 |
|---|---|---|
| 1 | 精确同一文件已被他人 claim | deny / 排队 |
| 2 | 同一 module / crate | **升级**（绝不自动） |
| 3 | workspace 依赖邻居（template↔generated、crate A 依赖 B） | **升级** |
| 4 | 全局共享状态（Cargo.lock / .git / 生成目录） | 绝不自动，强制串行 |
| 5 | 冲突历史路径（来自 `risk_zones`） | **升级** |

自动批准**仅限**：文件从未被任何人触碰 + 无 zone 占用 + 无冲突历史；或同一 owner 的重复请求。其余全部进入 `escalations` 队列，通过 digest 浮出，由 agent 用新工具 `approval_decide <req_id> <approve|deny|queue>` 决策。

自动批准在 digest 中留一条 trace，且**可撤销**（后续冲突上报时可回滚该次批准）。

### A4 Zone lock 权威化(现在已经实现了 manager 才能加锁)

- `list_zones` 成为唯一真相源；`zone_acquire` 持久化 owner；`zone_release` 校验 owner，不匹配直接报错。
- 新增 `zone_steal <path>`，**仅 manager agent 可用**，用于仲裁死锁。
- A2 的互斥判定、A3 的影响检查都读取这个注册表——不再各自维护一份"以为有锁"的视图。

### A5 可靠性

- RPC 默认超时 60s，自动重试 + 退避；`retry` 处理已过期请求。
- 状态持久化到 `~/mqtt/state.json`：tasks、zones、conflicts、事件序号。重启后 digest 的 `since` 增量不丢。

---

## 4. skill 改动

### B1 `agent-mux-manager/SKILL.md`

- **三阶段模型**：
  - **P1（并行改造）**：只派 `Src`/`Deps` 任务。`Validate` 可以 assign，但 executor 侧状态是 `scheduled`，**绝不提前开跑**。
  - **P2（统一校验）**：server 自动 release 后，做**一轮完整**校验（一次全量测试，不逐 executor 单独跑）。
  - **P3（失败路由）**：校验失败按"谁可用 + 谁有能力"路由，不按任务 owner。
- **状态协议**：executor 只上报 4 类状态：`blocked` / `conflict` / `done` / `error`。禁止 tick、禁止回声。
- **消费纪律**：manager 只消费 digest 里的 actionable 项；绝不读原始事件；绝不每个事件全量 poll topology / pending。
- **写集合纪律**：每次 assign 都必须声明 kind + target_crates + files，中途写集合扩张必须先 `may_i_touch`。
- **角色定位**：manager 是"异常处理器"（仲裁冲突、审批升级、覆盖调度），不是调度器（调度由 server 算）。

### B2 `agent-mux-executor/SKILL.md`

- 同样的 4 类状态纪律：无 tick、无 ack 回声。
- 收到 assign 后，用 kind + files + target 向 manager 确认（即 `report_status` 带这些字段）。
- 触碰任何新文件前必须先 `may_i_touch`。
- **P1 期间绝不跑全量 suite**——等 `release` 控制消息。
- 测试失败统一上报 manager，由 manager 路由修复者，不在 executor 内自行扩大写集合。

---

## 5. 实施顺序与验证

### 实施顺序

1. **Skills 先行**（纯文档改动，立即生效，规范下一次运行）。
2. **A1**：事件分类 + digest + wake 合并。
3. **A2**：任务状态机 + 依赖调度 + 自动 release。
4. **A3 / A4**：审批门 + zone 权威化（共用注册表，一起做）。
5. **A5**：超时/重试 + 状态持久化。

### 验证清单

- **分类器单元测试**：ack / tick → noise；blocked / conflict / rpc_request → action；空 wake 不唤醒。
- **调度器测试**：2 个 src + 1 个 validate，断言 validate 在 src 全部 `Done` 前不是 `Ready`；跨 crate 依赖用例（crate A 依赖 B，B 未完成时 A 的 validate 阻塞）。
- **审批测试**：五级影响每级一个用例；自动批准仅覆盖无风险路径。
- **集成 3-executor 演练**：A 改 codec、B 改 transport、C 做校验——断言零人工干预、C 恰好被唤醒一次、release 由 server 自动发出。
- **回归**：`cargo test`、`cargo clippy --all-targets --all-features -- -D warnings` 通过。

---

## 6. 明确不做的事（范围边界）

- **不修改 MQTT 协议**：topic 布局、心跳、LWT 机制全部保持现状。
- **不修改 broker 配置**：mosquitto 不动。
- **不引入新协议/新通道**：所有新能力都是新增 MCP 工具，与现有工具并列。
- **不暴露优先级标签**：事件分类只是 server 内部机制。
- **digest_mode 默认开启**，提供 opt-out 开关用于灰度回滚。
