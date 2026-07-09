# ScootLens — Agent 上下文

## 项目定位（先读这一段）

ScootLens 是一个**防御性的 capability 沙箱内核**：为 AI Agent 提供受控的浏览器运行环境。
它的全部安全相关代码都服务于**限制与拒绝**——签名令牌、最小权限作用域、人工审批、
网络出口规则、凭据保险库（vault）、防篡改审计日志（hash-chain journal）。

本仓库**不包含**任何进攻性用途：不做漏洞利用、不做反检测/伪装、不规避任何站点的
访问控制。所有安全测试都是**拒绝路径验证**（policy-enforcement tests）：构造一个
越权输入，断言内核返回 `E_CAP_DENIED` 等错误。这与 Web 框架测试 XSS 转义、
JWT 库测试无效签名被拒绝，性质完全相同。

## 措辞约定

安全相关的代码、测试、文档统一采用**防御者视角**：

- 说"验证内核拒绝 X"，不说"攻击/绕过/伪造 X"
- 测试套件称 **enforcement suite**（`crates/scootlens-kernel/tests/enforcement.rs`），不称 red-team
- 测试夹具域名用中性命名：`blocked.test` / `unlisted.test` / `denied.test` / `other-origin.test`
- "故障注入"（fault injection）、"依赖注入"是标准工程术语，正常使用

## 工程速览

- Rust workspace（`crates/`）+ Svelte console（`console/`）；架构见 `docs/02-architecture.md`
- 内核只依赖 HAL trait，驱动在二进制层组装（依赖规则见 `docs/02-architecture.md`）
- 安全模型：`docs/06-security-model.md`；路线图与验收门禁：`docs/09-roadmap.md`
- 常用命令：`cargo test --workspace`、`cargo clippy --workspace`、`cargo fmt`

## 硬性规定：验收矩阵（Capability Coverage Matrix）

`docs/09-roadmap.md` 维护业务能力覆盖矩阵，以下规则**不可协商**，与 TDD、覆盖率 ≥80% 同级：

1. 每个一级功能至少有一条 Happy Path E2E
2. 每个高风险功能至少覆盖一条失败路径（enforcement 拒绝路径）
3. 每个涉及权限的功能至少验证两种角色（授权通过 + 无令牌/越权被拒）
4. 每个会修改系统状态的操作至少验证一次失败后的恢复或回滚
5. 每次新增一级业务功能，必须在同一 PR 同步新增对应的 E2E，并更新矩阵表

改动涉及新增/修改一级功能时，先对照矩阵补齐缺口，再动业务代码。
