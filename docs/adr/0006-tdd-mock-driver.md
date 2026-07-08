# ADR-0006：测试策略以 Mock Driver 为基石

- 状态：Accepted
- 日期：2026-07-08

## 背景

TDD 与 80% 覆盖率是铁律，但真实浏览器引擎慢、非确定、CI 成本高。若内核测试依赖真实引擎，
TDD 循环速度与覆盖率目标都无法达成。

## 决策

`driver-mock`（可编程内存假引擎）在 Phase 0 先于任何真实驱动交付：

- 内核、gateway、安全、VFS 的全部单元/集成测试只依赖 mock（毫秒级、确定性）
- 支持故障注入（崩溃/超时/慢响应），覆盖监督与恢复路径
- 真实引擎只在 HAL conformance suite 与 e2e job 中出现（容器内锁定版本 chromium + 本地 fixtures 站点，禁公网）

## 备选方案

- 全部测试跑真实 chromium：CI 10 倍慢、随机失败侵蚀门禁公信力；拒绝
- 只 mock 不做真实 e2e：驱动语义漂移无法发现；拒绝（双层都要）

## 后果

- mock 与真实驱动的行为一致性靠同一套 conformance suite 双向约束
- mock 本身也是被测对象（页面模型逻辑计入覆盖率）
