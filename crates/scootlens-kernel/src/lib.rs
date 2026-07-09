//! # scootlens-kernel
//!
//! ScootLens 内核（docs/04-kernel-design.md）。P1 范围：
//!
//! - **Process Manager**：spawn/list/info/kill，状态机 Spawning→Running→Terminated/Crashed
//! - **Scheduler**：全局并发上限 + FIFO 排队（tokio 公平信号量）
//! - **Event Bus**：broadcast + 单调 `seq`，引擎事件与生命周期事件统一入总线
//! - **崩溃监督**：订阅驱动事件流，崩溃 → 标记 `Crashed` + 广播 + 释放槽位
//!
//! 内核只依赖 HAL trait，驱动在二进制层注入（依赖规则见 docs/02-architecture.md）。

mod bus;
mod dispatch;
mod frames;
mod journal;
mod netstack;
mod proc;
mod redact;
mod security;
mod takeover;
mod vfs;
mod wf;

use bus::Bus;
pub use bus::{BusEvent, BusPayload, BusReceiver, BusRecvError, WfRunStatus};
pub use dispatch::Dispatcher;
pub use journal::{Journal, JournalEntry, JournalKind, JournalLine, parse_lines};
pub use netstack::{NetStack, ProcPolicy};
pub use proc::{ProcInfo, ProcState};
pub use redact::{Redactor, SUBSTRING_MIN_LEN};
pub use security::{AuthzGate, Caller, SecurityManager};
pub use vfs::StateVfs;
pub use wf::WfClock;

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use scootlens_abi::{
    AbiError, ErrorCode, Pid, QuotaPolicy, QuotaSpec, REPLAY_FORMAT_VERSION, ReplayBundle,
    ReplayLine, SnapId,
};
use scootlens_hal::{
    A11ySnapshot, ActResult, EngineCaps, EngineDriver, EngineEvent, EngineHandle, EngineMetrics,
    HalResult, HistoryDir, InputAction, NavResult, ProfileSpec, SnapshotOpts, StateBundle,
};
use serde::{Deserialize, Serialize};
use tokio::sync::{Semaphore, broadcast};
use url::Url;

/// 内核配置。
#[derive(Debug, Clone)]
pub struct KernelConfig {
    /// 全局并发进程上限；超出的 spawn 请求 FIFO 排队。
    pub max_procs: usize,
    /// Event Bus 高频主题（nav/console/net.request）的每订阅者队列容量；
    /// 慢订阅者丢最旧事件并计数。关键主题（生命周期/审批/配额/工作流）永不丢。
    pub bus_capacity: usize,
    /// 状态目录（journal / keys / vault / downloads / uploads）。
    /// None = 内存模式（测试）。
    pub state_dir: Option<PathBuf>,
    /// 人工审批的调用内等待上限；超时返回 `E_APPROVAL_PENDING`。
    pub approval_timeout: Duration,
    /// 配额监控轮询间隔。
    pub quota_poll_interval: Duration,
    /// 高配额门槛：`proc.spawn` 申请的内存配额超过此值需 `quota:high` 作用域。
    pub quota_high_bytes: u64,
    /// 人工接管期间，其他主体输入调用的挂起等待上限；超时返回 `E_TIMEOUT`。
    pub takeover_hold_timeout: Duration,
}

impl Default for KernelConfig {
    fn default() -> Self {
        Self {
            max_procs: 8,
            bus_capacity: 1024,
            state_dir: None,
            approval_timeout: Duration::from_secs(60),
            quota_poll_interval: Duration::from_millis(500),
            quota_high_bytes: 2 * 1024 * 1024 * 1024,
            takeover_hold_timeout: Duration::from_secs(30),
        }
    }
}

/// `sys.info` 返回。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SysInfo {
    pub abi_version: String,
    pub kernel_version: String,
    pub engine: String,
    pub caps: EngineCaps,
    pub max_procs: usize,
    pub running_procs: usize,
}

struct ProcEntry {
    state: ProcState,
    engine: &'static str,
    profile: String,
    handle: Option<Arc<dyn EngineHandle>>,
    /// 占用的调度槽位；Terminated/Crashed/Suspended 时释放。
    permit: Option<tokio::sync::OwnedSemaphorePermit>,
    supervisor: Option<tokio::task::JoinHandle<()>>,
    /// 配额监控任务（spawn 带 quotas 时存在）。
    quota_monitor: Option<tokio::task::JoinHandle<()>>,
    /// 当前页面 URL（origin 鉴权依据）。
    current_url: Option<Url>,
}

struct Inner {
    driver: Arc<dyn EngineDriver>,
    config: KernelConfig,
    procs: Mutex<HashMap<Pid, ProcEntry>>,
    slots: Arc<Semaphore>,
    bus: Bus,
    seq: AtomicU64,
    pid_counter: AtomicU64,
    security: SecurityManager,
    journal: Journal,
    vfs: StateVfs,
    netstack: Arc<NetStack>,
    redactor: Redactor,
    takeover: takeover::TakeoverTable,
    frames: frames::FrameStore,
}

impl Inner {
    fn lock_procs(&self) -> std::sync::MutexGuard<'_, HashMap<Pid, ProcEntry>> {
        self.procs.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn emit(&self, pid: Option<Pid>, payload: BusPayload) {
        let seq = self.seq.fetch_add(1, Ordering::SeqCst) + 1;
        self.bus.publish(BusEvent {
            seq,
            pid,
            dropped: None,
            payload,
        });
    }

    fn next_pid(&self) -> Pid {
        let n = self.pid_counter.fetch_add(1, Ordering::SeqCst) + 1;
        let pid = format!("p-{}", to_base36(n));
        pid.parse().expect("generated pid is valid")
    }
}

fn to_base36(mut n: u64) -> String {
    const DIGITS: &[u8] = b"0123456789abcdefghijklmnopqrstuvwxyz";
    let mut buf = Vec::new();
    loop {
        buf.push(DIGITS[(n % 36) as usize]);
        n /= 36;
        if n == 0 {
            break;
        }
    }
    buf.reverse();
    String::from_utf8(buf).expect("base36 is ascii")
}

/// ScootLens 内核。廉价 Clone（内部 Arc 共享）。
#[derive(Clone)]
pub struct Kernel {
    inner: Arc<Inner>,
}

impl Kernel {
    /// 内存模式构造（state_dir 必须为 None；测试与嵌入场景）。
    pub fn new(driver: Arc<dyn EngineDriver>, config: KernelConfig) -> Self {
        assert!(
            config.state_dir.is_none(),
            "use Kernel::open for a state dir"
        );
        Self::open(driver, config).expect("memory-mode kernel init cannot fail")
    }

    /// 完整构造：初始化安全/journal/VFS/网络栈（state_dir 可选）。
    pub fn open(driver: Arc<dyn EngineDriver>, config: KernelConfig) -> std::io::Result<Self> {
        let state_dir = config.state_dir.clone();
        let security = SecurityManager::load_or_generate(state_dir.as_deref())?;
        let journal = match &state_dir {
            Some(dir) => Journal::open(dir)?,
            None => Journal::in_memory(),
        };
        let vfs = StateVfs::open(state_dir.as_deref())?;
        let bus = Bus::new(config.bus_capacity);
        let slots = Arc::new(Semaphore::new(config.max_procs));
        Ok(Self {
            inner: Arc::new(Inner {
                driver,
                config,
                procs: Mutex::new(HashMap::new()),
                slots,
                bus,
                seq: AtomicU64::new(0),
                pid_counter: AtomicU64::new(0),
                security,
                journal,
                vfs,
                netstack: Arc::new(NetStack::default()),
                redactor: Redactor::default(),
                takeover: takeover::TakeoverTable::default(),
                frames: frames::FrameStore::default(),
            }),
        })
    }

    // ---------- P2 子系统访问 ----------

    pub fn security(&self) -> &SecurityManager {
        &self.inner.security
    }

    pub fn journal(&self) -> &Journal {
        &self.inner.journal
    }

    pub fn vfs(&self) -> &StateVfs {
        &self.inner.vfs
    }

    pub fn netstack(&self) -> &NetStack {
        &self.inner.netstack
    }

    pub fn redactor(&self) -> &Redactor {
        &self.inner.redactor
    }

    /// 审批等待上限。
    pub fn approval_timeout(&self) -> Duration {
        self.inner.config.approval_timeout
    }

    /// 发布事件到总线（dispatch 层发 `cap.request` 用）。
    pub(crate) fn emit(&self, pid: Option<Pid>, payload: BusPayload) {
        self.inner.emit(pid, payload);
    }

    /// 当前页面 origin（`host[:port]`，仅显式端口带端口）。
    pub fn current_origin(&self, pid: &Pid) -> Option<String> {
        let procs = self.inner.lock_procs();
        procs
            .get(pid)
            .and_then(|e| e.current_url.as_ref())
            .and_then(origin_of)
    }

    // ---------- Process Manager ----------

    /// 启动进程。并发上限已满时排队（FIFO），直到有槽位释放。
    pub async fn spawn(&self, profile: ProfileSpec) -> HalResult<Pid> {
        self.spawn_with(profile, None).await
    }

    /// 启动进程并附加资源配额（配额监控见 docs/04-kernel-design.md §4.2）。
    pub async fn spawn_with(
        &self,
        mut profile: ProfileSpec,
        quotas: Option<QuotaSpec>,
    ) -> HalResult<Pid> {
        let permit = Arc::clone(&self.inner.slots)
            .acquire_owned()
            .await
            .map_err(|_| AbiError::new(ErrorCode::Internal, "scheduler closed"))?;

        if profile.download_dir.is_none() {
            profile.download_dir = self.inner.vfs.downloads_dir();
        }
        let handle: Arc<dyn EngineHandle> = Arc::from(self.inner.driver.spawn(&profile).await?);
        let pid = self.inner.next_pid();

        // 网络强制：驱动支持 net_rules 时装 per-proc 策略（默认规则全放行）
        if self.inner.driver.capabilities().net_rules {
            let policy = Arc::new(ProcPolicy::new(
                Arc::clone(&self.inner.netstack),
                pid.clone(),
            ));
            handle.set_request_policy(Some(policy)).await?;
        }

        // profile 复用：存在已导入的 profile 状态且引擎支持 state → 预加载
        if self.inner.driver.capabilities().state
            && let Some(text) = self.inner.vfs.profile_read(&profile.name)
        {
            match serde_json::from_str::<StateBundle>(&text) {
                Ok(bundle) => handle.import_state(&bundle).await?,
                Err(e) => {
                    tracing::warn!(profile = %profile.name, %e, "profile state unreadable; skipped");
                }
            }
        }

        let supervisor = tokio::spawn(supervise(
            Arc::clone(&self.inner),
            pid.clone(),
            handle.events(),
        ));
        let quota_monitor = quotas.map(|q| {
            tokio::spawn(monitor_quota(
                Arc::clone(&self.inner),
                pid.clone(),
                Arc::clone(&handle),
                q,
            ))
        });

        self.inner.lock_procs().insert(
            pid.clone(),
            ProcEntry {
                state: ProcState::Running,
                engine: self.inner.driver.id(),
                profile: profile.name,
                handle: Some(handle),
                permit: Some(permit),
                supervisor: Some(supervisor),
                quota_monitor,
                current_url: None,
            },
        );
        self.inner.emit(
            Some(pid.clone()),
            BusPayload::ProcLifecycle {
                state: ProcState::Running,
            },
        );
        tracing::info!(%pid, "proc spawned");
        Ok(pid)
    }

    /// 进程列表（按 pid 排序，输出稳定）。
    pub async fn list(&self) -> Vec<ProcInfo> {
        let procs = self.inner.lock_procs();
        let mut out: Vec<ProcInfo> = procs
            .iter()
            .map(|(pid, e)| ProcInfo {
                pid: pid.clone(),
                state: e.state,
                engine: e.engine.to_owned(),
                profile: e.profile.clone(),
                url: e.current_url.as_ref().map(ToString::to_string),
            })
            .collect();
        out.sort_by(|a, b| a.pid.as_str().cmp(b.pid.as_str()));
        out
    }

    pub async fn info(&self, pid: &Pid) -> HalResult<ProcInfo> {
        let procs = self.inner.lock_procs();
        let e = procs.get(pid).ok_or_else(|| not_found(pid))?;
        Ok(ProcInfo {
            pid: pid.clone(),
            state: e.state,
            engine: e.engine.to_owned(),
            profile: e.profile.clone(),
            url: e.current_url.as_ref().map(ToString::to_string),
        })
    }

    /// 挂起进程：释放调度槽 +（引擎支持时）冻结页面；挂起期间拒绝引擎操作。
    ///
    /// 仅 Running 可挂起；其余状态返回 `E_INVALID_ARG`。
    pub async fn suspend(&self, pid: &Pid) -> HalResult<()> {
        let (handle, permit) = {
            let mut procs = self.inner.lock_procs();
            let e = procs.get_mut(pid).ok_or_else(|| not_found(pid))?;
            match e.state {
                ProcState::Running => {
                    e.state = ProcState::Suspended;
                    (e.handle.clone(), e.permit.take())
                }
                other => {
                    return Err(AbiError::new(
                        ErrorCode::InvalidArg,
                        format!("cannot suspend proc in state {other:?}"),
                    ));
                }
            }
        };
        drop(permit); // 让出调度槽：挂起进程不占并发预算
        if self.inner.driver.capabilities().lifecycle
            && let Some(h) = handle
        {
            // 冻结失败不回滚：调度语义已生效，冻结只是尽力省资源
            if let Err(err) = h.set_lifecycle(true).await {
                tracing::warn!(%pid, %err, "engine freeze failed (suspend continues)");
            }
        }
        self.inner.emit(
            Some(pid.clone()),
            BusPayload::ProcLifecycle {
                state: ProcState::Suspended,
            },
        );
        tracing::info!(%pid, "proc suspended");
        Ok(())
    }

    /// 恢复挂起的进程：重新排队取调度槽（FIFO），成功后解冻并回到 Running。
    pub async fn resume(&self, pid: &Pid) -> HalResult<()> {
        {
            let procs = self.inner.lock_procs();
            let e = procs.get(pid).ok_or_else(|| not_found(pid))?;
            if e.state != ProcState::Suspended {
                return Err(AbiError::new(
                    ErrorCode::InvalidArg,
                    format!("cannot resume proc in state {:?}", e.state),
                ));
            }
        }
        let permit = Arc::clone(&self.inner.slots)
            .acquire_owned()
            .await
            .map_err(|_| AbiError::new(ErrorCode::Internal, "scheduler closed"))?;
        let handle = {
            let mut procs = self.inner.lock_procs();
            let e = procs.get_mut(pid).ok_or_else(|| not_found(pid))?;
            if e.state != ProcState::Suspended {
                // 排队等槽期间被 kill 或崩溃
                return Err(AbiError::new(
                    ErrorCode::InvalidArg,
                    format!("cannot resume proc in state {:?}", e.state),
                ));
            }
            e.state = ProcState::Running;
            e.permit = Some(permit);
            e.handle.clone()
        };
        if self.inner.driver.capabilities().lifecycle
            && let Some(h) = handle
            && let Err(err) = h.set_lifecycle(false).await
        {
            tracing::warn!(%pid, %err, "engine unfreeze failed (resume continues)");
        }
        self.inner.emit(
            Some(pid.clone()),
            BusPayload::ProcLifecycle {
                state: ProcState::Running,
            },
        );
        tracing::info!(%pid, "proc resumed");
        Ok(())
    }

    /// 终止进程：关闭引擎、释放槽位、状态 → Terminated。
    ///
    /// 对已 Terminated 的进程返回 `E_INVALID_ARG`。
    pub async fn kill(&self, pid: &Pid) -> HalResult<()> {
        let (handle, permit, supervisor, quota_monitor) = {
            let mut procs = self.inner.lock_procs();
            let e = procs.get_mut(pid).ok_or_else(|| not_found(pid))?;
            match e.state {
                ProcState::Terminated => {
                    return Err(AbiError::new(
                        ErrorCode::InvalidArg,
                        format!("proc {pid} already terminated"),
                    ));
                }
                ProcState::Spawning
                | ProcState::Running
                | ProcState::Suspended
                | ProcState::Crashed => {
                    e.state = ProcState::Terminated;
                    (
                        e.handle.take(),
                        e.permit.take(),
                        e.supervisor.take(),
                        e.quota_monitor.take(),
                    )
                }
            }
        };
        if let Some(s) = supervisor {
            s.abort();
        }
        if let Some(q) = quota_monitor {
            q.abort();
        }
        if let Some(h) = handle {
            // 尽力关闭；引擎可能已崩溃
            let _ = h.shutdown().await;
        }
        drop(permit);
        self.inner.netstack.drop_proc(pid);
        // 接管随进程一起清除：唤醒被挂起的输入调用（随后按进程状态失败）
        if let Some(holder) = self.inner.takeover.clear(pid) {
            self.inner.emit(
                Some(pid.clone()),
                BusPayload::Takeover {
                    active: false,
                    holder,
                },
            );
        }
        self.inner.emit(
            Some(pid.clone()),
            BusPayload::ProcLifecycle {
                state: ProcState::Terminated,
            },
        );
        tracing::info!(%pid, "proc terminated");
        Ok(())
    }

    // ---------- 人工接管（P4，docs/07-web-console.md） ----------

    /// 开始接管：`subject` 独占该 proc 的输入。仅 Running 可接管。
    pub fn takeover_start(&self, pid: &Pid, subject: &str) -> HalResult<()> {
        {
            let procs = self.inner.lock_procs();
            let e = procs.get(pid).ok_or_else(|| not_found(pid))?;
            if e.state != ProcState::Running {
                return Err(AbiError::new(
                    ErrorCode::InvalidArg,
                    format!("cannot take over proc in state {:?}", e.state),
                ));
            }
        }
        if self.inner.takeover.start(pid, subject)? {
            self.inner.emit(
                Some(pid.clone()),
                BusPayload::Takeover {
                    active: true,
                    holder: subject.to_owned(),
                },
            );
            tracing::info!(%pid, %subject, "takeover started");
        }
        Ok(())
    }

    /// 归还控制：仅 holder 本人；唤醒挂起中的输入调用。
    pub fn takeover_end(&self, pid: &Pid, subject: &str) -> HalResult<()> {
        self.inner.takeover.end(pid, subject)?;
        self.inner.emit(
            Some(pid.clone()),
            BusPayload::Takeover {
                active: false,
                holder: subject.to_owned(),
            },
        );
        tracing::info!(%pid, %subject, "takeover ended");
        Ok(())
    }

    /// 当前接管 holder（无接管 = None）。
    pub fn takeover_holder(&self, pid: &Pid) -> Option<String> {
        self.inner.takeover.holder(pid)
    }

    /// 输入门：接管期间非 holder 的输入调用挂起等待（超时 `E_TIMEOUT`）。
    pub async fn takeover_gate(&self, pid: &Pid, subject: &str) -> HalResult<()> {
        self.inner
            .takeover
            .gate(pid, subject, self.inner.config.takeover_hold_timeout)
            .await
    }

    // ---------- 回放导出（P4，docs/03-abi-spec.md obs.replay.export） ----------

    /// 导出回放包：journal 哈希链尾段（未过滤，可离线验链）+ 该 proc 的画面帧。
    ///
    /// 进程终止后仍可导出（事后取证）；未知 pid 返回 `E_PROC_NOT_FOUND`。
    pub fn replay_export(&self, pid: &Pid, journal_limit: usize) -> HalResult<ReplayBundle> {
        if !self.inner.lock_procs().contains_key(pid) {
            return Err(not_found(pid));
        }
        let journal = self
            .inner
            .journal
            .lines_tail(journal_limit.clamp(1, 4096))
            .into_iter()
            .map(|l| ReplayLine {
                seq: l.seq,
                prev: l.prev,
                hash: l.hash,
                raw: l.raw,
            })
            .collect();
        Ok(ReplayBundle {
            format_version: REPLAY_FORMAT_VERSION,
            pid: pid.clone(),
            engine: self.inner.driver.id().to_owned(),
            exported_at_ms: crate::security::unix_now_ms(),
            journal,
            frames: self.inner.frames.export(pid),
        })
    }

    // ---------- 引擎操作转发 ----------

    fn handle_of(&self, pid: &Pid) -> HalResult<Arc<dyn EngineHandle>> {
        let procs = self.inner.lock_procs();
        let e = procs.get(pid).ok_or_else(|| not_found(pid))?;
        match (&e.state, &e.handle) {
            (ProcState::Running, Some(h)) => Ok(Arc::clone(h)),
            (ProcState::Suspended, _) => Err(AbiError::new(
                ErrorCode::InvalidArg,
                format!("proc {pid} is suspended; resume it first"),
            )),
            (ProcState::Crashed, _) => Err(AbiError::new(
                ErrorCode::EngineCrash,
                format!("proc {pid} engine crashed"),
            )),
            _ => Err(not_found(pid)),
        }
    }

    pub async fn navigate(&self, pid: &Pid, url: &Url) -> HalResult<NavResult> {
        let nav = self.handle_of(pid)?.navigate(url).await?;
        self.set_current_url(pid, nav.url.clone());
        Ok(nav)
    }

    pub async fn page_info(&self, pid: &Pid) -> HalResult<NavResult> {
        self.handle_of(pid)?.page_info().await
    }

    pub async fn history(&self, pid: &Pid, dir: HistoryDir) -> HalResult<NavResult> {
        let nav = self.handle_of(pid)?.history(dir).await?;
        self.set_current_url(pid, nav.url.clone());
        Ok(nav)
    }

    pub async fn reload(&self, pid: &Pid) -> HalResult<NavResult> {
        let nav = self.handle_of(pid)?.reload().await?;
        self.set_current_url(pid, nav.url.clone());
        Ok(nav)
    }

    fn set_current_url(&self, pid: &Pid, url: Url) {
        if let Some(e) = self.inner.lock_procs().get_mut(pid) {
            e.current_url = Some(url);
        }
    }

    pub async fn snapshot(&self, pid: &Pid, opts: &SnapshotOpts) -> HalResult<A11ySnapshot> {
        self.handle_of(pid)?.snapshot(opts).await
    }

    pub async fn screenshot(&self, pid: &Pid) -> HalResult<Vec<u8>> {
        let bytes = self.handle_of(pid)?.screenshot().await?;
        // 采集回放帧（obs.replay.export 数据源）
        self.inner
            .frames
            .record(pid, crate::security::unix_now_ms(), bytes.clone());
        Ok(bytes)
    }

    pub async fn dispatch(&self, pid: &Pid, action: &InputAction) -> HalResult<ActResult> {
        self.handle_of(pid)?.dispatch(action).await
    }

    pub async fn eval(
        &self,
        pid: &Pid,
        script: &str,
        args: &[serde_json::Value],
    ) -> HalResult<serde_json::Value> {
        self.handle_of(pid)?.eval(script, args).await
    }

    pub async fn metrics(&self, pid: &Pid) -> HalResult<EngineMetrics> {
        self.handle_of(pid)?.metrics().await
    }

    pub async fn export_state(&self, pid: &Pid) -> HalResult<scootlens_hal::StateBundle> {
        self.handle_of(pid)?.export_state().await
    }

    pub async fn import_state(
        &self,
        pid: &Pid,
        bundle: &scootlens_hal::StateBundle,
    ) -> HalResult<()> {
        self.handle_of(pid)?.import_state(bundle).await
    }

    // ---------- 快照 / 恢复 / profile 复用 ----------

    /// 进程快照：导出会话状态 + 当前 URL，内容寻址落盘，返回 `snap-<hash>`。
    ///
    /// Running 与 Suspended 均可快照（挂起后落盘是常规 OS 语义）。
    pub async fn snapshot_proc(&self, pid: &Pid) -> HalResult<SnapId> {
        let (handle, profile, url) = {
            let procs = self.inner.lock_procs();
            let e = procs.get(pid).ok_or_else(|| not_found(pid))?;
            match (&e.state, &e.handle) {
                (ProcState::Running | ProcState::Suspended, Some(h)) => {
                    (Arc::clone(h), e.profile.clone(), e.current_url.clone())
                }
                (ProcState::Crashed, _) => {
                    return Err(AbiError::new(
                        ErrorCode::EngineCrash,
                        format!("proc {pid} engine crashed"),
                    ));
                }
                _ => return Err(not_found(pid)),
            }
        };
        let state = handle.export_state().await?;
        let doc = SnapshotDoc {
            engine: self.inner.driver.id().to_owned(),
            profile,
            url: url.map(|u| u.to_string()),
            state,
        };
        let text = serde_json::to_string_pretty(&doc)
            .map_err(|e| AbiError::new(ErrorCode::Internal, format!("snapshot encode: {e}")))?;
        let hash = self.inner.vfs.snapshot_write(&text)?;
        let snap: SnapId = format!("snap-{hash}")
            .parse()
            .map_err(|e| AbiError::new(ErrorCode::Internal, format!("snap id: {e}")))?;
        tracing::info!(%pid, %snap, "proc snapshot stored");
        Ok(snap)
    }

    /// 从快照恢复为新进程：spawn（同 profile）→ 导航回原 URL → 导入状态。
    ///
    /// `engine` 提供时须与当前驱动一致；快照的记录引擎不符也拒绝。
    pub async fn restore_proc(&self, snap: &SnapId, engine: Option<&str>) -> HalResult<Pid> {
        let text = self.inner.vfs.snapshot_read(snap.suffix())?;
        let doc: SnapshotDoc = serde_json::from_str(&text)
            .map_err(|e| AbiError::new(ErrorCode::Internal, format!("snapshot decode: {e}")))?;
        let current = self.inner.driver.id();
        let want = engine.unwrap_or(&doc.engine);
        if want != current || doc.engine != current {
            return Err(AbiError::new(
                ErrorCode::InvalidArg,
                format!(
                    "snapshot engine {:?} (requested {want:?}) does not match kernel engine {current:?}",
                    doc.engine
                ),
            ));
        }
        let pid = self
            .spawn(ProfileSpec {
                name: doc.profile,
                download_dir: None,
            })
            .await?;
        // 先导航后导入：storage 类状态依赖页面 origin 已就位
        if let Some(url_text) = &doc.url
            && let Ok(url) = Url::parse(url_text)
        {
            self.navigate(&pid, &url).await?;
        }
        self.handle_of(&pid)?.import_state(&doc.state).await?;
        tracing::info!(%snap, %pid, "proc restored from snapshot");
        Ok(pid)
    }

    /// `state.import`：把状态包并入 profile 存储；后续以该 profile spawn 时预加载。
    pub fn import_profile_state(
        &self,
        profile: &str,
        bundle: &scootlens_hal::StateBundle,
    ) -> HalResult<()> {
        let mut merged: StateBundle = self
            .inner
            .vfs
            .profile_read(profile)
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or_default();
        merged.entries.extend(bundle.entries.clone());
        let text = serde_json::to_string_pretty(&merged)
            .map_err(|e| AbiError::new(ErrorCode::Internal, format!("profile encode: {e}")))?;
        self.inner.vfs.profile_write(profile, &text)?;
        tracing::info!(%profile, entries = merged.entries.len(), "profile state imported");
        Ok(())
    }

    /// 已导入的 profile 名列表（`state.list` namespace=profiles）。
    pub fn list_profile_names(&self) -> HalResult<Vec<String>> {
        self.inner.vfs.profile_names()
    }

    /// 读取 profile 的已导入状态包；不存在 → `Ok(None)`，损坏 → `E_INTERNAL`。
    ///
    /// 仅供内核内部与摘要展示使用——dispatch 层只把 entry 元数据（键名/域/标志/
    /// 字节数）暴露出去，值本身绝不进入任何 syscall 返回值。
    pub fn read_profile_state(&self, profile: &str) -> HalResult<Option<StateBundle>> {
        match self.inner.vfs.profile_read(profile) {
            None => Ok(None),
            Some(text) => serde_json::from_str(&text)
                .map(Some)
                .map_err(|e| AbiError::new(ErrorCode::Internal, format!("profile decode: {e}"))),
        }
    }

    /// `state.delete`（ADR-0011）：删除整个 profile，或仅其中一条 entry。
    ///
    /// 返回删除的 entry 数。profile / entry 不存在 → `E_INVALID_ARG`。
    /// 只作用于 profile 存储，不触碰任何运行中的进程。
    pub fn delete_profile_state(&self, profile: &str, entry: Option<&str>) -> HalResult<u64> {
        match entry {
            None => {
                let removed = self
                    .read_profile_state(profile)?
                    .map(|b| b.entries.len() as u64)
                    .ok_or_else(|| no_such_profile(profile))?;
                self.inner.vfs.profile_delete(profile)?;
                tracing::info!(%profile, entries = removed, "profile state deleted");
                Ok(removed)
            }
            Some(key) => {
                let mut bundle = self
                    .read_profile_state(profile)?
                    .ok_or_else(|| no_such_profile(profile))?;
                if bundle.entries.remove(key).is_none() {
                    return Err(AbiError::new(
                        ErrorCode::InvalidArg,
                        format!("no such entry {key:?} in profile {profile:?}"),
                    ));
                }
                let text = serde_json::to_string_pretty(&bundle).map_err(|e| {
                    AbiError::new(ErrorCode::Internal, format!("profile encode: {e}"))
                })?;
                self.inner.vfs.profile_write(profile, &text)?;
                tracing::info!(%profile, entry = %key, "profile entry deleted");
                Ok(1)
            }
        }
    }

    /// 高配额门槛（dispatch 鉴权 `quota:high` 用）。
    pub fn quota_high_bytes(&self) -> u64 {
        self.inner.config.quota_high_bytes
    }

    // ---------- Event Bus / sys ----------

    /// 订阅内核事件总线。
    pub fn subscribe(&self) -> BusReceiver {
        self.inner.bus.subscribe()
    }

    pub async fn sys_info(&self) -> SysInfo {
        let running = self
            .inner
            .lock_procs()
            .values()
            .filter(|e| e.state == ProcState::Running)
            .count();
        SysInfo {
            abi_version: scootlens_abi::ABI_VERSION.to_owned(),
            kernel_version: env!("CARGO_PKG_VERSION").to_owned(),
            engine: self.inner.driver.id().to_owned(),
            caps: self.inner.driver.capabilities(),
            max_procs: self.inner.config.max_procs,
            running_procs: running,
        }
    }
}

fn not_found(pid: &Pid) -> AbiError {
    AbiError::new(ErrorCode::ProcNotFound, format!("proc {pid} not found"))
}

fn no_such_profile(profile: &str) -> AbiError {
    AbiError::new(ErrorCode::InvalidArg, format!("no such profile: {profile}"))
}

/// 快照文件内容（`state_dir/snapshots/<hash>.json`）。
#[derive(Debug, Serialize, Deserialize)]
struct SnapshotDoc {
    engine: String,
    profile: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    url: Option<String>,
    state: StateBundle,
}

/// URL → 规范化 origin（`host` 或 `host:port`，仅显式端口带端口）。
pub fn origin_of(url: &Url) -> Option<String> {
    let host = url.host_str()?;
    Some(match url.port() {
        Some(p) => format!("{host}:{p}"),
        None => host.to_owned(),
    })
}

/// 崩溃监督 + 引擎事件转发：驱动事件流 → 内核总线。
async fn supervise(inner: Arc<Inner>, pid: Pid, mut events: broadcast::Receiver<EngineEvent>) {
    loop {
        match events.recv().await {
            Ok(EngineEvent::Crashed) => {
                let permit = {
                    let mut procs = inner.lock_procs();
                    match procs.get_mut(&pid) {
                        Some(e) if matches!(e.state, ProcState::Running | ProcState::Suspended) => {
                            e.state = ProcState::Crashed;
                            e.handle.take();
                            e.permit.take()
                        }
                        _ => None,
                    }
                };
                drop(permit);
                inner.emit(
                    Some(pid.clone()),
                    BusPayload::ProcLifecycle {
                        state: ProcState::Crashed,
                    },
                );
                tracing::warn!(%pid, "engine crashed");
                break;
            }
            Ok(EngineEvent::Navigated { url }) => {
                if let Some(e) = inner.lock_procs().get_mut(&pid) {
                    e.current_url = Some(url.clone());
                }
                inner.emit(Some(pid.clone()), BusPayload::Navigated { url });
            }
            Ok(EngineEvent::ConsoleLog { text }) => {
                inner.emit(Some(pid.clone()), BusPayload::ConsoleLog { text });
            }
            Ok(EngineEvent::NetRequest { summary, allowed }) => {
                inner.emit(
                    Some(pid.clone()),
                    BusPayload::NetRequest { summary, allowed },
                );
            }
            Err(broadcast::error::RecvError::Lagged(n)) => {
                tracing::warn!(%pid, dropped = n, "engine event stream lagged");
            }
            Err(broadcast::error::RecvError::Closed) => break,
        }
    }
}

/// 配额监控：轮询引擎内存指标，越过水位 → 事件 + journal + 按策略处置。
///
/// 越限只在"跨越水位"时触发一次（回落后可再次触发），避免风暴。
async fn monitor_quota(
    inner: Arc<Inner>,
    pid: Pid,
    handle: Arc<dyn EngineHandle>,
    quota: QuotaSpec,
) {
    let mut over = false;
    loop {
        tokio::time::sleep(inner.config.quota_poll_interval).await;
        // 进程离场（终止/崩溃/句柄被替换）即退出
        let state = match inner.lock_procs().get(&pid) {
            Some(e) => e.state,
            None => break,
        };
        if matches!(state, ProcState::Terminated | ProcState::Crashed) {
            break;
        }
        let usage = match handle.metrics().await {
            Ok(m) => m.memory_bytes,
            Err(_) => continue, // 瞬时失败（如挂起中的引擎）不触发处置
        };
        let exceeded = usage > quota.max_memory_bytes;
        if exceeded && !over {
            inner.journal.record(
                JournalKind::Deny,
                "kernel:quota",
                "quota.exceeded",
                Some(pid.as_str()),
                serde_json::json!({
                    "usage_bytes": usage,
                    "limit_bytes": quota.max_memory_bytes,
                    "policy": quota.on_exceed,
                }),
            );
            inner.emit(
                Some(pid.clone()),
                BusPayload::QuotaExceeded {
                    usage_bytes: usage,
                    limit_bytes: quota.max_memory_bytes,
                    policy: quota.on_exceed,
                },
            );
            tracing::warn!(%pid, usage, limit = quota.max_memory_bytes, "memory quota exceeded");
            match quota.on_exceed {
                QuotaPolicy::Warn => {}
                QuotaPolicy::Suspend => {
                    let kernel = Kernel {
                        inner: Arc::clone(&inner),
                    };
                    if let Err(err) = kernel.suspend(&pid).await {
                        tracing::warn!(%pid, %err, "quota suspend failed");
                    }
                }
                QuotaPolicy::Kill => {
                    let kernel = Kernel {
                        inner: Arc::clone(&inner),
                    };
                    if let Err(err) = kernel.kill(&pid).await {
                        tracing::warn!(%pid, %err, "quota kill failed");
                    }
                    break;
                }
            }
        }
        over = exceeded;
    }
}
