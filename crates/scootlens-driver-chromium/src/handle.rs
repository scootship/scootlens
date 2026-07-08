//! ChromiumHandle：一个 headless Chromium 进程 = 一个引擎实例。

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use async_trait::async_trait;
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64;
use scootlens_abi::{AbiError, ErrorCode};
use scootlens_hal::{
    A11ySnapshot, ActResult, EngineEvent, EngineHandle, EngineMetrics, HalResult, HistoryDir,
    InputAction, NavResult, RequestPolicy, SnapshotOpts, StateBundle,
};
use serde_json::{Value, json};
use tokio::sync::broadcast;
use url::Url;

use crate::cdp::{CdpConn, CdpEvent};
use crate::process::BrowserProcess;
use crate::snapshot;

const NAV_TIMEOUT: Duration = Duration::from_secs(10);
/// 点击/按键后判定"是否引发导航"的观察窗口。
const NAV_PROBE: Duration = Duration::from_millis(900);
/// 固定视口尺寸（`process.rs` 启动参数 `--window-size` 用同一常量）；
/// `act.point.click` 的归一化坐标按此换算为像素（ADR-0010）。
pub(crate) const VIEWPORT_WIDTH: u32 = 1280;
pub(crate) const VIEWPORT_HEIGHT: u32 = 800;

struct RefTable {
    generation: u64,
    backend_ids: HashMap<u64, i64>,
}

pub(crate) struct ChromiumHandle {
    conn: Arc<CdpConn>,
    session_id: String,
    refs: Mutex<RefTable>,
    events: broadcast::Sender<EngineEvent>,
    shutting_down: Arc<AtomicBool>,
    shutdown_tx: tokio::sync::mpsc::Sender<()>,
    tasks: Vec<tokio::task::JoinHandle<()>>,
}

impl ChromiumHandle {
    /// 完整启动：进程 → CDP 连接 → target/session → 域使能 → 监督任务。
    pub async fn boot(process: BrowserProcess, conn: CdpConn) -> HalResult<Self> {
        let conn = Arc::new(conn);

        let target = conn
            .call(None, "Target.createTarget", json!({ "url": "about:blank" }))
            .await?;
        let target_id = target["targetId"]
            .as_str()
            .ok_or_else(|| AbiError::new(ErrorCode::Internal, "no targetId"))?
            .to_owned();
        let attach = conn
            .call(
                None,
                "Target.attachToTarget",
                json!({ "targetId": target_id, "flatten": true }),
            )
            .await?;
        let session_id = attach["sessionId"]
            .as_str()
            .ok_or_else(|| AbiError::new(ErrorCode::Internal, "no sessionId"))?
            .to_owned();

        for domain in [
            "Page.enable",
            "Runtime.enable",
            "DOM.enable",
            "Accessibility.enable",
        ] {
            conn.call(Some(&session_id), domain, json!({})).await?;
        }

        let (events, _) = broadcast::channel(256);
        let shutting_down = Arc::new(AtomicBool::new(false));
        let (shutdown_tx, shutdown_rx) = tokio::sync::mpsc::channel::<()>(1);

        let watcher = tokio::spawn(watch_process(
            process,
            Arc::clone(&shutting_down),
            events.clone(),
            shutdown_rx,
        ));
        let bridge = tokio::spawn(bridge_events(
            conn.subscribe(),
            session_id.clone(),
            events.clone(),
        ));

        Ok(Self {
            conn,
            session_id,
            refs: Mutex::new(RefTable {
                generation: 0,
                backend_ids: HashMap::new(),
            }),
            events,
            shutting_down,
            shutdown_tx,
            tasks: vec![watcher, bridge],
        })
    }

    async fn call(&self, method: &str, params: Value) -> HalResult<Value> {
        self.conn.call(Some(&self.session_id), method, params).await
    }

    /// 当前页 url + title。
    async fn current_page(&self) -> HalResult<NavResult> {
        let v = self
            .call(
                "Runtime.evaluate",
                json!({
                    "expression": "JSON.stringify({url: location.href, title: document.title})",
                    "returnByValue": true,
                }),
            )
            .await?;
        let raw = v
            .pointer("/result/value")
            .and_then(Value::as_str)
            .ok_or_else(|| AbiError::new(ErrorCode::Internal, "page info unavailable"))?;
        let parsed: Value = serde_json::from_str(raw)
            .map_err(|e| AbiError::new(ErrorCode::Internal, format!("page info: {e}")))?;
        let url: Url = parsed["url"]
            .as_str()
            .unwrap_or_default()
            .parse()
            .map_err(|e| AbiError::new(ErrorCode::Internal, format!("bad url: {e}")))?;
        Ok(NavResult {
            url,
            title: parsed["title"].as_str().unwrap_or_default().to_owned(),
        })
    }

    /// 在 `rx` 上等待本 session 的指定事件。
    async fn wait_on(
        &self,
        rx: &mut broadcast::Receiver<CdpEvent>,
        method: &str,
        timeout: Duration,
    ) -> bool {
        let fut = async {
            loop {
                match rx.recv().await {
                    Ok(e)
                        if e.method == method
                            && e.session_id.as_deref() == Some(&self.session_id) =>
                    {
                        return true;
                    }
                    Ok(_) => continue,
                    Err(broadcast::error::RecvError::Lagged(_)) => continue,
                    Err(broadcast::error::RecvError::Closed) => return false,
                }
            }
        };
        tokio::time::timeout(timeout, fut).await.unwrap_or(false)
    }

    /// 解析 ref → backendDOMNodeId（校验代数）。
    fn resolve_ref(&self, r: &scootlens_abi::ElementRef) -> HalResult<i64> {
        let table = self.refs.lock().unwrap_or_else(PoisonError::into_inner);
        if r.generation() != table.generation {
            return Err(AbiError::new(
                ErrorCode::RefStale,
                format!(
                    "ref generation {} != current {}",
                    r.generation(),
                    table.generation
                ),
            ));
        }
        table.backend_ids.get(&r.index()).copied().ok_or_else(|| {
            AbiError::new(
                ErrorCode::InvalidArg,
                format!("unknown ref index {}", r.index()),
            )
        })
    }

    /// 元素中心视口坐标。
    async fn center_of(&self, backend_id: i64) -> HalResult<(f64, f64)> {
        let v = self
            .call(
                "DOM.getContentQuads",
                json!({ "backendNodeId": backend_id }),
            )
            .await
            .map_err(|e| {
                AbiError::new(ErrorCode::InvalidArg, format!("element not on page: {e}"))
            })?;
        let quad = v["quads"]
            .as_array()
            .and_then(|q| q.first())
            .and_then(Value::as_array)
            .ok_or_else(|| AbiError::new(ErrorCode::InvalidArg, "element has no geometry"))?;
        let nums: Vec<f64> = quad.iter().filter_map(Value::as_f64).collect();
        if nums.len() != 8 {
            return Err(AbiError::new(ErrorCode::Internal, "bad quad"));
        }
        let x = (nums[0] + nums[2] + nums[4] + nums[6]) / 4.0;
        let y = (nums[1] + nums[3] + nums[5] + nums[7]) / 4.0;
        Ok((x, y))
    }

    async fn click_at(&self, x: f64, y: f64) -> HalResult<()> {
        for kind in ["mousePressed", "mouseReleased"] {
            self.call(
                "Input.dispatchMouseEvent",
                json!({
                    "type": kind, "x": x, "y": y,
                    "button": "left", "clickCount": 1,
                }),
            )
            .await?;
        }
        Ok(())
    }

    async fn press_key(&self, key: &str) -> HalResult<()> {
        let (code, key_code, text) = key_info(key)?;
        for kind in ["keyDown", "keyUp"] {
            let mut p = json!({
                "type": kind, "key": key, "code": code,
                "windowsVirtualKeyCode": key_code, "nativeVirtualKeyCode": key_code,
            });
            if kind == "keyDown" && !text.is_empty() {
                p["text"] = json!(text);
            }
            self.call("Input.dispatchKeyEvent", p).await?;
        }
        Ok(())
    }
}

#[async_trait]
impl EngineHandle for ChromiumHandle {
    async fn navigate(&self, url: &Url) -> HalResult<NavResult> {
        let mut rx = self.conn.subscribe();
        let v = self
            .call("Page.navigate", json!({ "url": url.as_str() }))
            .await?;
        if let Some(err) = v.get("errorText").and_then(Value::as_str)
            && !err.is_empty()
        {
            return Err(AbiError::new(
                ErrorCode::Internal,
                format!("navigation failed: {err}"),
            ));
        }
        if !self
            .wait_on(&mut rx, "Page.loadEventFired", NAV_TIMEOUT)
            .await
        {
            return Err(AbiError::new(ErrorCode::Timeout, "page load timeout"));
        }
        self.current_page().await
    }

    async fn page_info(&self) -> HalResult<NavResult> {
        self.current_page().await
    }

    async fn history(&self, dir: HistoryDir) -> HalResult<NavResult> {
        let v = self.call("Page.getNavigationHistory", json!({})).await?;
        let current = v["currentIndex"].as_i64().unwrap_or(0);
        let entries = v["entries"].as_array().cloned().unwrap_or_default();
        let target = match dir {
            HistoryDir::Back => current - 1,
            HistoryDir::Forward => current + 1,
        };
        let Some(entry) = usize::try_from(target).ok().and_then(|i| entries.get(i)) else {
            return self.current_page().await; // 历史耗尽：no-op
        };
        let entry_id = entry["id"]
            .as_i64()
            .ok_or_else(|| AbiError::new(ErrorCode::Internal, "bad history entry"))?;
        let mut rx = self.conn.subscribe();
        self.call(
            "Page.navigateToHistoryEntry",
            json!({ "entryId": entry_id }),
        )
        .await?;
        self.wait_on(&mut rx, "Page.loadEventFired", NAV_TIMEOUT)
            .await;
        self.current_page().await
    }

    async fn reload(&self) -> HalResult<NavResult> {
        let mut rx = self.conn.subscribe();
        self.call("Page.reload", json!({})).await?;
        if !self
            .wait_on(&mut rx, "Page.loadEventFired", NAV_TIMEOUT)
            .await
        {
            return Err(AbiError::new(ErrorCode::Timeout, "reload timeout"));
        }
        self.current_page().await
    }

    async fn snapshot(&self, opts: &SnapshotOpts) -> HalResult<A11ySnapshot> {
        let v = self.call("Accessibility.getFullAXTree", json!({})).await?;
        let nodes = v["nodes"]
            .as_array()
            .ok_or_else(|| AbiError::new(ErrorCode::Internal, "no AX nodes"))?;

        let mut table = self.refs.lock().unwrap_or_else(PoisonError::into_inner);
        table.generation += 1;
        let generation = table.generation;
        let converted = snapshot::convert(nodes, generation, opts.max_nodes);
        table.backend_ids = converted.backend_ids;
        drop(table);

        Ok(A11ySnapshot {
            generation,
            root: converted.root,
            truncated: converted.truncated,
        })
    }

    async fn screenshot(&self) -> HalResult<Vec<u8>> {
        let v = self
            .call("Page.captureScreenshot", json!({ "format": "png" }))
            .await?;
        let data = v["data"]
            .as_str()
            .ok_or_else(|| AbiError::new(ErrorCode::Internal, "no screenshot data"))?;
        BASE64
            .decode(data)
            .map_err(|e| AbiError::new(ErrorCode::Internal, format!("bad base64: {e}")))
    }

    async fn dispatch(&self, action: &InputAction) -> HalResult<ActResult> {
        match action {
            InputAction::Click { target } => {
                let backend = self.resolve_ref(target)?;
                let (x, y) = self.center_of(backend).await?;
                let mut rx = self.conn.subscribe();
                self.click_at(x, y).await?;
                let nav = self
                    .wait_on(&mut rx, "Page.loadEventFired", NAV_PROBE)
                    .await;
                Ok(ActResult { nav_occurred: nav })
            }
            InputAction::Type { target, text } => {
                let backend = self.resolve_ref(target)?;
                self.call("DOM.focus", json!({ "backendNodeId": backend }))
                    .await
                    .map_err(|e| {
                        AbiError::new(ErrorCode::InvalidArg, format!("cannot focus: {e}"))
                    })?;
                self.call("Input.insertText", json!({ "text": text }))
                    .await?;
                Ok(ActResult {
                    nav_occurred: false,
                })
            }
            InputAction::Press { keys } => {
                let mut rx = self.conn.subscribe();
                self.press_key(keys).await?;
                let nav = self
                    .wait_on(&mut rx, "Page.loadEventFired", NAV_PROBE)
                    .await;
                Ok(ActResult { nav_occurred: nav })
            }
            InputAction::Scroll { target, dx, dy } => {
                let (x, y) = match target {
                    Some(r) => {
                        let backend = self.resolve_ref(r)?;
                        self.center_of(backend).await?
                    }
                    None => (400.0, 300.0),
                };
                self.call(
                    "Input.dispatchMouseEvent",
                    json!({
                        "type": "mouseWheel", "x": x, "y": y,
                        "deltaX": dx, "deltaY": dy,
                    }),
                )
                .await?;
                Ok(ActResult {
                    nav_occurred: false,
                })
            }
            InputAction::Select { target, values } => {
                let backend = self.resolve_ref(target)?;
                let obj = self
                    .call("DOM.resolveNode", json!({ "backendNodeId": backend }))
                    .await?;
                let object_id = obj
                    .pointer("/object/objectId")
                    .and_then(Value::as_str)
                    .ok_or_else(|| AbiError::new(ErrorCode::Internal, "no objectId"))?;
                let decl = r#"function(vals) {
                    if (this.tagName !== 'SELECT') throw new Error('target is not a <select>');
                    if (!this.multiple && vals.length !== 1) throw new Error('single-select requires exactly one value');
                    const want = Array.from(new Set(vals));
                    const opts = Array.from(this.options);
                    const matches = (o, w) => o.value === w || o.textContent.trim() === w;
                    for (const w of want) {
                        if (!opts.some((o) => matches(o, w))) throw new Error('no matching option: ' + w);
                    }
                    if (this.multiple) {
                        for (const o of opts) o.selected = want.some((w) => matches(o, w));
                    } else {
                        this.value = opts.find((o) => matches(o, want[0])).value;
                    }
                    this.dispatchEvent(new Event('input', { bubbles: true }));
                    this.dispatchEvent(new Event('change', { bubbles: true }));
                }"#;
                let r = self
                    .call(
                        "Runtime.callFunctionOn",
                        json!({
                            "objectId": object_id,
                            "functionDeclaration": decl,
                            "arguments": [{ "value": values }],
                        }),
                    )
                    .await?;
                if let Some(ex) = r.get("exceptionDetails") {
                    let text = ex
                        .pointer("/exception/description")
                        .or_else(|| ex.get("text"))
                        .and_then(Value::as_str)
                        .unwrap_or("select failed");
                    return Err(AbiError::new(ErrorCode::InvalidArg, text));
                }
                Ok(ActResult {
                    nav_occurred: false,
                })
            }
            InputAction::Upload { target, paths } => {
                let backend = self.resolve_ref(target)?;
                let files: Vec<String> = paths
                    .iter()
                    .map(|p| p.to_string_lossy().into_owned())
                    .collect();
                self.call(
                    "DOM.setFileInputFiles",
                    json!({ "files": files, "backendNodeId": backend }),
                )
                .await
                .map_err(|e| {
                    AbiError::new(ErrorCode::InvalidArg, format!("setFileInputFiles: {e}"))
                })?;
                Ok(ActResult {
                    nav_occurred: false,
                })
            }
            InputAction::ClickAt { x_ratio, y_ratio } => {
                // 归一化比例 → 本引擎固定视口像素（内核已校验 [0,1]，kernel 也已
                // 校验调用者持有接管；这里只管坐标换算与单击本身）。
                let x = x_ratio * f64::from(VIEWPORT_WIDTH);
                let y = y_ratio * f64::from(VIEWPORT_HEIGHT);
                let mut rx = self.conn.subscribe();
                self.click_at(x, y).await?;
                let nav = self
                    .wait_on(&mut rx, "Page.loadEventFired", NAV_PROBE)
                    .await;
                Ok(ActResult { nav_occurred: nav })
            }
        }
    }

    async fn eval(&self, script: &str, args: &[Value]) -> HalResult<Value> {
        if !args.is_empty() {
            return Err(AbiError::new(
                ErrorCode::Unsupported,
                "js.exec args are not supported in P1 (use template literals)",
            ));
        }
        let v = self
            .call(
                "Runtime.evaluate",
                json!({
                    "expression": script,
                    "returnByValue": true,
                    "awaitPromise": true,
                }),
            )
            .await?;
        if let Some(ex) = v.get("exceptionDetails") {
            let text = ex
                .pointer("/exception/description")
                .or_else(|| ex.get("text"))
                .and_then(Value::as_str)
                .unwrap_or("uncaught exception");
            return Err(AbiError::new(ErrorCode::Internal, format!("js: {text}")));
        }
        Ok(v.pointer("/result/value").cloned().unwrap_or(Value::Null))
    }

    async fn export_state(&self) -> HalResult<StateBundle> {
        let mut bundle = StateBundle::default();

        // cookies：完整属性作为不透明值保存（restore 时原样 setCookie）
        let v = self.call("Network.getCookies", json!({})).await?;
        if let Some(cookies) = v["cookies"].as_array() {
            for c in cookies {
                if let Some(name) = c["name"].as_str() {
                    bundle.entries.insert(
                        format!("cookie:{name}"),
                        json!({
                            "value": c["value"],
                            "domain": c["domain"],
                            "path": c["path"],
                            "secure": c["secure"],
                            "httpOnly": c["httpOnly"],
                        }),
                    );
                }
            }
        }

        // localStorage：当前 origin 的全部键值
        let v = self
            .call(
                "Runtime.evaluate",
                json!({
                    "expression": "try { JSON.stringify(Object.entries(localStorage)) } catch (_) { \"[]\" }",
                    "returnByValue": true,
                }),
            )
            .await?;
        if let Some(text) = v.pointer("/result/value").and_then(Value::as_str)
            && let Ok(Value::Array(pairs)) = serde_json::from_str::<Value>(text)
        {
            for pair in pairs {
                if let (Some(k), Some(val)) = (pair[0].as_str(), pair.get(1)) {
                    bundle.entries.insert(format!("storage:{k}"), val.clone());
                }
            }
        }
        Ok(bundle)
    }

    async fn import_state(&self, bundle: &StateBundle) -> HalResult<()> {
        let page_url = self.current_page().await?.url;
        for (key, value) in &bundle.entries {
            if let Some(name) = key.strip_prefix("cookie:") {
                let mut params = json!({ "name": name });
                match value {
                    // export_state 产出的完整 cookie 对象
                    Value::Object(o) => {
                        params["value"] = o.get("value").cloned().unwrap_or(Value::Null);
                        for k in ["domain", "path", "secure", "httpOnly"] {
                            if let Some(v) = o.get(k) {
                                params[k] = v.clone();
                            }
                        }
                    }
                    // 裸值：以当前页 URL 为上下文
                    other => {
                        params["value"] = json!(value_as_cookie_str(other));
                        params["url"] = json!(page_url.as_str());
                    }
                }
                self.call("Network.setCookie", params).await?;
            } else if let Some(name) = key.strip_prefix("storage:") {
                let k = serde_json::to_string(name).unwrap_or_default();
                let v = serde_json::to_string(&value_as_cookie_str(value)).unwrap_or_default();
                self.call(
                    "Runtime.evaluate",
                    json!({
                        "expression": format!("localStorage.setItem({k}, {v})"),
                        "returnByValue": true,
                    }),
                )
                .await?;
            }
        }
        Ok(())
    }

    async fn set_request_policy(&self, _policy: Option<Arc<dyn RequestPolicy>>) -> HalResult<()> {
        Err(AbiError::new(
            ErrorCode::Unsupported,
            "net rules land in a later chromium milestone (Fetch interception)",
        ))
    }

    async fn set_lifecycle(&self, frozen: bool) -> HalResult<()> {
        let state = if frozen { "frozen" } else { "active" };
        self.call("Page.setWebLifecycleState", json!({ "state": state }))
            .await?;
        Ok(())
    }

    fn events(&self) -> broadcast::Receiver<EngineEvent> {
        self.events.subscribe()
    }

    async fn metrics(&self) -> HalResult<EngineMetrics> {
        // Performance.enable 幂等；getMetrics 取 JSHeapUsedSize 作为内存水位
        self.call("Performance.enable", json!({})).await?;
        let v = self.call("Performance.getMetrics", json!({})).await?;
        let memory_bytes = v["metrics"]
            .as_array()
            .into_iter()
            .flatten()
            .find(|m| m["name"] == "JSHeapUsedSize")
            .and_then(|m| m["value"].as_f64())
            .unwrap_or(0.0) as u64;
        Ok(EngineMetrics { memory_bytes })
    }

    async fn shutdown(&self) -> HalResult<()> {
        if self.shutting_down.swap(true, Ordering::SeqCst) {
            return Ok(()); // 幂等
        }
        // 优雅关闭；失败无妨，watcher 会兜底 kill
        let _ = self.conn.call(None, "Browser.close", json!({})).await;
        let _ = self.shutdown_tx.send(()).await;
        Ok(())
    }
}

impl Drop for ChromiumHandle {
    fn drop(&mut self) {
        self.shutting_down.store(true, Ordering::SeqCst);
        for t in &self.tasks {
            t.abort();
        }
    }
}

/// 进程监督：非预期退出 → Crashed 事件；收到 shutdown 信号 → 限时等待后 kill。
async fn watch_process(
    mut process: BrowserProcess,
    shutting_down: Arc<AtomicBool>,
    events: broadcast::Sender<EngineEvent>,
    mut shutdown_rx: tokio::sync::mpsc::Receiver<()>,
) {
    tokio::select! {
        status = process.child.wait() => {
            if !shutting_down.load(Ordering::SeqCst) {
                tracing::warn!(?status, "chromium exited unexpectedly");
                let _ = events.send(EngineEvent::Crashed);
            }
        }
        _ = shutdown_rx.recv() => {
            // Browser.close 已发出：给 2s 优雅退出，否则强杀
            let graceful =
                tokio::time::timeout(Duration::from_secs(2), process.child.wait()).await;
            if graceful.is_err() {
                let _ = process.child.kill().await;
            }
        }
    }
}

/// CDP 事件 → HAL 引擎事件。
async fn bridge_events(
    mut rx: broadcast::Receiver<CdpEvent>,
    session_id: String,
    events: broadcast::Sender<EngineEvent>,
) {
    loop {
        let e = match rx.recv().await {
            Ok(e) => e,
            Err(broadcast::error::RecvError::Lagged(_)) => continue,
            Err(broadcast::error::RecvError::Closed) => break,
        };
        if e.session_id.as_deref() != Some(&session_id) {
            continue;
        }
        let mapped = match e.method.as_str() {
            "Page.frameNavigated" => {
                // 只关心主 frame（无 parentId）
                if e.params.pointer("/frame/parentId").is_some() {
                    continue;
                }
                e.params
                    .pointer("/frame/url")
                    .and_then(Value::as_str)
                    .and_then(|u| u.parse::<Url>().ok())
                    .map(|url| EngineEvent::Navigated { url })
            }
            "Runtime.consoleAPICalled" => {
                let text = e.params["args"]
                    .as_array()
                    .map(|args| {
                        args.iter()
                            .filter_map(|a| {
                                a.get("value").map(|v| match v {
                                    Value::String(s) => s.clone(),
                                    other => other.to_string(),
                                })
                            })
                            .collect::<Vec<_>>()
                            .join(" ")
                    })
                    .unwrap_or_default();
                Some(EngineEvent::ConsoleLog { text })
            }
            "Inspector.targetCrashed" => Some(EngineEvent::Crashed),
            _ => None,
        };
        if let Some(evt) = mapped
            && events.send(evt).is_err()
        {
            break;
        }
    }
}

/// P1 最小按键表。
fn key_info(key: &str) -> HalResult<(&'static str, u32, &'static str)> {
    Ok(match key {
        "Enter" => ("Enter", 13, "\r"),
        "Tab" => ("Tab", 9, "\t"),
        "Escape" => ("Escape", 27, ""),
        "Backspace" => ("Backspace", 8, ""),
        "ArrowDown" => ("ArrowDown", 40, ""),
        "ArrowUp" => ("ArrowUp", 38, ""),
        "ArrowLeft" => ("ArrowLeft", 37, ""),
        "ArrowRight" => ("ArrowRight", 39, ""),
        other => {
            return Err(AbiError::new(
                ErrorCode::InvalidArg,
                format!("unsupported key: {other} (P1 supports Enter/Tab/Escape/Backspace/Arrows)"),
            ));
        }
    })
}

/// 状态值转字符串：字符串取原文，其余序列化（导入 cookie/localStorage 用）。
fn value_as_cookie_str(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}
