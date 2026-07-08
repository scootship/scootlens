//! # scootlens-driver-mock
//!
//! 可编程内存假引擎（ADR-0006）：确定性、毫秒级、支持故障注入。
//! 内核与 gateway 的单元/集成测试全部跑在它上面；真实引擎只出现在 e2e。
//!
//! ```
//! use scootlens_driver_mock::MockDriver;
//! let driver = MockDriver::standard_fixture();
//! ```

mod model;

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use scootlens_abi::{AbiError, ElementRef, ErrorCode, NetRequestSummary};
use scootlens_hal::{
    A11ySnapshot, ActResult, EngineCaps, EngineDriver, EngineEvent, EngineHandle, EngineMetrics,
    HalResult, HistoryDir, InputAction, NavResult, ProfileSpec, RequestPolicy, SnapshotOpts,
    StateBundle,
};
use serde_json::Value;
use tokio::sync::broadcast;
use url::Url;

pub use model::{NodeModel, PageModel, SiteBuilder, SiteModel};

/// 标准 fixture 站点基地址（conformance 语义见 scootlens-hal）。
pub fn fixture_base() -> Url {
    Url::parse("http://fixture.test/").expect("static url")
}

/// mock 截图产物：合法 1x1 PNG（#2b2f33）。
pub const MOCK_PNG: &[u8] = &[
    0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0x00, 0x00, 0x00, 0x0D, 0x49, 0x48, 0x44, 0x52,
    0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x02, 0x00, 0x00, 0x00, 0x90, 0x77, 0x53,
    0xDE, 0x00, 0x00, 0x00, 0x0C, 0x49, 0x44, 0x41, 0x54, 0x78, 0x9C, 0x63, 0xD0, 0xD6, 0x37, 0x06,
    0x00, 0x01, 0x16, 0x00, 0x8E, 0x44, 0xD8, 0x9D, 0x06, 0x00, 0x00, 0x00, 0x00, 0x49, 0x45, 0x4E,
    0x44, 0xAE, 0x42, 0x60, 0x82,
];

/// Mock 引擎驱动。
pub struct MockDriver {
    site: Arc<SiteModel>,
    caps: EngineCaps,
    /// 已 spawn 实例的崩溃注入端口（按 spawn 顺序）。
    ports: Mutex<Vec<Arc<CrashPort>>>,
}

impl MockDriver {
    pub fn new(site: SiteModel) -> Self {
        Self {
            site: Arc::new(site),
            caps: EngineCaps {
                snapshot: true,
                screenshot: true,
                input: true,
                eval: true,
                net_rules: true,
                state: true,
                events: true,
                metrics: true,
                lifecycle: true,
            },
            ports: Mutex::new(Vec::new()),
        }
    }

    /// 覆写能力矩阵（测试 E_UNSUPPORTED 路径用）。
    pub fn with_caps(mut self, caps: EngineCaps) -> Self {
        self.caps = caps;
        self
    }

    /// 以具体类型 spawn（测试用：故障注入 / eval 编程需要 `MockHandle` 方法）。
    pub fn spawn_mock(&self, _profile: &ProfileSpec) -> MockHandle {
        let h = MockHandle::new(Arc::clone(&self.site), self.caps);
        self.register(&h);
        h
    }

    /// 对第 `index` 个 spawn 出的实例注入崩溃（存在返回 true）。
    ///
    /// 供 kernel 崩溃监督测试：kernel 只持 `Box<dyn EngineHandle>`，
    /// 由 driver 侧触发崩溃事件。
    pub fn crash_spawned(&self, index: usize) -> bool {
        let ports = self
            .ports
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        match ports.get(index) {
            Some(p) => {
                p.crash();
                true
            }
            None => false,
        }
    }

    /// 对第 `index` 个 spawn 出的实例注入内存占用指标（配额测试用）。
    pub fn set_memory_spawned(&self, index: usize, bytes: u64) -> bool {
        let ports = self
            .ports
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        match ports.get(index) {
            Some(p) => {
                p.memory_bytes.store(bytes, Ordering::SeqCst);
                true
            }
            None => false,
        }
    }

    fn register(&self, h: &MockHandle) {
        self.ports
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(Arc::clone(&h.port));
    }

    /// 内建标准 fixture 站点（conformance 语义）。
    pub fn standard_fixture() -> Self {
        let base = fixture_base();
        let site = SiteBuilder::new(base.clone())
            .page(
                "/",
                PageModel::document("Fixture Home")
                    .child(NodeModel::heading("Fixture Home"))
                    .child(NodeModel::link("Go to Login", "/login")),
            )
            .page(
                "/login",
                PageModel::document("Login")
                    .child(NodeModel::textbox("Username"))
                    .child(NodeModel::textbox("Password"))
                    .child(NodeModel::button("Sign in", Some("/welcome"))),
            )
            .page(
                "/welcome",
                PageModel::document("Welcome").child(NodeModel::heading("Welcome")),
            )
            .page(
                "/widgets",
                PageModel::document("Widgets")
                    .child(NodeModel::combobox(
                        "Color",
                        &["red", "green", "blue"],
                        "red",
                    ))
                    .child(NodeModel::file_input("Attachment")),
            )
            .build();
        Self::new(site)
    }
}

#[async_trait]
impl EngineDriver for MockDriver {
    fn id(&self) -> &'static str {
        "mock"
    }

    fn capabilities(&self) -> EngineCaps {
        self.caps
    }

    async fn spawn(&self, _profile: &ProfileSpec) -> HalResult<Box<dyn EngineHandle>> {
        let h = MockHandle::new(Arc::clone(&self.site), self.caps);
        self.register(&h);
        Ok(Box::new(h))
    }
}

struct HandleState {
    current: Url,
    /// 导航历史（含当前页）与游标。
    history: Vec<Url>,
    cursor: usize,
    generation: u64,
    /// 当前代数下 ref index → 节点路径。
    ref_paths: HashMap<u64, Vec<usize>>,
    /// (页面 url, 节点路径) → 输入值。
    values: HashMap<(Url, Vec<usize>), String>,
    state_store: StateBundle,
    eval_responses: HashMap<String, Value>,
    policy: Option<Arc<dyn RequestPolicy>>,
    /// 引擎冻结态（set_lifecycle）。
    frozen: bool,
}

/// 崩溃/指标注入端口：driver 与 handle 共享。
struct CrashPort {
    crashed: AtomicBool,
    /// 注入的内存占用指标（配额测试）。
    memory_bytes: AtomicU64,
    events: broadcast::Sender<EngineEvent>,
}

impl CrashPort {
    fn crash(&self) {
        self.crashed.store(true, Ordering::SeqCst);
        let _ = self.events.send(EngineEvent::Crashed);
    }

    fn is_crashed(&self) -> bool {
        self.crashed.load(Ordering::SeqCst)
    }
}

/// Mock 引擎实例。除 HAL 接口外提供故障注入与 eval 编程接口。
pub struct MockHandle {
    site: Arc<SiteModel>,
    caps: EngineCaps,
    state: Mutex<HandleState>,
    port: Arc<CrashPort>,
}

impl MockHandle {
    fn new(site: Arc<SiteModel>, caps: EngineCaps) -> Self {
        let (events, _) = broadcast::channel(64);
        let blank = site.blank_url();
        Self {
            site,
            caps,
            state: Mutex::new(HandleState {
                current: blank.clone(),
                history: vec![blank],
                cursor: 0,
                generation: 0,
                ref_paths: HashMap::new(),
                values: HashMap::new(),
                state_store: StateBundle::default(),
                eval_responses: HashMap::new(),
                policy: None,
                frozen: false,
            }),
            port: Arc::new(CrashPort {
                crashed: AtomicBool::new(false),
                memory_bytes: AtomicU64::new(42 * 1024 * 1024),
                events,
            }),
        }
    }

    /// 故障注入：进入崩溃态，后续调用返回 E_ENGINE_CRASH 并广播 Crashed 事件。
    pub fn inject_crash(&self) {
        self.port.crash();
    }

    /// 注入内存占用指标（配额测试）。
    pub fn set_memory_bytes(&self, bytes: u64) {
        self.port.memory_bytes.store(bytes, Ordering::SeqCst);
    }

    /// 当前是否处于冻结态（suspend 断言用）。
    pub fn is_frozen(&self) -> bool {
        self.lock().frozen
    }

    /// 注册 js.exec 脚本响应（未注册脚本返回 Null）。
    pub fn program_eval(&self, script: impl Into<String>, response: Value) {
        self.lock().eval_responses.insert(script.into(), response);
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, HandleState> {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn ensure_alive(&self) -> HalResult<()> {
        if self.port.is_crashed() {
            Err(AbiError::new(ErrorCode::EngineCrash, "mock engine crashed"))
        } else {
            Ok(())
        }
    }

    /// 文档请求询问策略；拒 → 发事件并返回 E_NET_BLOCKED，页面状态不变。
    fn check_policy(&self, url: &Url) -> HalResult<()> {
        let Some(policy) = self.lock().policy.clone() else {
            return Ok(());
        };
        let summary = NetRequestSummary {
            url: url.to_string(),
            method: "GET".into(),
            resource_type: "document".into(),
        };
        let allowed = policy.decide(&summary).allowed();
        let _ = self
            .port
            .events
            .send(EngineEvent::NetRequest { summary, allowed });
        if allowed {
            Ok(())
        } else {
            Err(AbiError::new(
                ErrorCode::NetBlocked,
                format!("request to {url} blocked by policy"),
            ))
        }
    }

    fn navigate_internal(&self, url: &Url) -> NavResult {
        let page = self.site.resolve(url);
        let mut st = self.lock();
        st.current = url.clone();
        let cut = st.cursor + 1;
        st.history.truncate(cut);
        st.history.push(url.clone());
        st.cursor = st.history.len() - 1;
        st.ref_paths.clear();
        drop(st);
        let _ = self
            .port
            .events
            .send(EngineEvent::Navigated { url: url.clone() });
        NavResult {
            url: url.clone(),
            title: page.title.clone(),
        }
    }

    /// 设置当前页为历史中 cursor 指向的 URL（back/forward 用，不清剪历史）。
    fn move_history(&self, dir: HistoryDir) -> NavResult {
        let mut st = self.lock();
        match dir {
            HistoryDir::Back if st.cursor > 0 => st.cursor -= 1,
            HistoryDir::Forward if st.cursor + 1 < st.history.len() => st.cursor += 1,
            _ => {}
        }
        st.current = st.history[st.cursor].clone();
        st.ref_paths.clear();
        let url = st.current.clone();
        let page = self.site.resolve(&url);
        drop(st);
        let _ = self
            .port
            .events
            .send(EngineEvent::Navigated { url: url.clone() });
        NavResult {
            url,
            title: page.title.clone(),
        }
    }
}

#[async_trait]
impl EngineHandle for MockHandle {
    async fn navigate(&self, url: &Url) -> HalResult<NavResult> {
        self.ensure_alive()?;
        self.check_policy(url)?;
        Ok(self.navigate_internal(url))
    }

    async fn page_info(&self) -> HalResult<NavResult> {
        self.ensure_alive()?;
        let st = self.lock();
        let page = self.site.resolve(&st.current);
        Ok(NavResult {
            url: st.current.clone(),
            title: page.title.clone(),
        })
    }

    async fn history(&self, dir: HistoryDir) -> HalResult<NavResult> {
        self.ensure_alive()?;
        Ok(self.move_history(dir))
    }

    async fn reload(&self) -> HalResult<NavResult> {
        self.ensure_alive()?;
        let current = self.lock().current.clone();
        self.check_policy(&current)?;
        let (url, page) = {
            let mut st = self.lock();
            st.ref_paths.clear();
            let url = st.current.clone();
            (url.clone(), self.site.resolve(&url))
        };
        let _ = self
            .port
            .events
            .send(EngineEvent::Navigated { url: url.clone() });
        Ok(NavResult {
            url,
            title: page.title.clone(),
        })
    }

    async fn snapshot(&self, opts: &SnapshotOpts) -> HalResult<A11ySnapshot> {
        self.ensure_alive()?;
        if !self.caps.snapshot {
            return Err(AbiError::new(
                ErrorCode::Unsupported,
                "snapshot not supported",
            ));
        }
        let mut st = self.lock();
        st.generation += 1;
        let generation = st.generation;
        let page = self.site.resolve(&st.current);
        let (root, ref_paths, truncated) =
            model::render_snapshot(page, generation, opts.max_nodes, &st.current, &st.values);
        st.ref_paths = ref_paths;
        Ok(A11ySnapshot {
            generation,
            root,
            truncated,
        })
    }

    async fn screenshot(&self) -> HalResult<Vec<u8>> {
        self.ensure_alive()?;
        if !self.caps.screenshot {
            return Err(AbiError::new(
                ErrorCode::Unsupported,
                "screenshot not supported",
            ));
        }
        // 合法 1x1 PNG（#2b2f33）：mock 不做真实渲染，但产物可被 <img>/解码器接受
        Ok(MOCK_PNG.to_vec())
    }

    async fn dispatch(&self, action: &InputAction) -> HalResult<ActResult> {
        self.ensure_alive()?;
        if !self.caps.input {
            return Err(AbiError::new(ErrorCode::Unsupported, "input not supported"));
        }
        let resolve = |target: &ElementRef| -> HalResult<Vec<usize>> {
            let st = self.lock();
            if target.is_stale(st.generation) {
                return Err(AbiError::new(
                    ErrorCode::RefStale,
                    format!(
                        "ref {target} is stale (current generation {})",
                        st.generation
                    ),
                ));
            }
            st.ref_paths.get(&target.index()).cloned().ok_or_else(|| {
                AbiError::new(ErrorCode::InvalidArg, format!("unknown ref {target}"))
            })
        };

        match action {
            InputAction::Click { target } => {
                let path = resolve(target)?;
                let (page, current) = {
                    let st = self.lock();
                    (self.site.resolve(&st.current), st.current.clone())
                };
                let node = page
                    .node_at(&path)
                    .ok_or_else(|| AbiError::new(ErrorCode::Internal, "ref path out of tree"))?;
                match &node.on_click {
                    Some(dest) => {
                        let url = current.join(dest).map_err(|e| {
                            AbiError::new(ErrorCode::Internal, format!("bad on_click url: {e}"))
                        })?;
                        self.check_policy(&url)?;
                        self.navigate_internal(&url);
                        Ok(ActResult { nav_occurred: true })
                    }
                    None => Ok(ActResult {
                        nav_occurred: false,
                    }),
                }
            }
            InputAction::Type { target, text } => {
                let path = resolve(target)?;
                let mut st = self.lock();
                let key = (st.current.clone(), path);
                st.values.entry(key).or_default().push_str(text);
                Ok(ActResult {
                    nav_occurred: false,
                })
            }
            InputAction::Select { target, values } => {
                let path = resolve(target)?;
                let (page, current) = {
                    let st = self.lock();
                    (self.site.resolve(&st.current), st.current.clone())
                };
                let node = page
                    .node_at(&path)
                    .ok_or_else(|| AbiError::new(ErrorCode::Internal, "ref path out of tree"))?;
                if node.role != "combobox" {
                    return Err(AbiError::new(
                        ErrorCode::InvalidArg,
                        format!("select target must be combobox, got {}", node.role),
                    ));
                }
                let [value] = values.as_slice() else {
                    return Err(AbiError::new(
                        ErrorCode::InvalidArg,
                        "single-select requires exactly one value",
                    ));
                };
                if !node.options.contains(value) {
                    return Err(AbiError::new(
                        ErrorCode::InvalidArg,
                        format!("no option {value:?}"),
                    ));
                }
                self.lock().values.insert((current, path), value.clone());
                Ok(ActResult {
                    nav_occurred: false,
                })
            }
            InputAction::Upload { target, paths } => {
                let path = resolve(target)?;
                let (page, current) = {
                    let st = self.lock();
                    (self.site.resolve(&st.current), st.current.clone())
                };
                let node = page
                    .node_at(&path)
                    .ok_or_else(|| AbiError::new(ErrorCode::Internal, "ref path out of tree"))?;
                if node.role != "fileinput" {
                    return Err(AbiError::new(
                        ErrorCode::InvalidArg,
                        format!("upload target must be fileinput, got {}", node.role),
                    ));
                }
                if paths.is_empty() {
                    return Err(AbiError::new(ErrorCode::InvalidArg, "no files given"));
                }
                let names: Vec<String> = paths
                    .iter()
                    .map(|p| p.file_name().unwrap_or_default().to_string_lossy().into())
                    .collect();
                self.lock().values.insert((current, path), names.join(","));
                Ok(ActResult {
                    nav_occurred: false,
                })
            }
            InputAction::Press { .. } | InputAction::Scroll { .. } => Ok(ActResult {
                nav_occurred: false,
            }),
        }
    }

    async fn eval(&self, script: &str, _args: &[Value]) -> HalResult<Value> {
        self.ensure_alive()?;
        if !self.caps.eval {
            return Err(AbiError::new(ErrorCode::Unsupported, "eval not supported"));
        }
        Ok(self
            .lock()
            .eval_responses
            .get(script)
            .cloned()
            .unwrap_or(Value::Null))
    }

    async fn export_state(&self) -> HalResult<StateBundle> {
        self.ensure_alive()?;
        if !self.caps.state {
            return Err(AbiError::new(ErrorCode::Unsupported, "state not supported"));
        }
        Ok(self.lock().state_store.clone())
    }

    async fn import_state(&self, bundle: &StateBundle) -> HalResult<()> {
        self.ensure_alive()?;
        if !self.caps.state {
            return Err(AbiError::new(ErrorCode::Unsupported, "state not supported"));
        }
        let mut st = self.lock();
        st.state_store.entries.extend(bundle.entries.clone());
        Ok(())
    }

    async fn set_request_policy(&self, policy: Option<Arc<dyn RequestPolicy>>) -> HalResult<()> {
        self.ensure_alive()?;
        if !self.caps.net_rules {
            return Err(AbiError::new(
                ErrorCode::Unsupported,
                "net rules not supported",
            ));
        }
        self.lock().policy = policy;
        Ok(())
    }

    async fn set_lifecycle(&self, frozen: bool) -> HalResult<()> {
        self.ensure_alive()?;
        if !self.caps.lifecycle {
            return Err(AbiError::new(
                ErrorCode::Unsupported,
                "lifecycle not supported",
            ));
        }
        self.lock().frozen = frozen;
        Ok(())
    }

    fn events(&self) -> broadcast::Receiver<EngineEvent> {
        self.port.events.subscribe()
    }

    async fn metrics(&self) -> HalResult<EngineMetrics> {
        self.ensure_alive()?;
        if !self.caps.metrics {
            return Err(AbiError::new(
                ErrorCode::Unsupported,
                "metrics not supported",
            ));
        }
        Ok(EngineMetrics {
            memory_bytes: self.port.memory_bytes.load(Ordering::SeqCst),
        })
    }

    async fn shutdown(&self) -> HalResult<()> {
        Ok(())
    }
}
