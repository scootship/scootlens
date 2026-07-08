//! 进程状态与信息。

use scootlens_abi::Pid;
use serde::{Deserialize, Serialize};

/// 进程状态机（docs/04-kernel-design.md 4.1；Suspended 属 P3）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProcState {
    Spawning,
    Running,
    Terminated,
    Crashed,
}

/// `proc.list` / `proc.info` 返回项。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProcInfo {
    pub pid: Pid,
    pub state: ProcState,
    pub engine: String,
    pub profile: String,
}
