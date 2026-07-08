# 05 · 引擎驱动层（HAL）

## 设计原则

1. 语义对齐 **WebDriver BiDi**：trait 语义以 W3C 标准为参照，避免绑死 CDP 私有概念
2. **能力矩阵**：驱动声明支持的调用集；不支持→内核返回 `E_UNSUPPORTED`，一致性测试自动跳过
3. 内核只见 trait，驱动在二进制层注入（见依赖规则）

## HAL trait（骨架）

```rust
#[async_trait]
pub trait EngineDriver: Send + Sync {
    fn id(&self) -> EngineId;                    // "chromium" | "wpe" | "mock"
    fn capabilities(&self) -> EngineCaps;        // 能力矩阵
    async fn spawn(&self, profile: &ProfileSpec) -> Result<Box<dyn EngineHandle>>;
}

#[async_trait]
pub trait EngineHandle: Send + Sync {
    async fn navigate(&self, url: &Url) -> Result<NavResult>;
    async fn history(&self, dir: HistoryDir) -> Result<NavResult>;
    async fn snapshot(&self, opts: &SnapshotOpts) -> Result<A11ySnapshot>;
    async fn screenshot(&self, opts: &ScreenshotOpts) -> Result<Image>;
    async fn dispatch(&self, action: &InputAction) -> Result<ActResult>; // click/type/press/scroll…
    async fn eval(&self, script: &str, args: &[Value]) -> Result<Value>;
    async fn set_net_rules(&self, rules: &NetRules) -> Result<()>;
    async fn export_state(&self) -> Result<StateBundle>;
    async fn import_state(&self, bundle: &StateBundle) -> Result<()>;
    fn events(&self) -> EventStream;             // 引擎事件 → Event Bus
    async fn metrics(&self) -> Result<EngineMetrics>; // 内存/CPU，供 Scheduler
    async fn shutdown(self: Box<Self>) -> Result<()>;
}
```

## 语义快照（view.snapshot）规范

这是 token 成本最大的调用，规则：

1. 基于可访问性树（a11y tree），非原始 DOM
2. **剪枝**：不可见节点、纯装饰节点剔除；文本合并；`max_nodes` 截断（默认 800）+ 截断标记
3. **元素引用**：`s<gen>e<n>`；代数 `gen` 每次快照递增；旧代 ref → `E_REF_STALE`
4. **diff 模式**：`diff_from: <gen>` 只返回变更子树（P2）
5. 输出为紧凑文本格式（YAML-like 缩进树），非 JSON 全量——直接面向 LLM 消费

## 驱动实现

### Mock Driver（P0，TDD 基石）

- 纯内存假引擎：可编程页面模型（预置 DOM 树/导航图/网络行为脚本）
- 确定性：无网络、无真实渲染、毫秒级——内核与 gateway 的全部单元/集成测试跑在它上面
- 故障注入：可模拟崩溃、超时、慢响应，测试监督与恢复路径

### Chromium Driver（P1）

- **外部进程 CDP**（不用 CEF 嵌入，见 ADR-0002）：spawn 系统/捆绑 headless chromium，`--remote-debugging-port`，独立 user-data-dir
- 自研薄 CDP 客户端：类型从 `browser_protocol.json` 代码生成，只生成用到的 domain（Page/DOM/Runtime/Input/Fetch/Network/Accessibility/Target/Browser）
- screencast：`Page.startScreencast` → Console 实时画面
- 崩溃检测：进程退出 + WebSocket 断连双信号

### WPE Driver（P5）

- libwpe + WPEWebKit FFI（unsafe 隔离在此 crate，`// SAFETY` 注释强制）
- 目标：单 proc 内存 < Chromium 的 40%；嵌入式部署验证
- 自动化面：WebDriver BiDi（WebKit 支持进度）+ WebKitWebAutomation 补齐

### Servo Driver（观察）

- 跟踪 embedding API 成熟度，只保留 trait 适配层的可行性验证，不排期

## 一致性测试套件（Conformance Suite）

`scootlens-hal` 内置 `conformance` 模块：同一套测试跑所有驱动。

```rust
// 驱动 crate 中一行注册：
scootlens_hal::conformance::run_all!(ChromiumDriver::new(test_config()));
```

- 覆盖：导航、快照 ref 稳定性、输入动作、状态导入导出、事件流、崩溃恢复
- 按能力矩阵自动跳过不支持项并计数（跳过率进入验收门禁）
- 真实引擎的 conformance 在 CI 的 e2e job 跑（容器内 headless chromium），Mock 的跑在单元测试 job

## 性能预算（CI 基准测试守护，回归 >10% 失败）

| 指标 | Chromium | WPE 目标 |
|---|---|---|
| proc.spawn 冷启动 | < 1.5s | < 800ms |
| view.snapshot（典型页） | < 300ms | < 300ms |
| act.click 往返 | < 50ms | < 50ms |
| 单 proc 常驻内存 | 基线记录 | < 40% 基线 |
| 内核自身常驻内存 | < 50MB | — |
