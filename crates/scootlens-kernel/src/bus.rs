//! 内核事件总线：载荷定义 + 自研发布/订阅通道。
//!
//! 背压策略（docs/04-kernel-design.md §4.7）：
//! - **高频可丢主题**（`nav` / `console` / `net.request`）：每订阅者队列有界，
//!   队满丢最旧事件并计数；计数随下一条送达事件的 `dropped` 字段带给订阅者。
//! - **关键不丢主题**（`proc.lifecycle` / `cap.request` / `quota.exceeded` / `wf.run`）：
//!   无界入队，永不丢弃——审计与安全语义优先于内存占用。

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError, Weak};

use scootlens_abi::{NetRequestSummary, Pid, QuotaPolicy};
use serde::{Deserialize, Serialize};
use tokio::sync::Notify;
use url::Url;

use crate::proc::ProcState;

/// 总线事件：单调 `seq` + 关联进程 + 载荷。
///
/// `dropped` 仅在投递时置位：表示该订阅者在此事件之前因背压丢弃的事件数。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BusEvent {
    pub seq: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pid: Option<Pid>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dropped: Option<u64>,
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
    /// `net.request`：一次网络请求经过策略判定（`net.log` 数据源）。
    #[serde(rename = "net.request")]
    NetRequest {
        summary: NetRequestSummary,
        allowed: bool,
    },
    /// `cap.request`：出现待审批请求（Console 审批收件箱数据源）。
    #[serde(rename = "cap.request")]
    CapRequest {
        approval_id: String,
        method: String,
        scope: String,
    },
    /// `act.takeover`：人工接管开始/结束（结束含进程终止时的自动清除）。
    #[serde(rename = "act.takeover")]
    Takeover { active: bool, holder: String },
    /// `quota.exceeded`：进程内存越过配额水位（处置动作见 `policy`）。
    #[serde(rename = "quota.exceeded")]
    QuotaExceeded {
        usage_bytes: u64,
        limit_bytes: u64,
        policy: QuotaPolicy,
    },
    /// `wf.run`：工作流运行进展（启动/步骤/重试/结束）。
    #[serde(rename = "wf.run")]
    WfRun {
        wf: String,
        status: WfRunStatus,
        #[serde(skip_serializing_if = "Option::is_none")]
        step: Option<u32>,
        #[serde(skip_serializing_if = "Option::is_none")]
        detail: Option<String>,
    },
}

/// 工作流运行状态（`wf.run` 事件）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WfRunStatus {
    Started,
    StepOk,
    StepRetry,
    Succeeded,
    Failed,
}

impl BusPayload {
    /// 事件主题名（evt.subscribe 过滤用）。
    pub fn topic(&self) -> &'static str {
        match self {
            BusPayload::ProcLifecycle { .. } => "proc.lifecycle",
            BusPayload::Navigated { .. } => "nav",
            BusPayload::ConsoleLog { .. } => "console",
            BusPayload::NetRequest { .. } => "net.request",
            BusPayload::CapRequest { .. } => "cap.request",
            BusPayload::Takeover { .. } => "act.takeover",
            BusPayload::QuotaExceeded { .. } => "quota.exceeded",
            BusPayload::WfRun { .. } => "wf.run",
        }
    }

    /// 高频可丢主题（背压时丢最旧并计数）；关键主题永不丢。
    pub fn droppable(&self) -> bool {
        matches!(
            self,
            BusPayload::Navigated { .. }
                | BusPayload::ConsoleLog { .. }
                | BusPayload::NetRequest { .. }
        )
    }
}

/// 订阅者共享状态：队列 + 丢弃计数 + 唤醒信号。
struct SubShared {
    queue: Mutex<VecDeque<BusEvent>>,
    dropped: AtomicU64,
    notify: Notify,
    closed: AtomicBool,
}

/// 自研事件总线。发布为同步操作；订阅者异步消费。
pub(crate) struct Bus {
    subs: Mutex<Vec<Weak<SubShared>>>,
    /// 高频主题的每订阅者队列容量。
    capacity: usize,
}

impl Bus {
    pub(crate) fn new(capacity: usize) -> Self {
        Self {
            subs: Mutex::new(Vec::new()),
            capacity: capacity.max(1),
        }
    }

    pub(crate) fn subscribe(&self) -> BusReceiver {
        let shared = Arc::new(SubShared {
            queue: Mutex::new(VecDeque::new()),
            dropped: AtomicU64::new(0),
            notify: Notify::new(),
            closed: AtomicBool::new(false),
        });
        self.lock_subs().push(Arc::downgrade(&shared));
        BusReceiver { shared }
    }

    /// 发布事件：每订阅者独立入队；顺带清理已消失的订阅者。
    pub(crate) fn publish(&self, event: BusEvent) {
        let droppable = event.payload.droppable();
        self.lock_subs().retain(|weak| {
            let Some(sub) = weak.upgrade() else {
                return false;
            };
            {
                let mut q = sub.queue.lock().unwrap_or_else(PoisonError::into_inner);
                if droppable {
                    // 背压只淘汰同为可丢主题的最旧事件；关键主题即使排队也不受挤压
                    let droppable_len = q.iter().filter(|e| e.payload.droppable()).count();
                    if droppable_len >= self.capacity
                        && let Some(pos) = q.iter().position(|e| e.payload.droppable())
                    {
                        q.remove(pos);
                        sub.dropped.fetch_add(1, Ordering::SeqCst);
                    }
                }
                q.push_back(event.clone());
            }
            sub.notify.notify_one();
            true
        });
    }

    fn lock_subs(&self) -> std::sync::MutexGuard<'_, Vec<Weak<SubShared>>> {
        self.subs.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

impl Drop for Bus {
    fn drop(&mut self) {
        for weak in self.lock_subs().iter() {
            if let Some(sub) = weak.upgrade() {
                sub.closed.store(true, Ordering::SeqCst);
                sub.notify.notify_one();
            }
        }
    }
}

/// 总线接收端（[`Bus::subscribe`] 返回）。
pub struct BusReceiver {
    shared: Arc<SubShared>,
}

/// 接收错误：总线已关闭（内核销毁）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BusRecvError {
    Closed,
}

impl std::fmt::Display for BusRecvError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("bus closed")
    }
}

impl std::error::Error for BusRecvError {}

impl BusReceiver {
    /// 取下一条事件；若此前有背压丢弃，事件带 `dropped` 计数。
    ///
    /// 队列耗尽且总线关闭时返回 [`BusRecvError::Closed`]。
    pub async fn recv(&mut self) -> Result<BusEvent, BusRecvError> {
        loop {
            {
                let mut q = self
                    .shared
                    .queue
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner);
                if let Some(mut ev) = q.pop_front() {
                    let dropped = self.shared.dropped.swap(0, Ordering::SeqCst);
                    if dropped > 0 {
                        ev.dropped = Some(dropped);
                    }
                    return Ok(ev);
                }
            }
            if self.shared.closed.load(Ordering::SeqCst) {
                return Err(BusRecvError::Closed);
            }
            self.shared.notify.notified().await;
        }
    }
}
