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
mod proc;

pub use bus::{BusEvent, BusPayload};
pub use dispatch::Dispatcher;
pub use proc::{ProcInfo, ProcState};

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

use scootlens_abi::{AbiError, ErrorCode, Pid};
use scootlens_hal::{
    A11ySnapshot, ActResult, EngineCaps, EngineDriver, EngineEvent, EngineHandle, EngineMetrics,
    HalResult, HistoryDir, InputAction, NavResult, ProfileSpec, SnapshotOpts,
};
use serde::{Deserialize, Serialize};
use tokio::sync::{Semaphore, broadcast};
use url::Url;

/// 内核配置。
#[derive(Debug, Clone)]
pub struct KernelConfig {
    /// 全局并发进程上限；超出的 spawn 请求 FIFO 排队。
    pub max_procs: usize,
    /// Event Bus 缓冲容量（慢订阅者会丢事件并收到 Lagged）。
    pub bus_capacity: usize,
}

impl Default for KernelConfig {
    fn default() -> Self {
        Self {
            max_procs: 8,
            bus_capacity: 1024,
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
    /// 占用的调度槽位；Terminated/Crashed 时释放。
    permit: Option<tokio::sync::OwnedSemaphorePermit>,
    supervisor: Option<tokio::task::JoinHandle<()>>,
}

struct Inner {
    driver: Arc<dyn EngineDriver>,
    config: KernelConfig,
    procs: Mutex<HashMap<Pid, ProcEntry>>,
    slots: Arc<Semaphore>,
    bus: broadcast::Sender<BusEvent>,
    seq: AtomicU64,
    pid_counter: AtomicU64,
}

impl Inner {
    fn lock_procs(&self) -> std::sync::MutexGuard<'_, HashMap<Pid, ProcEntry>> {
        self.procs.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn emit(&self, pid: Option<Pid>, payload: BusPayload) {
        let seq = self.seq.fetch_add(1, Ordering::SeqCst) + 1;
        let _ = self.bus.send(BusEvent { seq, pid, payload });
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
    pub fn new(driver: Arc<dyn EngineDriver>, config: KernelConfig) -> Self {
        let (bus, _) = broadcast::channel(config.bus_capacity);
        let slots = Arc::new(Semaphore::new(config.max_procs));
        Self {
            inner: Arc::new(Inner {
                driver,
                config,
                procs: Mutex::new(HashMap::new()),
                slots,
                bus,
                seq: AtomicU64::new(0),
                pid_counter: AtomicU64::new(0),
            }),
        }
    }

    // ---------- Process Manager ----------

    /// 启动进程。并发上限已满时排队（FIFO），直到有槽位释放。
    pub async fn spawn(&self, profile: ProfileSpec) -> HalResult<Pid> {
        let permit = Arc::clone(&self.inner.slots)
            .acquire_owned()
            .await
            .map_err(|_| AbiError::new(ErrorCode::Internal, "scheduler closed"))?;

        let handle: Arc<dyn EngineHandle> =
            Arc::from(self.inner.driver.spawn(&profile).await?);
        let pid = self.inner.next_pid();

        let supervisor = tokio::spawn(supervise(
            Arc::clone(&self.inner),
            pid.clone(),
            handle.events(),
        ));

        self.inner.lock_procs().insert(
            pid.clone(),
            ProcEntry {
                state: ProcState::Running,
                engine: self.inner.driver.id(),
                profile: profile.name,
                handle: Some(handle),
                permit: Some(permit),
                supervisor: Some(supervisor),
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
        })
    }

    /// 终止进程：关闭引擎、释放槽位、状态 → Terminated。
    ///
    /// 对已 Terminated 的进程返回 `E_INVALID_ARG`。
    pub async fn kill(&self, pid: &Pid) -> HalResult<()> {
        let (handle, permit, supervisor) = {
            let mut procs = self.inner.lock_procs();
            let e = procs.get_mut(pid).ok_or_else(|| not_found(pid))?;
            match e.state {
                ProcState::Terminated => {
                    return Err(AbiError::new(
                        ErrorCode::InvalidArg,
                        format!("proc {pid} already terminated"),
                    ));
                }
                ProcState::Spawning | ProcState::Running | ProcState::Crashed => {
                    e.state = ProcState::Terminated;
                    (e.handle.take(), e.permit.take(), e.supervisor.take())
                }
            }
        };
        if let Some(s) = supervisor {
            s.abort();
        }
        if let Some(h) = handle {
            // 尽力关闭；引擎可能已崩溃
            let _ = h.shutdown().await;
        }
        drop(permit);
        self.inner.emit(
            Some(pid.clone()),
            BusPayload::ProcLifecycle {
                state: ProcState::Terminated,
            },
        );
        tracing::info!(%pid, "proc terminated");
        Ok(())
    }

    // ---------- 引擎操作转发 ----------

    fn handle_of(&self, pid: &Pid) -> HalResult<Arc<dyn EngineHandle>> {
        let procs = self.inner.lock_procs();
        let e = procs.get(pid).ok_or_else(|| not_found(pid))?;
        match (&e.state, &e.handle) {
            (ProcState::Running, Some(h)) => Ok(Arc::clone(h)),
            (ProcState::Crashed, _) => Err(AbiError::new(
                ErrorCode::EngineCrash,
                format!("proc {pid} engine crashed"),
            )),
            _ => Err(not_found(pid)),
        }
    }

    pub async fn navigate(&self, pid: &Pid, url: &Url) -> HalResult<NavResult> {
        self.handle_of(pid)?.navigate(url).await
    }

    pub async fn page_info(&self, pid: &Pid) -> HalResult<NavResult> {
        self.handle_of(pid)?.page_info().await
    }

    pub async fn history(&self, pid: &Pid, dir: HistoryDir) -> HalResult<NavResult> {
        self.handle_of(pid)?.history(dir).await
    }

    pub async fn reload(&self, pid: &Pid) -> HalResult<NavResult> {
        self.handle_of(pid)?.reload().await
    }

    pub async fn snapshot(&self, pid: &Pid, opts: &SnapshotOpts) -> HalResult<A11ySnapshot> {
        self.handle_of(pid)?.snapshot(opts).await
    }

    pub async fn screenshot(&self, pid: &Pid) -> HalResult<Vec<u8>> {
        self.handle_of(pid)?.screenshot().await
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

    // ---------- Event Bus / sys ----------

    /// 订阅内核事件总线。
    pub fn subscribe(&self) -> broadcast::Receiver<BusEvent> {
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

/// 崩溃监督 + 引擎事件转发：驱动事件流 → 内核总线。
async fn supervise(
    inner: Arc<Inner>,
    pid: Pid,
    mut events: broadcast::Receiver<EngineEvent>,
) {
    loop {
        match events.recv().await {
            Ok(EngineEvent::Crashed) => {
                let permit = {
                    let mut procs = inner.lock_procs();
                    match procs.get_mut(&pid) {
                        Some(e) if e.state == ProcState::Running => {
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
                inner.emit(Some(pid.clone()), BusPayload::Navigated { url });
            }
            Ok(EngineEvent::ConsoleLog { text }) => {
                inner.emit(Some(pid.clone()), BusPayload::ConsoleLog { text });
            }
            Err(broadcast::error::RecvError::Lagged(n)) => {
                tracing::warn!(%pid, dropped = n, "engine event stream lagged");
            }
            Err(broadcast::error::RecvError::Closed) => break,
        }
    }
}
