# ADR-0009：P4 ABI 增量 — 人工接管、回放导出与 MCP 投影落地

- 状态：Accepted
- 日期：2026-07-08
- 关联：docs/09-roadmap.md（Phase 4）、docs/03-abi-spec.md、docs/07-web-console.md、ADR-0005

## 背景

P4 的目标是人机协同闭环 + Agent 生态接入（roadmap 门禁：MCP 完整任务含审批、
接管 e2e 事件序列、回放包离线验链、Console UI e2e）。这需要三块此前未定的协议表面：

1. **人工接管（takeover）**：Console 操作者对单个 proc 独占输入期间，Agent 的
   act 调用该如何处置（拒绝？丢弃？挂起？）
2. **回放包（obs.replay.export）**：P0 起就在方法表中声明，但载荷格式未定
3. **MCP server 形态**：ADR-0005 定了"rmcp 投影"，未定运行拓扑与工具生成方式

## 决策

### 1. takeover：`act.takeover.start/end` + 挂起语义

- 新方法 `act.takeover.start` / `act.takeover.end`（参数 `{pid}`），要求态作用域
  **`act:takeover`（无 origin 维度、列入敏感集合）**——持有 `act@<origin>` 的
  Agent **不能**升格为接管者（origin 授权不覆盖无 origin 要求）
- 接管期间非 holder 主体的 `act.*` 调用**挂起等待**（不拒绝、不丢弃），归还控制后
  按序恢复执行；等待超过 `KernelConfig::takeover_hold_timeout`（默认 30s）返回
  `E_TIMEOUT`。这与 docs/07"Agent 的 act 调用被暂挂并提示"一致
- 仅 Running 可接管；同 holder 幂等；他人抢占 → `E_INVALID_ARG`；非 holder 归还 →
  `E_CAP_DENIED`；proc 终止自动清除接管并唤醒等待者
- 新事件主题 `act.takeover`（`{active, holder}`），列入**关键不丢**主题
- `js.exec` 不在输入门内（它不是输入回注通道；如需限制走审批策略）

### 2. 回放包：journal 链段 + 帧序列（`ReplayBundle`）

- `obs.replay.export {pid, journal_limit?}` → `{bundle: ReplayBundle}`；类型进
  `scootlens-abi`（`format_version/pid/engine/exported_at_ms/journal[]/frames[]`）
- `journal` 是哈希链**连续尾段**（未按 pid 过滤）：离线验证只需
  `hash == sha256(prev + raw)` 且相邻行 `prev` 链接——与 journal.jsonl 行格式同构，
  Console 播放器用 WebCrypto 独立重放验证，按 pid 过滤只发生在展示层
- 帧来源：内核 FrameStore 在每次 `view.screenshot` 成功时采集（Console screencast
  轮询即持续产帧）；per-pid 环形缓冲 60 帧、全局 32 proc，终止后保留（事后取证）
- proc 终止后仍可导出；未知 pid → `E_PROC_NOT_FOUND`

### 3. MCP：stdio 薄代理二进制，工具表由方法表生成

- `scootlens-mcp` = **独立二进制**：MCP 客户端 spawn 它（stdio 传输，rmcp），它以
  capability 令牌（`--token`/`SCOOTLENS_TOKEN`）作为 **WS 客户端**连接 scootlensd
  gateway，把每个工具调用转发为一次 ABI 调用
- 工具清单从 `method::ALL` 自动生成：`scootlens_<domain>_<verb>`（`.`→`_`）；
  连接级方法 `evt.subscribe/unsubscribe` 不投影（订阅是 gateway 会话语义，
  MCP 客户端用 `scootlens_evt_wait`）。方法表增删自动反映到工具表，无第二真源
- 入参 schema 为宽松 object：权威校验在内核（serde 强校验 → `E_INVALID_ARG`），
  投影层零逻辑、零权限判断（ADR-0005 硬约束）；内核错误以工具级错误返回
  （content 内含 `abi_code`），传输故障才是协议级错误
- 不选内嵌 kernel 的进程内 MCP：MCP 客户端 spawn 的进程无法共享 scootlensd 的
  内核实例；代理模式保持单一强制点与单一事实内核

### 4. 配套

- `scootlensd --issue <subject>=<scope,…>`（可重复）：启动时签发受限令牌并打印
  （审批策略取默认：敏感=manual）。填补"除 admin 外无法发 agent 令牌"的空档，
  也是 Console/MCP e2e 的接入通道
- Console screencast 采用 `view.screenshot` 轮询（~2fps，running 时），不新增
  推流 ABI；WebRTC/CDP screencast 留待后续按 docs/07 评估
- `ABI_VERSION` 0.1.0 → **0.2.0**（v0 期间破坏性变更合规：方法表 +2、敏感集合 +1）
- OTLP trace 导出（roadmap 标注"可选项"）本阶段不做，入 backlog

## 备选方案

- **takeover 期间直接拒绝 Agent act**：Agent 需自行重试，接管窗口内任务必然失败；
  挂起语义让人工干预对 Agent 透明（只观察到延迟），拒绝
- **回放包只含 pid 过滤后的条目**：过滤段无法离线验链（seq 天然有洞），拒绝；
  完整段 + 展示层过滤两者兼得
- **MCP over Streamable HTTP 内嵌 scootlensd**：单进程更简，但主流 MCP host 以
  stdio 为默认接入面，且 HTTP 通道还需另做鉴权绑定；stdio 代理二者皆避，拒绝
- **每工具手写 JSON Schema**：与 ABI 漂移风险高，违背"投影自动生成"；拒绝

## 后果

- 方法表 45 → 47；事件主题 +1（`act.takeover`，关键不丢）；敏感作用域 +`act:takeover`
- 内核新增 takeover 表与 FrameStore（内存上界：60 帧 × 32 proc）；act.* 路径多一次
  接管门检查（无接管时为一次锁读，可忽略）
- 新依赖 rmcp（Apache-2.0，仅 `scootlens-mcp`）；deny/licenses 门禁通过
- CI 新增门禁 #12（Playwright Console e2e）与 #13（MCP stdio e2e）；coverage 忽略清单
  增加 `scootlens-mcp/src/main.rs`（薄组装层，由 e2e 覆盖）
- Console 增加 Session/Inspector/Replay/Settings 四页；`?token=…&connect=1` 快速接入
  （令牌本就经 URL query 完成 WS 握手，无新增暴露面）
