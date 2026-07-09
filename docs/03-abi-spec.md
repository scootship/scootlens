# 03 · ScootLens ABI 规范 v0

ABI 是 ScootLens 的核心资产。所有客户端（Agent/Console/CLI/MCP）只能通过 ABI 与内核交互。

## 传输与封装

- **传输**：WebSocket（主）+ HTTP（一次性调用）
- **封装**：JSON-RPC 2.0；事件流使用 server notification（`evt.*`）
- **认证**：连接握手时提交 capability token；每个调用在内核侧按作用域校验
- **版本**：`abi_version` 随握手返回。v0 期间允许破坏性变更（需 ADR）；v1 起向后兼容，破坏性变更需主版本号提升

## 通用约定

- `pid`：进程 ID（`p-` 前缀短 ID）
- `ref`：语义快照中的元素引用（如 `s3e17`，`s3` 为快照代数 generation）
- 所有调用支持 `timeout_ms`（默认 30000）
- 快照代数过期后对旧 ref 操作返回 `E_REF_STALE`，客户端必须重新 snapshot

## 系统调用表

标注：🔒 = 敏感作用域，默认需人工审批；P0-P5 = 交付阶段（见 [09-roadmap](09-roadmap.md)）

### proc — 进程管理

| 调用 | 参数 | 返回 | 所需能力 | 阶段 |
|---|---|---|---|---|
| `proc.spawn` | profile, engine?, quotas? | pid | `proc:spawn` | P1 |
| `proc.list` | filter? | ProcInfo[] | `proc:list` | P1 |
| `proc.info` | pid | ProcInfo | `proc:list` | P1 |
| `proc.kill` | pid | ok | `proc:kill` | P1 |
| `proc.suspend` | pid | ok | `proc:manage` | P3 |
| `proc.resume` | pid | ok | `proc:manage` | P3 |
| `proc.snapshot` | pid | snap_id | `proc:snapshot` | P3 |
| `proc.restore` | snap_id, engine? | pid | `proc:spawn` | P3 |

`ProcInfo` 返回 `pid/state/engine/profile`，进程已导航时还包含当前 `url`（用于 Console
按当前 origin 做安全提示与凭据绑定匹配）。

### nav / view / act — 观察与操作

| 调用 | 参数 | 返回 | 所需能力 | 阶段 |
|---|---|---|---|---|
| `nav.goto` | pid, url | NavResult | `nav@<origin>` | P1 |
| `nav.back` / `nav.forward` / `nav.reload` | pid | NavResult | `nav@<origin>` | P1 |
| `view.snapshot` | pid, opts(viewport_only?, max_nodes?, diff_from?) | A11ySnapshot(带 ref) | `view@<origin>` | P1 |
| `view.screenshot` | pid, opts(format, clip?) | image(base64/binary) | `view@<origin>` | P1 |
| `act.click` | pid, ref, opts? | ActResult | `act@<origin>` | P1 |
| `act.type` | pid, ref, text \| vault_ref | ActResult | `act@<origin>`（vault 另需 🔒`vault:use`） | P1 |
| `act.press` | pid, keys | ActResult | `act@<origin>` | P1 |
| `act.scroll` | pid, target(ref\|page), delta | ActResult | `act@<origin>` | P1 |
| `act.select` | pid, ref, values | ActResult | `act@<origin>` | P2 |
| `act.upload` | pid, ref, vfs_path | ActResult | 🔒 `act:upload@<origin>` | P2 |
| `act.takeover.start` | pid | ok + holder | 🔒 `act:takeover`（Console/人类操作者） | P4 |
| `act.takeover.end` | pid | ok | 🔒 `act:takeover`（仅 holder） | P4 |
| `act.point.click` | pid, x_ratio, y_ratio | ActResult | `act@<origin>` + 仅当前 pid 的接管 holder 可用，否则 `E_CAP_DENIED`（不经 takeover_gate 挂起队列，见 [ADR-0010](adr/0010-takeover-point-click.md)） | P4 |
| `dom.extract` | pid, query(css/schema) | 结构化数据 | `view@<origin>` | P2 |
| `js.exec` | pid, script, args? | value | 🔒 `js:exec@<origin>` | P2 |
| `evt.wait` | pid, cond(selector/url/net-idle/download), timeout | WaitResult | `view@<origin>` | P1 |

### state — State VFS

命名空间：`proc://<pid>/…`、`profile://<name>/…`、`vault://…`、`downloads://<pid>/…`

| 调用 | 参数 | 返回 | 所需能力 | 阶段 |
|---|---|---|---|---|
| `state.read` | path | value | 🔒 `state:read:<ns>@<origin>` | P2 |
| `state.write` | path, value | ok | 🔒 `state:write:<ns>@<origin>` | P2 |
| `state.list` | path | entries | `state:list:<ns>` | P2 |
| `state.export` | pid | StateBundle | 🔒 `state:export` | P3 |
| `state.import` | profile, StateBundle | ok | 🔒 `state:import` | P3 |

**vault 特例**：`vault://` 对 Agent 永远**只写不读**。Agent 在 `act.type` 中传 `vault_ref`，
由内核在引擎侧替换真实凭据——LLM 上下文永不出现明文密钥。

### net — 网络

| 调用 | 参数 | 返回 | 所需能力 | 阶段 |
|---|---|---|---|---|
| `net.rules.set` | scope(pid\|global), rules[] | ok | 🔒 `net:rules` | P2 |
| `net.rules.get` | scope | rules[] | `net:rules:read` | P2 |
| `net.log` | pid, filter | entries[] | `net:log@<origin>` | P2 |

### evt — 事件订阅

| 调用 | 参数 | 返回 | 所需能力 | 阶段 |
|---|---|---|---|---|
| `evt.subscribe` | pid?, topics[] | sub_id + notification 流 | `evt@<topic>` | P1 |
| `evt.unsubscribe` | sub_id | ok | — | P1 |

主题：`proc.lifecycle`、`nav.*`、`dom.mutation`、`net.request`、`net.response`、
`console.log`、`dialog.open`、`download.*`、`cap.request`、`act.takeover`

### cap — 能力

| 调用 | 参数 | 返回 | 所需能力 | 阶段 |
|---|---|---|---|---|
| `cap.request` | scopes[], reason | 审批结果/待审(approval_id) | —（任何主体可申请） | P2 |
| `cap.list` | — | 自身 subject + 已授予作用域 | —（自助，仅返回调用者自身令牌作用域） | P2 |
| `cap.grant` / `cap.revoke` | subject, scope | ok | 🔒 `cap:admin`（Console/管理员） | P2 |
| `cap.approve` | approval_id, decision(allow\|deny), remember? | ok | 🔒 `cap:admin` | P2 |
| `cap.pending` | — | 待审条目[]（id/subject/method/scope） | 🔒 `cap:admin` | P2 |

### wf — 工作流

| 调用 | 参数 | 返回 | 所需能力 | 阶段 |
|---|---|---|---|---|
| `wf.create` | spec(name, trigger(cron\|event\|manual), steps[], scopes[]) | ok | `wf:manage`（spec.scopes 须 ⊆ 创建者作用域） | P3 |
| `wf.list` | — | workflows[] | `wf:manage` | P3 |
| `wf.run` / `wf.cancel` | name | run 结果 / ok | `wf:manage` | P3 |

### obs — 观测

| 调用 | 参数 | 返回 | 所需能力 | 阶段 |
|---|---|---|---|---|
| `obs.journal` | filter, page | 审计条目 | `obs:journal` | P2 |
| `obs.trace` | pid | trace 流 | `obs:trace` | P2 |
| `obs.replay.export` | pid, journal_limit? | ReplayBundle（journal 哈希链段 + 画面帧） | 🔒 `obs:replay` | P4 |
| `sys.info` | — | 版本/引擎/配额水位 | — | P1 |

**takeover 语义**（ADR-0009）：接管期间非 holder 主体的 `act.*` 调用**挂起等待**，
归还控制后按序恢复；超过内核 `takeover_hold_timeout`（默认 30s）返回 `E_TIMEOUT`。
事件主题 `act.takeover`（`{active, holder}`，关键不丢）随 start/end/proc 终止广播。

**回放包**（`ReplayBundle`）：`journal` 为哈希链**连续尾段**（未按 pid 过滤，
`hash = sha256(prev + raw)` 可离线重放验证）；`frames` 为该 proc 的 PNG 帧
（`view.screenshot` 成功时由内核 FrameStore 采集，per-proc 环形 60 帧）。
proc 终止后仍可导出。

## 错误码

| 码 | 含义 | 客户端应对 |
|---|---|---|
| `E_CAP_DENIED` | 能力不足或被策略拒绝 | 走 `cap.request` 或放弃 |
| `E_APPROVAL_PENDING` | 敏感操作等待人工审批 | 订阅 `cap.request` 结果 |
| `E_PROC_NOT_FOUND` | pid 无效 | 刷新 `proc.list` |
| `E_REF_STALE` | 快照代数过期 | 重新 `view.snapshot` |
| `E_TIMEOUT` | 超时 | 重试/增大 timeout |
| `E_NET_BLOCKED` | 请求被网络规则拦截 | 预期行为，不重试 |
| `E_ENGINE_CRASH` | 引擎进程崩溃 | `proc.restore` 或重建 |
| `E_UNSUPPORTED` | 当前引擎不支持该调用 | 查 `sys.info` 能力矩阵 |
| `E_INVALID_ARG` | 参数错误 | 修正调用 |
| `E_QUOTA` | 超出配额 | 排队或释放资源 |
| `E_INTERNAL` | 内核内部错误 | 上报 issue，附 journal |

## MCP 投影

MCP server 是 ABI 的子集投影，工具命名 `scootlens_<domain>_<verb>`（如 `scootlens_view_snapshot`）。
MCP 层不做任何权限判断——一切由内核 Security Manager 强制。

P4 落地形态（ADR-0009）：`scootlens-mcp` 独立二进制，stdio 传输（rmcp）；由 MCP 客户端
spawn，以 capability 令牌连接 scootlensd gateway（`--url`/`--token` 或 `SCOOTLENS_URL`/
`SCOOTLENS_TOKEN`）。工具清单从 `method::ALL` 自动生成；连接级方法
（`evt.subscribe/unsubscribe`）不投影，条件等待用 `scootlens_evt_wait`。

## 变更流程

1. 提 ADR（动机/方案/兼容性影响）
2. 更新本规范 + `scootlens-abi` crate（类型与错误码同源）
3. 先写契约测试（TDD），再实现
4. `abi_version` 提升，CHANGELOG 记录

## 修订记录

| 日期 | 变更 | 协议表面影响 |
|---|---|---|
| P1 | crate 补充 `RpcOutcome`、`V2` re-export（dispatch/gateway 内部使用） | 无（wire 格式与方法表不变） |
| P2 | 令牌 wire 格式 `slt1.<b64url(claims)>.<b64url(sig)>`（见 ADR-0007）；新增 `cap.approve`/`cap.pending` 审批收件箱方法；`TokenClaims`、`NetRuleSet`/`NetRule` 类型进 `scootlens-abi`（新增契约快照 `token_claims`、`net_rule_set`）；落地 P2 方法实现：`state.read/write/list`、`net.rules.set/get`、`net.log`、`dom.extract`、`act.select`、`act.upload`、`act.type` 的 `vault_ref` 注入、`obs.journal/trace` | 方法表新增 2 项（approve/pending）；错误码新增语义 `E_QUOTA`（限速）；wire 封装不变（仍为 JSON-RPC 帧，令牌走连接握手） |
| P3 | OS 语义落地（见 ADR-0008）：`proc.suspend/resume`（`proc:manage`）、`proc.snapshot/restore`（`SnapId = snap-<16hex>` 内容寻址）、`state.export/import`（🔒；import 目标为 profile）、`wf.create/list/run/cancel`（`wf:manage`，spec.scopes 防提权）；`proc.spawn` 新增 `quotas` 参数（超 `quota_high_bytes` 需 `quota:high`）；新类型 `SnapId`/`QuotaSpec`/`QuotaPolicy`/`WfSpec`/`WfTrigger`/`WfStep`/`WfRetry` 进 `scootlens-abi`（契约快照 `snap_id`、`quota_spec`、`wf_spec`）；事件新增主题 `quota.exceeded`、`wf.run`，`BusEvent` 增可选 `dropped` 背压计数字段 | 方法表 P3 项全部由声明转为已实现；`EngineCaps` 新增 `lifecycle` 位（`#[serde(default)]` 向后兼容）；wire 封装不变 |
| P4 | 人机协同 + 生态接入（见 ADR-0009）：新方法 `act.takeover.start/end`（🔒 `act:takeover`，敏感集合 +1；挂起语义 + `act.takeover` 事件主题）；`obs.replay.export` 由声明转为实现（`ReplayBundle`/`ReplayLine`/`ReplayFrame` 类型进 `scootlens-abi`，契约快照 `replay_bundle`）；MCP 投影落地（`scootlens-mcp` stdio 代理，工具表由 `method::ALL` 生成）；`ABI_VERSION` 0.1.0 → 0.2.0 | 方法表 45 → 47（全表已实现）；事件主题 +`act.takeover`（关键不丢）；wire 封装不变 |
| P4 | 接管期间坐标点击（见 [ADR-0010](adr/0010-takeover-point-click.md)）：新方法 `act.point.click`（归一化视口坐标 `x_ratio`/`y_ratio` ∈ [0,1]；复用 `act@<origin>`，不新增 sensitive scope，风险面由"仅当前 pid 的接管 holder 可用"这一状态前提收紧，故意不经 `takeover_gate` 挂起队列）；HAL 新增 `InputAction::ClickAt`；Console Session 页画面在接管中可直接点击；`ABI_VERSION` 0.2.0 → 0.3.0 | 方法表 47 → 48；无新增 sensitive scope；wire 封装不变 |
