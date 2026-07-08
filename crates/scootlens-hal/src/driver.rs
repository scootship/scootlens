//! EngineDriver / EngineHandle trait（docs/05-engine-hal.md）。

use std::sync::Arc;

use async_trait::async_trait;
use scootlens_abi::AbiError;
use scootlens_abi::{NetDecision, NetRequestSummary};
use serde_json::Value;
use tokio::sync::broadcast;
use url::Url;

use crate::types::{
    A11ySnapshot, ActResult, EngineCaps, EngineEvent, EngineMetrics, HistoryDir, InputAction,
    NavResult, ProfileSpec, SnapshotOpts, StateBundle,
};

/// HAL 统一结果类型：错误即 ABI 错误（单一错误分类学）。
pub type HalResult<T> = Result<T, AbiError>;

/// 请求策略回调：规则引擎属于内核，驱动逐请求询问（强制点在引擎侧）。
///
/// 实现必须快速无阻塞（同步判定）。驱动对被拒的**主文档**请求
/// 返回 `E_NET_BLOCKED`；子资源请求静默失败并发出
/// [`EngineEvent::NetRequest`] 事件。
pub trait RequestPolicy: Send + Sync {
    fn decide(&self, req: &NetRequestSummary) -> NetDecision;
}

/// 引擎驱动：负责 spawn 引擎实例。
#[async_trait]
pub trait EngineDriver: Send + Sync {
    /// 驱动标识："mock" | "chromium" | "wpe" | …
    fn id(&self) -> &'static str;

    /// 能力矩阵。
    fn capabilities(&self) -> EngineCaps;

    /// 启动一个引擎实例（一 proc 一引擎进程一 profile）。
    async fn spawn(&self, profile: &ProfileSpec) -> HalResult<Box<dyn EngineHandle>>;
}

/// 单个引擎实例句柄。
#[async_trait]
pub trait EngineHandle: Send + Sync {
    async fn navigate(&self, url: &Url) -> HalResult<NavResult>;

    /// 当前页信息。
    async fn page_info(&self) -> HalResult<NavResult>;

    /// 历史移动（back/forward）。历史耗尽时为 no-op，返回当前页。
    async fn history(&self, dir: HistoryDir) -> HalResult<NavResult>;

    /// 重新加载当前页。
    async fn reload(&self) -> HalResult<NavResult>;

    /// 语义快照：每次调用快照代数 +1，旧代 ref 立即过期。
    async fn snapshot(&self, opts: &SnapshotOpts) -> HalResult<A11ySnapshot>;

    async fn screenshot(&self) -> HalResult<Vec<u8>>;

    /// 输入动作。对过期代数的 ref 必须返回 `E_REF_STALE`。
    async fn dispatch(&self, action: &InputAction) -> HalResult<ActResult>;

    async fn eval(&self, script: &str, args: &[Value]) -> HalResult<Value>;

    async fn export_state(&self) -> HalResult<StateBundle>;

    async fn import_state(&self, bundle: &StateBundle) -> HalResult<()>;

    /// 安装/清除请求策略（None = 放行一切）。
    ///
    /// 无 `net_rules` 能力的驱动返回 `E_UNSUPPORTED`。
    async fn set_request_policy(&self, policy: Option<Arc<dyn RequestPolicy>>) -> HalResult<()>;

    /// 引擎事件流（broadcast：订阅后才收到事件）。
    fn events(&self) -> broadcast::Receiver<EngineEvent>;

    async fn metrics(&self) -> HalResult<EngineMetrics>;

    /// 关闭引擎实例（幂等；关闭后句柄不再可用）。
    async fn shutdown(&self) -> HalResult<()>;
}
