# ScootLens 设计文档

ScootLens 是一个 **Web Operating System**：以 Rust 内核管理 Web 会话（进程）、状态（文件系统）、
能力（权限）与浏览器引擎（驱动），为 AI Agent、人类操作者与外部系统提供统一的系统调用接口（ABI）。

## 工程铁律（不可协商）

1. **模块化** —— 严格的 crate 边界与单向依赖规则（见 [02-architecture](02-architecture.md)）
2. **分阶段实现** —— 按路线图阶段交付，禁止跨阶段偷跑（见 [09-roadmap](09-roadmap.md)）
3. **TDD** —— 先写测试，红→绿→重构；bugfix 必须先有复现测试（见 [08-engineering](08-engineering.md)）
4. **覆盖率 ≥ 80%** —— 每个 crate 行覆盖率 ≥ 80%，CI 强制，低于即失败
5. **验收门禁** —— 每个阶段有可度量的退出条件，门禁不过不进入下一阶段

## 阅读顺序

| # | 文档 | 内容 |
|---|------|------|
| 1 | [01-vision.md](01-vision.md) | 愿景、定位、非目标、竞品对照 |
| 2 | [02-architecture.md](02-architecture.md) | 总体架构、概念映射、模块划分、依赖规则 |
| 3 | [03-abi-spec.md](03-abi-spec.md) | 系统调用表 v0（ABI 规范、错误码、版本策略） |
| 4 | [04-kernel-design.md](04-kernel-design.md) | 内核子系统设计 |
| 5 | [05-engine-hal.md](05-engine-hal.md) | 引擎驱动层（HAL）与一致性测试套件 |
| 6 | [06-security-model.md](06-security-model.md) | Capability 安全模型与威胁模型 |
| 7 | [07-web-console.md](07-web-console.md) | 纯 Web 控制台设计 |
| 8 | [08-engineering.md](08-engineering.md) | 工程铁律细则：TDD、覆盖率、CI 门禁、unsafe 政策 |
| 9 | [09-roadmap.md](09-roadmap.md) | 分阶段路线图与各阶段验收门禁 |
| 10 | [adr/](adr/README.md) | 架构决策记录（ADR） |

## 文档维护规则

- 文档与代码同仓同 PR：行为变更必须同步更新受影响文档
- 任何 ABI 变更必须先提交 ADR 并通过评审
- 图使用 mermaid，保证 GitHub 直接可渲染
- 术语中英对照以 [01-vision.md](01-vision.md) 术语表为准
