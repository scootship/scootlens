//! syscall 分发层：`RpcRequest` → 内核调用 → `RpcResponse`。
//!
//! - 参数用 serde 强校验，失败 → `E_INVALID_ARG`
//! - 方法表内但本阶段未落地 → `E_UNSUPPORTED`
//! - 方法表外 → JSON-RPC `-32601` Method not found
//! - `evt.subscribe/unsubscribe` 是连接级语义，由 gateway 处理；进到这里 → `E_UNSUPPORTED`

use std::time::Duration;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64;
use scootlens_abi::{
    AbiError, ErrorCode, Pid, RpcError, RpcId, RpcOutcome, RpcRequest, RpcResponse, method,
};
use scootlens_hal::{HistoryDir, InputAction, ProfileSpec, SnapshotOpts};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::Kernel;
use crate::bus::BusPayload;

/// syscall 分发器。廉价 Clone。
#[derive(Clone)]
pub struct Dispatcher {
    kernel: Kernel,
}

impl Dispatcher {
    pub fn new(kernel: Kernel) -> Self {
        Self { kernel }
    }

    /// 底层内核（gateway 订阅事件用）。
    pub fn kernel(&self) -> &Kernel {
        &self.kernel
    }

    /// 分发一个请求。任何错误都折叠进 `RpcResponse`，本函数不失败。
    pub async fn dispatch(&self, req: RpcRequest) -> RpcResponse {
        let id = req.id.clone();
        if !method::is_known(&req.method) {
            return method_not_found(id, &req.method);
        }
        match self.route(&req.method, req.params).await {
            Ok(result) => RpcResponse::success(id, result),
            Err(e) => RpcResponse::failure(id, e),
        }
    }

    async fn route(&self, m: &str, params: Value) -> Result<Value, AbiError> {
        let k = &self.kernel;
        match m {
            method::PROC_SPAWN => {
                let p: SpawnParams = parse(params)?;
                let profile = ProfileSpec {
                    name: p.profile.unwrap_or_else(|| "default".into()),
                };
                let pid = k.spawn(profile).await?;
                Ok(json!({ "pid": pid }))
            }
            method::PROC_LIST => {
                let _: Empty = parse(params)?;
                Ok(json!({ "procs": k.list().await }))
            }
            method::PROC_INFO => {
                let p: PidParams = parse(params)?;
                Ok(to_value(k.info(&parse_pid(&p.pid)?).await?))
            }
            method::PROC_KILL => {
                let p: PidParams = parse(params)?;
                k.kill(&parse_pid(&p.pid)?).await?;
                Ok(json!({ "ok": true }))
            }
            method::NAV_GOTO => {
                let p: GotoParams = parse(params)?;
                let url = parse_url(&p.url)?;
                Ok(to_value(k.navigate(&parse_pid(&p.pid)?, &url).await?))
            }
            method::NAV_BACK => {
                let p: PidParams = parse(params)?;
                Ok(to_value(
                    k.history(&parse_pid(&p.pid)?, HistoryDir::Back).await?,
                ))
            }
            method::NAV_FORWARD => {
                let p: PidParams = parse(params)?;
                Ok(to_value(
                    k.history(&parse_pid(&p.pid)?, HistoryDir::Forward).await?,
                ))
            }
            method::NAV_RELOAD => {
                let p: PidParams = parse(params)?;
                Ok(to_value(k.reload(&parse_pid(&p.pid)?).await?))
            }
            method::VIEW_SNAPSHOT => {
                let p: SnapshotParams = parse(params)?;
                let opts = SnapshotOpts {
                    max_nodes: p
                        .max_nodes
                        .unwrap_or_else(|| SnapshotOpts::default().max_nodes),
                };
                let snap = k.snapshot(&parse_pid(&p.pid)?, &opts).await?;
                Ok(json!({
                    "generation": snap.generation,
                    "truncated": snap.truncated,
                    "text": snap.to_compact_text(),
                }))
            }
            method::VIEW_SCREENSHOT => {
                let p: PidParams = parse(params)?;
                let bytes = k.screenshot(&parse_pid(&p.pid)?).await?;
                Ok(json!({ "format": "png", "data_base64": BASE64.encode(bytes) }))
            }
            method::ACT_CLICK => {
                let p: RefParams = parse(params)?;
                let action = InputAction::Click {
                    target: parse_ref(&p.r#ref)?,
                };
                Ok(to_value(k.dispatch(&parse_pid(&p.pid)?, &action).await?))
            }
            method::ACT_TYPE => {
                let p: TypeParams = parse(params)?;
                let action = InputAction::Type {
                    target: parse_ref(&p.r#ref)?,
                    text: p.text,
                };
                Ok(to_value(k.dispatch(&parse_pid(&p.pid)?, &action).await?))
            }
            method::ACT_PRESS => {
                let p: PressParams = parse(params)?;
                let action = InputAction::Press { keys: p.keys };
                Ok(to_value(k.dispatch(&parse_pid(&p.pid)?, &action).await?))
            }
            method::ACT_SCROLL => {
                let p: ScrollParams = parse(params)?;
                let target = match &p.r#ref {
                    Some(r) => Some(parse_ref(r)?),
                    None => None,
                };
                let action = InputAction::Scroll {
                    target,
                    dx: p.dx,
                    dy: p.dy,
                };
                Ok(to_value(k.dispatch(&parse_pid(&p.pid)?, &action).await?))
            }
            method::JS_EXEC => {
                let p: EvalParams = parse(params)?;
                let out = k
                    .eval(&parse_pid(&p.pid)?, &p.script, &p.args.unwrap_or_default())
                    .await?;
                Ok(json!({ "value": out }))
            }
            method::EVT_WAIT => {
                let p: WaitParams = parse(params)?;
                self.wait_event(p).await
            }
            method::SYS_INFO => {
                let _: Empty = parse(params)?;
                Ok(to_value(k.sys_info().await))
            }
            method::EVT_SUBSCRIBE | method::EVT_UNSUBSCRIBE => Err(AbiError::new(
                ErrorCode::Unsupported,
                format!("{m} is connection-scoped; use the gateway session"),
            )),
            other => Err(AbiError::new(
                ErrorCode::Unsupported,
                format!("{other} is not implemented in this phase"),
            )),
        }
    }

    async fn wait_event(&self, p: WaitParams) -> Result<Value, AbiError> {
        let pid: Pid = parse_pid(&p.pid)?;
        let mut rx = self.kernel.subscribe();
        let deadline = Duration::from_millis(p.timeout_ms);
        let fut = async {
            loop {
                match rx.recv().await {
                    Ok(e) => {
                        if e.pid.as_ref() != Some(&pid) {
                            continue;
                        }
                        if p.cond.matches(&e.payload) {
                            return Ok(e);
                        }
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                        return Err(AbiError::new(ErrorCode::Internal, "event bus closed"));
                    }
                }
            }
        };
        match tokio::time::timeout(deadline, fut).await {
            Ok(Ok(e)) => Ok(json!({ "event": e })),
            Ok(Err(err)) => Err(err),
            Err(_) => Err(AbiError::new(
                ErrorCode::Timeout,
                format!("no matching event within {}ms", p.timeout_ms),
            )),
        }
    }
}

// ---------- 参数类型 ----------

#[derive(Deserialize)]
struct Empty {}

#[derive(Deserialize)]
struct SpawnParams {
    profile: Option<String>,
}

#[derive(Deserialize)]
struct PidParams {
    pid: String,
}

#[derive(Deserialize)]
struct GotoParams {
    pid: String,
    url: String,
}

#[derive(Deserialize)]
struct SnapshotParams {
    pid: String,
    max_nodes: Option<usize>,
}

#[derive(Deserialize)]
struct RefParams {
    pid: String,
    r#ref: String,
}

#[derive(Deserialize)]
struct TypeParams {
    pid: String,
    r#ref: String,
    text: String,
}

#[derive(Deserialize)]
struct PressParams {
    pid: String,
    keys: String,
}

#[derive(Deserialize)]
struct ScrollParams {
    pid: String,
    r#ref: Option<String>,
    #[serde(default)]
    dx: f64,
    #[serde(default)]
    dy: f64,
}

#[derive(Deserialize)]
struct EvalParams {
    pid: String,
    script: String,
    args: Option<Vec<Value>>,
}

#[derive(Deserialize)]
struct WaitParams {
    pid: String,
    cond: WaitCond,
    timeout_ms: u64,
}

/// evt.wait 条件（P1 最小集）。
#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
enum WaitCond {
    UrlContains(String),
    Lifecycle(crate::ProcState),
}

impl WaitCond {
    fn matches(&self, payload: &BusPayload) -> bool {
        match (self, payload) {
            (WaitCond::UrlContains(sub), BusPayload::Navigated { url }) => {
                url.as_str().contains(sub.as_str())
            }
            (WaitCond::Lifecycle(want), BusPayload::ProcLifecycle { state }) => state == want,
            _ => false,
        }
    }
}

// ---------- 辅助 ----------

fn parse<T: serde::de::DeserializeOwned>(params: Value) -> Result<T, AbiError> {
    // 无参调用允许省略 params（Null 视为空对象）
    let params = if params.is_null() { json!({}) } else { params };
    serde_json::from_value(params)
        .map_err(|e| AbiError::new(ErrorCode::InvalidArg, format!("invalid params: {e}")))
}

fn parse_pid(s: &str) -> Result<Pid, AbiError> {
    s.parse()
        .map_err(|e| AbiError::new(ErrorCode::InvalidArg, format!("{e}")))
}

fn parse_ref(s: &str) -> Result<scootlens_abi::ElementRef, AbiError> {
    s.parse()
        .map_err(|e| AbiError::new(ErrorCode::InvalidArg, format!("{e}")))
}

fn parse_url(s: &str) -> Result<url::Url, AbiError> {
    s.parse()
        .map_err(|e| AbiError::new(ErrorCode::InvalidArg, format!("invalid url: {e}")))
}

fn to_value<T: serde::Serialize>(v: T) -> Value {
    serde_json::to_value(v).unwrap_or(Value::Null)
}

fn method_not_found(id: RpcId, m: &str) -> RpcResponse {
    RpcResponse {
        jsonrpc: scootlens_abi::V2,
        id,
        outcome: RpcOutcome::Failure {
            error: RpcError {
                code: -32601,
                message: format!("method not found: {m}"),
                data: json!({ "code": "METHOD_NOT_FOUND" }),
            },
        },
    }
}
