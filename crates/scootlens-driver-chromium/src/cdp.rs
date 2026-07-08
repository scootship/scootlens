//! 薄 CDP 客户端：单 WS 连接上的请求/响应关联 + 事件广播（flatten sessions）。

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use scootlens_abi::{AbiError, ErrorCode};
use serde_json::{Value, json};
use tokio::sync::{broadcast, mpsc, oneshot};
use tokio_tungstenite::tungstenite::Message;

/// CDP 事件（`method` + `params`，携带来源 session）。
#[derive(Debug, Clone)]
pub(crate) struct CdpEvent {
    pub session_id: Option<String>,
    pub method: String,
    pub params: Value,
}

type Pending = Arc<Mutex<HashMap<u64, oneshot::Sender<Result<Value, AbiError>>>>>;

/// 一条 browser 级 CDP 连接（session 命令走同一 WS，`sessionId` 字段区分）。
pub(crate) struct CdpConn {
    next_id: AtomicU64,
    pending: Pending,
    out: mpsc::Sender<String>,
    events: broadcast::Sender<CdpEvent>,
    worker: tokio::task::JoinHandle<()>,
}

impl CdpConn {
    pub async fn connect(ws_url: &str) -> Result<Self, AbiError> {
        let (ws, _) = tokio::time::timeout(
            Duration::from_secs(10),
            tokio_tungstenite::connect_async(ws_url),
        )
        .await
        .map_err(|_| AbiError::new(ErrorCode::Timeout, "CDP connect timeout"))?
        .map_err(|e| AbiError::new(ErrorCode::Internal, format!("CDP connect: {e}")))?;

        let (mut sink, mut stream) = ws.split();
        let (out_tx, mut out_rx) = mpsc::channel::<String>(64);
        let (evt_tx, _) = broadcast::channel::<CdpEvent>(1024);
        let pending: Pending = Arc::default();

        let pending_r = Arc::clone(&pending);
        let evt_tx_r = evt_tx.clone();
        let worker = tokio::spawn(async move {
            loop {
                tokio::select! {
                    Some(frame) = out_rx.recv() => {
                        if sink.send(Message::Text(frame.into())).await.is_err() {
                            break;
                        }
                    }
                    msg = stream.next() => {
                        let Some(Ok(msg)) = msg else { break };
                        let Message::Text(text) = msg else { continue };
                        let Ok(v) = serde_json::from_str::<Value>(&text) else { continue };
                        route_incoming(v, &pending_r, &evt_tx_r);
                    }
                    else => break,
                }
            }
            // 连接断开：所有挂起请求立即失败
            let mut map = pending_r.lock().unwrap_or_else(PoisonError::into_inner);
            for (_, tx) in map.drain() {
                let _ = tx.send(Err(AbiError::new(
                    ErrorCode::EngineCrash,
                    "CDP connection closed",
                )));
            }
        });

        Ok(Self {
            next_id: AtomicU64::new(1),
            pending,
            out: out_tx,
            events: evt_tx,
            worker,
        })
    }

    /// 发送命令并等待响应。`session_id` 为 None 时是 browser 级命令。
    pub async fn call(
        &self,
        session_id: Option<&str>,
        method: &str,
        params: Value,
    ) -> Result<Value, AbiError> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let mut msg = json!({ "id": id, "method": method, "params": params });
        if let Some(sid) = session_id {
            msg["sessionId"] = json!(sid);
        }
        let (tx, rx) = oneshot::channel();
        self.pending
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(id, tx);

        self.out
            .send(msg.to_string())
            .await
            .map_err(|_| AbiError::new(ErrorCode::EngineCrash, "CDP connection closed"))?;

        let resp = tokio::time::timeout(Duration::from_secs(10), rx)
            .await
            .map_err(|_| {
                self.pending
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .remove(&id);
                AbiError::new(ErrorCode::Timeout, format!("CDP call timeout: {method}"))
            })?
            .map_err(|_| AbiError::new(ErrorCode::EngineCrash, "CDP connection closed"))?;
        resp
    }

    pub fn subscribe(&self) -> broadcast::Receiver<CdpEvent> {
        self.events.subscribe()
    }
}

impl Drop for CdpConn {
    fn drop(&mut self) {
        self.worker.abort();
    }
}

fn route_incoming(v: Value, pending: &Pending, events: &broadcast::Sender<CdpEvent>) {
    if let Some(id) = v.get("id").and_then(Value::as_u64) {
        let sender = pending
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(&id);
        if let Some(tx) = sender {
            let outcome = if let Some(err) = v.get("error") {
                Err(AbiError::new(
                    ErrorCode::Internal,
                    format!(
                        "CDP error {}: {}",
                        err["code"],
                        err["message"].as_str().unwrap_or("?")
                    ),
                ))
            } else {
                Ok(v.get("result").cloned().unwrap_or(Value::Null))
            };
            let _ = tx.send(outcome);
        }
        return;
    }
    if let Some(method) = v.get("method").and_then(Value::as_str) {
        let _ = events.send(CdpEvent {
            session_id: v
                .get("sessionId")
                .and_then(Value::as_str)
                .map(str::to_owned),
            method: method.to_owned(),
            params: v.get("params").cloned().unwrap_or(Value::Null),
        });
    }
}
