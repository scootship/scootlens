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
3. **vault 写入与站点绑定**：Settings 中单向写入表单；写入后仅显示 `vault_ref`
   句柄；可保存 `origin → username_ref/password_ref` 绑定，Session 只在当前 URL
   命中该 origin 时显示显式填充动作

## 权限

Console 会话与 Agent 走同一 ABI，无任何专用后门。人类用户经登录获得会话
（等价于全作用域 + 全自动审批的用户令牌），两种方式（docs/06-security-model.md
§Console 认证）：

- **用户名密码**（`scootlensd --admin-user` + `SCOOTLENS_ADMIN_PASSWORD` /
  `--admin-password-sha256`）
- **Microsoft Entra ID**（`--msauth-client-id/--msauth-tenant/--msauth-redirect-uri` +
  `SCOOTLENS_MSAUTH_CLIENT_SECRET`，白名单 `--msauth-allow-email/-domain` 必配）

登录换取 HttpOnly + SameSite=Strict 会话 cookie（内存态、12h、重启失效），WS 握手在缺
`?token=` 时回退 cookie。令牌握手路径保持不变（Agent/自动化）。多用户/RBAC 在 P4 之后按需引入。

## 阶段拆分

- P2：Dashboard + Approvals + Journal（安全闭环所需的最小 Console）
- P4：Session 实时画面/接管 + Inspector + Replay + Settings 完整版

## 实现状态（P2 + P4）

已落地于 `console/`（Svelte 5 runes + Vite + TS strict）：

- **布局**：标准 admin dashboard —— 左侧分组导航（概览 / 会话 / 治理 / 配置 + Approvals
  待审徽标 + 底部身份与退出），右侧顶栏（页名 + 连接状态）+ 主面板；窄屏（<1024px）
  侧栏折叠为抽屉（汉堡按钮 + 遮罩）
- **登录**（docs/06-security-model.md §Console 认证）：登录页支持**用户名密码**与
  **Microsoft Entra ID**（gateway `/auth/*` → HttpOnly 会话 cookie → 无 token 的 `/ws`
  cookie 握手），管理员不再把 `slt1` 令牌贴进 URL；「使用 Capability 令牌连接」保留为
  折叠的高级入口（Agent/自动化路径）。`?token=…&connect=1[&base=…]` 快速接入仍支持
  （e2e/自动化便利）
- **连接**：`RpcClient` 握手后建立**连接级 `evt.subscribe`（全主题）**驱动页面刷新与事件流
- **Dashboard**：`sys.info`（引擎/版本/进程配额水位）+ `proc.list` + Spawn/Kill 生命周期操作；
  进程表默认只列活跃进程（terminated 会持续累积，收起为「显示已终止（N）」开关，
  展开时置灰列出）
- **Session**（P4）：`view.screenshot` 轮询 screencast（running 时 ~2fps，帧同时进内核
  FrameStore 供回放；「放大」按钮弹全屏模态，轮询提速）；**人工接管**
  （`act.takeover.start/end`，接管期间 Agent 输入挂起、归还后恢复）；输入注入面板
  （语义快照元素清单 → `act.click/type/press` + `nav.goto`）；
  **画面直接点击**（[ADR-0010](adr/0010-takeover-point-click.md)）——接管中画面 `<img>`
  可直接点击，`containRect`/`clickRatio` 纯函数把点击偏移换算成归一化视口坐标 →
  `act.point.click`（仅当前 holder 可用，非接管中一律拒绝；驱动侧按
  `Page.getLayoutMetrics` 实测视口换算，避免 headless 窗口 UI 高度造成纵向偏移）；
  **接管键盘透传**——可打印字符与 Enter/Tab/方向键等经 `act.press` 注入
  （`pressKeyFor` 纯函数过滤，Cmd/Ctrl 组合键留给本地浏览器）；
  **快速 Kill** 当前会话并自动切到下一活跃进程；**保存登录态为 profile**
  （`state.export` → `state.import`，接管登录后一键存档，新开会话复用）；
  进程下拉只列活跃进程（当前选中的终止进程保留在末尾防跳变），终止会话给
  空态面板而非报错。旧版守护进程缺方法时报友好升级提示（`friendlyError`），
  不透出裸 "method not found"
- **Inspector**（P4）：语义快照文本（Agent 视角）、`net.log` 判定表、结构化事件流
  （topic 分族着色 + 载荷键值摘要 + 可选「仅当前 pid」过滤，点击行展开完整
  JSON 载荷）；终止进程给分面板空态提示而非全局报错
- **Approvals**：`cap.pending` 收件箱卡片（主体/方法/作用域/理由/时间）→ `cap.approve`
  （批准 / 批准并记忆 / 拒绝）；**自动审批清单**——管理员预先勾选可放行的敏感作用域族
  （与 `SENSITIVE_SCOPES` 对齐），命中的 `cap.request` 事件由 console 自动
  `cap.approve`（单次、不产生永久授权、照常进 journal；勾选存浏览器本地，默认全不勾）
- **Journal**：`obs.journal` 审计表（按 pid 过滤、limit 可调），客户端轻量完整性自证（seq 连续性 + hash 存在性）
- **Replay**（P4）：`obs.replay.export` 导出/离线打开回放包；WebCrypto 逐行重放哈希链
  （`sha256(prev+raw)` + prev 链接）并显示校验状态；syscall 时间线 + 画面帧按 `ts_ms` 对齐，
  支持步进/拖动/仅本 pid 过滤与 `.json` 下载
- **Settings**（P4）：本会话令牌作用域（`cap.list`）、动态授权（`cap.grant/revoke`）、
  vault 单向写入（写后仅显示 `vault_ref`；凭据可按名删除——`state.delete`，
  值从未回流、历史 journal 脱敏不回收）、站点凭据绑定（origin 匹配后在 Session
  中经 `act.type` + `vault_ref` 显式填充）、全局网络规则编辑（`net.rules.get/set`）、
  登录会话导入（cookie 粘贴 → `state.import`）与**已导入 profiles 管理**
  （[ADR-0011](adr/0011-profile-state-inspect-delete.md)：`state.list` 列名、
  `state.read` 元数据摘要——cookie 名/域/标志/字节数，值永不回流、`state.delete`
  整删或按 entry 单删）；令牌签发在守护进程侧：`scootlensd --issue <subject>=<scope,…>`
- **分层与测试**：全部协议/校验逻辑集中在 `src/lib/`（`rpc` / `api` / `format` / `journal` /
  `session` / `replay` / `connect`），Vitest 单测覆盖 ≥80%（CI 门禁 #11）；Svelte 组件仅做展示；
  **Playwright UI e2e**（CI 门禁 #12）以 `scootlensd --engine mock` 全栈驱动关键路径：
  连接/spawn/screencast/接管挂起-恢复/审批闭环/journal 完整性/回放验链/设置动作
- **分发**：`npm run build` 产出 `console/dist/`，由 `scootlensd --console-dir console/dist` 静态托管于 `/`（`tower-http` ServeDir）；
  或以 `cargo build -p scootlensd --features embed-console` 把 `dist/` 编译进二进制
  （`include_dir`，单文件分发，无需外部静态目录；`--console-dir` 仍可覆盖）
