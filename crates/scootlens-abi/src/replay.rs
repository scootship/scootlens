//! 回放包类型（`obs.replay.export`，docs/03-abi-spec.md）。
//!
//! 回放包 = journal 哈希链**连续段**（未按 pid 过滤，可离线重放验链）加上
//! 画面帧序列（内核 FrameStore 采集）。Console Replay 播放器离线加载：
//! 逐行校验 `hash == sha256(prev + raw)` 与 `prev` 链接，再按 pid 过滤时间线。

use serde::{Deserialize, Serialize};

use crate::id::Pid;

/// 回放包格式版本（破坏性变更时递增）。
pub const REPLAY_FORMAT_VERSION: u32 = 1;

/// `obs.replay.export` 返回的回放包。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ReplayBundle {
    /// 包格式版本（当前 1）。
    pub format_version: u32,
    /// 目标进程。
    pub pid: Pid,
    /// 导出时内核引擎标识。
    pub engine: String,
    /// 导出时间（unix 毫秒）。
    pub exported_at_ms: u64,
    /// journal 哈希链连续尾段（旧→新）。含全部主体/方法（已脱敏），
    /// 离线验证：首行之后每行 `prev` 必须等于前行 `hash`。
    pub journal: Vec<ReplayLine>,
    /// 画面帧（旧→新；与 journal 条目按 `ts_ms` 对齐）。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub frames: Vec<ReplayFrame>,
}

/// journal 链上一行（与内核 journal.jsonl 行格式一致）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReplayLine {
    /// 单调序号。
    pub seq: u64,
    /// 前一行哈希（hex；创世为 64 个 `0`）。
    pub prev: String,
    /// 本行哈希 = sha256(prev + raw)（hex）。
    pub hash: String,
    /// 写入时的精确条目 JSON 字节序列（验证不依赖 JSON 规范化）。
    pub raw: String,
}

/// 一帧画面。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReplayFrame {
    /// 采集时间（unix 毫秒）。
    pub ts_ms: u64,
    /// 图像格式（当前恒 `png`）。
    pub format: String,
    /// base64 图像数据。
    pub data_base64: String,
}
