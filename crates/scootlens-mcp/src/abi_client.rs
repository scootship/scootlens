//! WS JSON-RPC ABI 客户端（tokio-tungstenite）。
//!
//! MCP 工具调用 → 一帧 `RpcRequest` → gateway → 一帧 `RpcResponse`。
//! 支持并发调用（pending 表按 id 配对）；`evt.event` 通知帧忽略
//! （MCP 投影无订阅语义，条件等待走 `evt.wait`）。

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

use futures_util::{SinkExt, StreamExt};
use scootlens_abi::{RpcId, RpcRequest};
use serde_json::Value;
use tokio::sync::{mpsc, oneshot};
use tokio_tungstenite::tungstenite::Message;

/// ABI 调用错误。
#[derive(Debug, Clone)]
pub enum CallError {
    /// 内核返回 JSON-RPC error（`data.code` 为 ABI 错误码，如 `E_CAP_DENIED`）。
    Rpc {
        code: i64,
        message: String,
        data: Value,
    },
    /// 连接层故障（gateway 不可达/连接中断）。
    Transport(String),
}

impl CallError {
    /// ABI 错误码（`E_*`），传输错误返回 None。
    pub fn abi_code(&self) -> Option<&str> {
        match self {
            CallError::Rpc { data, .. } => data.get("code").and_then(Value::as_str),
            CallError::Transport(_) => None,
        }
    }
}

impl std::fmt::Display for CallError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CallError::Rpc { code, message, .. } => match self.abi_code() {
                Some(abi) => write!(f, "{abi}: {message}"),
                None => write!(f, "rpc {code}: {message}"),
            },
            CallError::Transport(e) => write!(f, "transport: {e}"),
        }
    }
}

impl std::error::Error for CallError {}

type Pending = Arc<Mutex<HashMap<i64, oneshot::Sender<Result<Value, CallError>>>>>;

fn lock(
    p: &Pending,
) -> std::sync::MutexGuard<'_, HashMap<i64, oneshot::Sender<Result<Value, CallError>>>> {
    p.lock().unwrap_or_else(PoisonError::into_inner)
}

/// ABI 客户端。廉价 Clone。
#[derive(Clone)]
pub struct AbiClient {
    tx: mpsc::Sender<String>,
    pending: Pending,
    seq: Arc<AtomicU64>,
}

impl AbiClient {
    /// 连接 gateway（`url` 须含 `?token=<slt1…>` 握手参数）。
    pub async fn connect(url: &str) -> Result<Self, CallError> {
        let (ws, _) = tokio_tungstenite::connect_async(url)
            .await
            .map_err(|e| CallError::Transport(format!("connect {e}")))?;
        let (mut sink, mut stream) = ws.split();
        let (tx, mut rx) = mpsc::channel::<String>(64);
        let pending: Pending = Arc::default();

        tokio::spawn(async move {
            while let Some(text) = rx.recv().await {
                if sink.send(Message::Text(text.into())).await.is_err() {
                    break;
                }
            }
        });

        let reader_pending = Arc::clone(&pending);
        tokio::spawn(async move {
            while let Some(msg) = stream.next().await {
                let text = match msg {
                    Ok(Message::Text(t)) => t,
                    Ok(Message::Close(_)) | Err(_) => break,
                    Ok(_) => continue,
                };
                let Ok(frame) = serde_json::from_str::<Value>(&text) else {
                    continue;
                };
                // 通知帧（evt.event）无 id 配对，忽略
                let Some(id) = frame.get("id").and_then(Value::as_i64) else {
                    continue;
                };
                let Some(waiter) = lock(&reader_pending).remove(&id) else {
                    continue;
                };
                let outcome = if let Some(err) = frame.get("error") {
                    Err(CallError::Rpc {
                        code: err.get("code").and_then(Value::as_i64).unwrap_or(0),
                        message: err
                            .get("message")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_owned(),
                        data: err.get("data").cloned().unwrap_or(Value::Null),
                    })
                } else {
                    Ok(frame.get("result").cloned().unwrap_or(Value::Null))
                };
                let _ = waiter.send(outcome);
            }
            // 连接终止：让所有在途调用立即失败
            for (_, waiter) in lock(&reader_pending).drain() {
                let _ = waiter.send(Err(CallError::Transport("connection closed".into())));
            }
        });

        Ok(Self {
            tx,
            pending,
            seq: Arc::new(AtomicU64::new(0)),
        })
    }

    /// 一次 ABI 调用。审批挂起等慢路径由内核语义决定（本层不加超时）。
    pub async fn call(&self, method: &str, params: Value) -> Result<Value, CallError> {
        let id = i64::try_from(self.seq.fetch_add(1, Ordering::Relaxed) + 1).unwrap_or(i64::MAX);
        let req = RpcRequest::new(RpcId::Num(id), method, params);
        let frame =
            serde_json::to_string(&req).map_err(|e| CallError::Transport(format!("encode {e}")))?;
        let (done_tx, done_rx) = oneshot::channel();
        lock(&self.pending).insert(id, done_tx);
        if self.tx.send(frame).await.is_err() {
            lock(&self.pending).remove(&id);
            return Err(CallError::Transport("connection closed".into()));
        }
        done_rx
            .await
            .map_err(|_| CallError::Transport("connection closed".into()))?
    }
}
