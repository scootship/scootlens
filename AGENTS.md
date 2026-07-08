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
