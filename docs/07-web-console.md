# 07 · Web Console 设计

**纯 Web，无桌面壳**（ADR-0004）。Console 是用户空间客户端：与 Agent 走同一 ABI + capability
令牌，无任何专用后门；它的存在本身就是对 ABI 完备性的持续 dogfooding。

## 技术栈

| 层 | 选型 | 说明 |
|---|---|---|
| 框架 | **Svelte 5 + Vite + TypeScript(strict)** | 轻、编译时框架，符合低资源理念 |
| 通讯 | 原生 WebSocket + JSON-RPC 客户端（自研薄封装） | 与 ABI 同构，零重量级依赖 |
| 实时画面 | screencast 帧流 → `<canvas>` | P4 评估 WebRTC 升级 |
| 图表/时间线 | 轻量自绘（svg）优先，避免重型图表库 | |
| 测试 | Vitest（单元）+ Playwright（UI e2e，仅 devDependency） | 覆盖率 ≥80% 同样适用 |
| 分发 | 构建产物由 `scootlensd` 静态托管（`/console`） | 无独立部署物 |

> Playwright 只作为 Console 的 UI 测试工具（dev-only），不进入运行时。后续可自举替换。

## 信息架构

```text
/console
├── Dashboard        # proc 列表、状态、配额水位、引擎健康（sys.info + evt 流）
├── Session /:pid    # 实时画面(screencast) · 人工接管(输入回注) · 快照树查看器
├── Inspector /:pid  # a11y/语义快照浏览 · net.log · console.log · 事件流
├── Approvals        # 审批收件箱：cap.request 卡片（作用域/参数/截图）→ 批准/拒绝/记忆规则
├── Journal          # 审计检索：主体/作用域/时间/结果过滤，哈希链校验状态
├── Replay /:id      # 回放播放器：syscall 时间线 + 画面帧对齐（P4）
├── Workflows        # wf 列表/编辑/运行历史（P3）
└── Settings         # 令牌管理(cap.grant/revoke) · vault 写入 · net 全局规则
```

## 关键交互

1. **人工接管（takeover）**：Session 页点击"接管" → Console 申请该 pid 的 `act@*` 临时提升 →
   鼠标/键盘事件经 ABI 回注（VNC 模式）；接管期间 Agent 的 act 调用被暂挂并提示
2. **审批卡**：`E_APPROVAL_PENDING` 挂起的调用实时推送；卡片必含：主体、作用域、参数摘要、
   当前页截图；支持"本次/永久（生成规则）/拒绝"
3. **vault 写入**：Settings 中单向写入表单；写入后仅显示 `vault_ref` 句柄

## 权限

Console 登录 = 持有用户令牌（默认含 `cap:admin`、`obs:*`）。多用户/RBAC 在 P4 之后按需引入，
v0 单管理员令牌足够。

## 阶段拆分

- P2：Dashboard + Approvals + Journal（安全闭环所需的最小 Console）
- P4：Session 实时画面/接管 + Inspector + Replay + Settings 完整版

## 实现状态（P2 + P4）

已落地于 `console/`（Svelte 5 runes + Vite + TS strict）：

- **连接**：首屏输入 Gateway 基址 + `slt1` 令牌 → `RpcClient` 经 `GET /ws?token=…` 握手；
  连接后建立**连接级 `evt.subscribe`（全主题）**驱动页面刷新与事件流；
  支持 `?token=…&connect=1[&base=…]` 快速接入（本地/e2e 便利，令牌本就走 URL 握手）
- **Dashboard**：`sys.info`（引擎/版本/进程配额水位）+ `proc.list` + Spawn/Kill 生命周期操作
- **Session**（P4）：`view.screenshot` 轮询 screencast（running 时 ~2fps，帧同时进内核
  FrameStore 供回放）；**人工接管**（`act.takeover.start/end`，接管期间 Agent 输入挂起、
  归还后恢复）；输入注入面板（语义快照元素清单 → `act.click/type/press` + `nav.goto`）；
  **画面直接点击**（[ADR-0010](adr/0010-takeover-point-click.md)）——接管中画面 `<img>`
  可直接点击，`containRect`/`clickRatio` 纯函数把点击偏移换算成归一化视口坐标 →
  `act.point.click`（仅当前 holder 可用，非接管中一律拒绝）；元素清单面板保留作为
  键盘可达的替代路径
- **Inspector**（P4）：语义快照文本（Agent 视角）、`net.log` 判定表、实时事件流（最近 100 条）
- **Approvals**：`cap.pending` 收件箱卡片（主体/方法/作用域/理由/时间）→ `cap.approve`（批准 / 批准并记忆 / 拒绝）
- **Journal**：`obs.journal` 审计表（按 pid 过滤、limit 可调），客户端轻量完整性自证（seq 连续性 + hash 存在性）
- **Replay**（P4）：`obs.replay.export` 导出/离线打开回放包；WebCrypto 逐行重放哈希链
  （`sha256(prev+raw)` + prev 链接）并显示校验状态；syscall 时间线 + 画面帧按 `ts_ms` 对齐，
  支持步进/拖动/仅本 pid 过滤与 `.json` 下载
- **Settings**（P4）：本会话令牌作用域（`cap.list`）、动态授权（`cap.grant/revoke`）、
  vault 单向写入（写后仅显示 `vault_ref`）、全局网络规则编辑（`net.rules.get/set`）；
  令牌签发在守护进程侧：`scootlensd --issue <subject>=<scope,…>`
- **分层与测试**：全部协议/校验逻辑集中在 `src/lib/`（`rpc` / `api` / `format` / `journal` /
  `session` / `replay` / `connect`），Vitest 单测覆盖 ≥80%（CI 门禁 #11）；Svelte 组件仅做展示；
  **Playwright UI e2e**（CI 门禁 #12）以 `scootlensd --engine mock` 全栈驱动关键路径：
  连接/spawn/screencast/接管挂起-恢复/审批闭环/journal 完整性/回放验链/设置动作
- **分发**：`npm run build` 产出 `console/dist/`，由 `scootlensd --console-dir console/dist` 静态托管于 `/`（`tower-http` ServeDir）；
  或以 `cargo build -p scootlensd --features embed-console` 把 `dist/` 编译进二进制
  （`include_dir`，单文件分发，无需外部静态目录；`--console-dir` 仍可覆盖）
