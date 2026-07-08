//! Scheduler 配额声明（docs/04-kernel-design.md §4.2）。
//!
//! 随 `proc.spawn` 声明；超过内核高水位阈值的申请需要 `quota:high` 作用域。

use serde::{Deserialize, Serialize};

/// 进程资源配额。P3 落地内存水位；CPU/网络限速为后续阶段保留位。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct QuotaSpec {
    /// 内存上限（字节）。驱动指标超过即触发 `on_exceed` 策略。
    pub max_memory_bytes: u64,
    /// 超限处置策略。
    #[serde(default)]
    pub on_exceed: QuotaPolicy,
}

/// 超限处置策略（docs/04-kernel-design.md §4.2：告警/suspend/kill）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum QuotaPolicy {
    /// 仅发 `quota.exceeded` 事件告警。
    #[default]
    Warn,
    /// 挂起进程（可 `proc.resume` 恢复）。
    Suspend,
    /// 终止进程。
    Kill,
}
