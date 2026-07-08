//! 内核事件总线载荷。

use scootlens_abi::Pid;
use serde::{Deserialize, Serialize};
use url::Url;

use crate::proc::ProcState;

/// 总线事件：单调 `seq` + 关联进程 + 载荷。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BusEvent {
    pub seq: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pid: Option<Pid>,
    #[serde(flatten)]
    pub payload: BusPayload,
}

/// 事件载荷（`topic` 字段区分主题，序列化名与 [`BusPayload::topic`] 一致）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "topic")]
pub enum BusPayload {
    /// `proc.lifecycle`：进程状态迁移。
    #[serde(rename = "proc.lifecycle")]
    ProcLifecycle { state: ProcState },
    /// `nav`：页面导航完成。
    #[serde(rename = "nav")]
    Navigated { url: Url },
    /// `console`：引擎控制台输出。
    #[serde(rename = "console")]
    ConsoleLog { text: String },
}

impl BusPayload {
    /// 事件主题名（evt.subscribe 过滤用）。
    pub fn topic(&self) -> &'static str {
        match self {
            BusPayload::ProcLifecycle { .. } => "proc.lifecycle",
            BusPayload::Navigated { .. } => "nav",
            BusPayload::ConsoleLog { .. } => "console",
        }
    }
}
