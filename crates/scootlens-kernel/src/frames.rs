//! 回放帧存储：per-pid 环形缓冲（docs/03-abi-spec.md `obs.replay.export`）。
//!
//! `view.screenshot` 成功即采集一帧。内存上限：每 proc 保留最近
//! [`MAX_FRAMES_PER_PID`] 帧；全局最多 [`MAX_PIDS`] 个 proc（超出时
//! 淘汰最久未更新者）。进程终止后帧保留（事后回放），随内核销毁。

use std::collections::{HashMap, VecDeque};
use std::sync::{Mutex, PoisonError};

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64;
use scootlens_abi::{Pid, ReplayFrame};

const MAX_FRAMES_PER_PID: usize = 60;
const MAX_PIDS: usize = 32;

struct StoredFrame {
    ts_ms: u64,
    png: Vec<u8>,
}

/// 帧存储。
#[derive(Default)]
pub(crate) struct FrameStore {
    inner: Mutex<HashMap<Pid, VecDeque<StoredFrame>>>,
}

impl FrameStore {
    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<Pid, VecDeque<StoredFrame>>> {
        self.inner.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// 采集一帧。
    pub fn record(&self, pid: &Pid, ts_ms: u64, png: Vec<u8>) {
        let mut map = self.lock();
        if !map.contains_key(pid) && map.len() >= MAX_PIDS {
            // 淘汰最久未更新的 proc（其最新帧时间最小）
            if let Some(stalest) = map
                .iter()
                .min_by_key(|(_, q)| q.back().map_or(0, |f| f.ts_ms))
                .map(|(p, _)| p.clone())
            {
                map.remove(&stalest);
            }
        }
        let q = map.entry(pid.clone()).or_default();
        q.push_back(StoredFrame { ts_ms, png });
        while q.len() > MAX_FRAMES_PER_PID {
            q.pop_front();
        }
    }

    /// 导出某 proc 的全部帧（旧→新，base64 编码）。
    pub fn export(&self, pid: &Pid) -> Vec<ReplayFrame> {
        self.lock().get(pid).map_or_else(Vec::new, |q| {
            q.iter()
                .map(|f| ReplayFrame {
                    ts_ms: f.ts_ms,
                    format: "png".into(),
                    data_base64: BASE64.encode(&f.png),
                })
                .collect()
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pid(n: u32) -> Pid {
        format!("p-f{n}").parse().expect("pid")
    }

    #[test]
    fn records_and_exports_in_order() {
        let s = FrameStore::default();
        s.record(&pid(1), 10, vec![1]);
        s.record(&pid(1), 20, vec![2]);
        let frames = s.export(&pid(1));
        assert_eq!(frames.len(), 2);
        assert_eq!((frames[0].ts_ms, frames[1].ts_ms), (10, 20));
        assert_eq!(frames[0].format, "png");
        assert!(s.export(&pid(9)).is_empty(), "unknown pid yields no frames");
    }

    #[test]
    fn ring_buffer_caps_per_pid() {
        let s = FrameStore::default();
        for i in 0..(MAX_FRAMES_PER_PID as u64 + 5) {
            s.record(&pid(1), i, vec![0]);
        }
        let frames = s.export(&pid(1));
        assert_eq!(frames.len(), MAX_FRAMES_PER_PID);
        assert_eq!(frames[0].ts_ms, 5, "oldest frames dropped first");
    }

    #[test]
    fn evicts_stalest_pid_beyond_global_cap() {
        let s = FrameStore::default();
        for n in 0..(MAX_PIDS as u32) {
            s.record(&pid(n), u64::from(n) + 1, vec![0]);
        }
        s.record(&pid(999), 10_000, vec![0]);
        assert!(s.export(&pid(0)).is_empty(), "stalest pid evicted");
        assert_eq!(s.export(&pid(999)).len(), 1);
        assert_eq!(s.export(&pid(1)).len(), 1, "fresher pids survive");
    }
}
