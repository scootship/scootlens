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
