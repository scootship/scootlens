# 08 · 工程规范（铁律细则）

本文档是强制性的。CI 门禁自动执行其中一切可自动化的条款；不可自动化的条款由 PR review 把关。

## 8.1 模块化铁律

- 严格遵守 [02-architecture](02-architecture.md) 的 crate 边界与单向依赖图
- 新增 crate / 新增跨 crate 依赖：必须先提 ADR
- `scootlens-abi` 是唯一协议真源：类型、错误码在此定义，禁止在别处重复定义
- 公共 API 必须有 rustdoc；`cargo doc` 无 warning

## 8.2 TDD 铁律

1. **先测试后实现**：每个 PR 的 commit 历史应体现 红（测试）→ 绿（实现）→ 重构
2. **bugfix 必须先有失败的复现测试**，再修复
3. 测试金字塔：

| 层 | 对象 | 依赖 | 速度要求 |
|---|---|---|---|
| 单元 | 纯逻辑（作用域匹配、剪枝、规则求值…） | 无 IO | <10ms/用例 |
| 集成 | kernel + gateway + **Mock Driver** | 无网络、无真实引擎 | <100ms/用例 |
| 契约 | ABI 序列化/错误码/版本兼容（golden files） | scootlens-abi | 快 |
| 一致性 | HAL conformance suite × 每个驱动 | Mock 常跑；真实引擎在 e2e job | — |
| e2e | scootlensd + headless chromium + 本地固定测试站点 | 容器内确定性环境 | 全套 <10min |

4. **确定性**：e2e 用本地自托管测试站点（fixtures），禁止依赖公网站点；禁止 sleep 等待，一律 `evt.wait`
5. 快照/剪枝类输出使用 golden file 测试（`insta` crate）

## 8.3 覆盖率铁律：≥ 80%

- 工具：`cargo llvm-cov`（Rust）、`vitest --coverage`（Console）
- 阈值：**每 crate / console 包行覆盖率 ≥ 80%**，CI 低于即红
- 排除项白名单（仅限：代码生成产物、二进制 main 装配层）集中登记在 `coverage.toml`，新增排除需评审
- 禁止为凑覆盖率写无断言测试——review 把关，发现即打回

## 8.4 CI 验收门禁（全绿才可合并）

| # | 门禁 | 工具 |
|---|---|---|
| 1 | 格式 | `cargo fmt --check`、`prettier --check` |
| 2 | 静态检查 | `cargo clippy --all-targets -- -D warnings`、`eslint`、`svelte-check` |
| 3 | 全部测试 | `cargo test --workspace`、`vitest run` |
| 4 | 覆盖率 ≥80% | `cargo llvm-cov --fail-under-lines 80` |
| 5 | 依赖审计 | `cargo deny check`（advisory/license/依赖图规则） |
| 6 | 密钥扫描 | `gitleaks` |
| 7 | unsafe 检查 | forbid 属性核查 + SAFETY 注释 lint |
| 8 | e2e smoke | 容器内 chromium 全链路冒烟 |
| 9 | 性能基准（release 分支） | criterion 基准，关键指标回归 >10% 失败 |
| 10 | ABI 变更核查 | `scootlens-abi` 有 diff 时要求关联 ADR 链接 |

## 8.5 分支与提交

- trunk-based：短命分支 → PR → squash 合入 `main`；`main` 永远可发布
- Conventional Commits（`feat:`/`fix:`/`docs:`/`refactor:`/`test:`/`chore:`）
- PR 模板 checklist：测试先行？覆盖率达标？文档同步？需要 ADR？

## 8.6 Definition of Done

代码 + 测试（先行）+ rustdoc/文档同步 + tracing/journal 埋点 + 门禁全绿 + 阶段验收项勾选。

## 8.7 工具链版本

- Rust：stable，`rust-toolchain.toml` 锁定；MSRV 记录并 CI 验证
- Node：LTS，`.nvmrc` 锁定；包管理器 `pnpm`
- 一切版本升级走 PR + 门禁
