# 架构决策记录（ADR）

记录不可逆或影响面大的技术决策。新决策复制 [template.md](template.md)，编号递增，PR 评审通过后合入。
状态流转：Proposed → Accepted → (Superseded by ADR-XXXX)。

| # | 标题 | 状态 |
|---|---|---|
| [0001](0001-rust-core.md) | 核心语言采用 Rust | Accepted |
| [0002](0002-cdp-external-process.md) | Chromium 接入采用外部进程 CDP 而非 CEF 嵌入 | Accepted |
| [0003](0003-hal-webdriver-bidi.md) | HAL 语义对齐 WebDriver BiDi | Accepted |
| [0004](0004-pure-web-console.md) | 控制台采用纯 Web，不做桌面 GUI | Accepted |
| [0005](0005-agent-interface-mcp.md) | Agent 接口采用 原生 ABI + MCP 投影双通道 | Accepted |
| [0006](0006-tdd-mock-driver.md) | 测试策略以 Mock Driver 为基石 | Accepted |
| [0007](0007-token-wire-format.md) | Capability 令牌 wire 格式采用 `slt1.<claims>.<sig>` | Accepted |
| [0008](0008-p3-os-semantics-abi.md) | P3 OS 语义 ABI 面（suspend/snapshot/quota/wf） | Accepted |
| [0009](0009-p4-takeover-replay-mcp.md) | P4 ABI 增量 — 人工接管、回放导出与 MCP 投影落地 | Accepted |
| [0010](0010-takeover-point-click.md) | 接管期间坐标点击（`act.point.click`） | Accepted |
| [0011](0011-profile-state-inspect-delete.md) | 导入状态（profiles）与 vault 凭据的可观察与可删除（`state.delete` + 隐私摘要） | Accepted |
