//! Workflow Daemon 声明式 spec（docs/04-kernel-design.md §4.7）。
//!
//! 触发器（cron/事件/手动）+ 步骤（ABI 调用序列 + 重试）。以最小权限令牌
//! 运行（`wf:<name>` 主体，作用域不得超出创建者），**不是** Agent 编排器。

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// 工作流规格（`wf.create` 入参）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WfSpec {
    /// 名称（运行主体为 `wf:<name>`）；`[a-z0-9-]+`。
    pub name: String,
    /// 触发器。
    pub trigger: WfTrigger,
    /// 步骤序列（顺序执行，任一步骤重试耗尽即整轮失败）。
    pub steps: Vec<WfStep>,
    /// 运行时作用域（最小权限令牌）；必须是创建者有效作用域的子集。
    pub scopes: Vec<String>,
}

/// 触发器。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum WfTrigger {
    /// 5 段 cron：`分 时 日 月 周`（UTC；支持 `*` 与数值）。
    Cron { expr: String },
    /// 总线事件触发：主题匹配即运行。
    Event { topic: String },
    /// 仅 `wf.run` 手动触发。
    Manual,
}

/// 单个步骤：一次 ABI 调用。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WfStep {
    /// 系统调用方法名（须在方法表内）。
    pub method: String,
    /// 调用参数。
    #[serde(default)]
    pub params: Value,
    /// 失败重试（默认不重试）。
    #[serde(default)]
    pub retry: WfRetry,
}

/// 步骤重试策略：固定次数 + 指数退避。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct WfRetry {
    /// 最大重试次数（0 = 不重试）。
    pub max_attempts: u32,
    /// 首次退避毫秒数；之后按 2^n 递增。
    pub backoff_ms: u64,
}

impl Default for WfRetry {
    fn default() -> Self {
        Self {
            max_attempts: 0,
            backoff_ms: 100,
        }
    }
}
