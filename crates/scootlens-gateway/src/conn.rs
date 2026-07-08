//! 单条 WS 连接的生命周期：读循环 + 写通道 + 连接级订阅表。

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

use axum::extract::ws::{Message, WebSocket};
use futures_util::stream::{SplitSink, SplitStream};
use futures_util::{SinkExt, StreamExt};
use scootlens_abi::{AbiError, ErrorCode, Pid, RpcNotification, RpcRequest, RpcResponse, method};
use scootlens_kernel::{BusEvent, Dispatcher};
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::sync::mpsc;

/// 连接级订阅。
struct Subscription {
    pid: Option<Pid>,
    /// 空 = 全部主题。
    topics: Vec<String>,
}

impl Subscription {
    fn matches(&self, e: &BusEvent) -> bool {
        if let Some(want) = &self.pid
            && e.pid.as_ref() != Some(want)
        {
            return false;
        }
        self.topics.is_empty() || self.topics.iter().any(|t| t == e.payload.topic())
    }
}

#[derive(Default)]
struct SubTable {
    subs: HashMap<String, Subscription>,
}

type SharedSubs = Arc<Mutex<SubTable>>;

fn lock(subs: &SharedSubs) -> std::sync::MutexGuard<'_, SubTable> {
    subs.lock().unwrap_or_else(PoisonError::into_inner)
}

/// 连接主循环。socket 关闭或出错即退出，所有派生任务随之终止。
pub(crate) async fn run(socket: WebSocket, dispatcher: Dispatcher) {
    let (sink, stream) = socket.split();
    let (tx, rx) = mpsc::channel::<String>(64);
    let subs: SharedSubs = Arc::default();
    let sub_seq = Arc::new(AtomicU64::new(0));

    let writer = tokio::spawn(write_loop(sink, rx));
    let pusher = tokio::spawn(push_loop(
        dispatcher.kernel().subscribe(),
        Arc::clone(&subs),
        tx.clone(),
    ));

    read_loop(stream, dispatcher, subs, sub_seq, tx).await;

    pusher.abort();
    writer.abort();
}

async fn write_loop(mut sink: SplitSink<WebSocket, Message>, mut rx: mpsc::Receiver<String>) {
    while let Some(text) = rx.recv().await {
        if sink.send(Message::Text(text.into())).await.is_err() {
            break;
        }
    }
}

/// 总线事件 → 命中订阅 → `evt.event` 通知帧。
async fn push_loop(
    mut bus: tokio::sync::broadcast::Receiver<BusEvent>,
    subs: SharedSubs,
    tx: mpsc::Sender<String>,
) {
    loop {
        let event = match bus.recv().await {
            Ok(e) => e,
            Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                tracing::warn!(skipped = n, "ws push lagged; events dropped");
                continue;
            }
            Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
        };
        let hits: Vec<String> = {
            let table = lock(&subs);
            table
                .subs
                .iter()
                .filter(|(_, s)| s.matches(&event))
                .map(|(id, _)| id.clone())
                .collect()
        };
        for sub_id in hits {
            let note =
                RpcNotification::new("evt.event", json!({ "sub_id": sub_id, "event": event }));
            let frame = match serde_json::to_string(&note) {
                Ok(f) => f,
                Err(_) => continue,
            };
            if tx.send(frame).await.is_err() {
                return;
            }
        }
    }
}

async fn read_loop(
    mut stream: SplitStream<WebSocket>,
    dispatcher: Dispatcher,
    subs: SharedSubs,
    sub_seq: Arc<AtomicU64>,
    tx: mpsc::Sender<String>,
) {
    while let Some(msg) = stream.next().await {
        let text = match msg {
            Ok(Message::Text(t)) => t,
            Ok(Message::Close(_)) | Err(_) => break,
            Ok(_) => continue, // Ping/Pong/Binary 忽略（axum 自动回 Pong）
        };
        let raw: Value = match serde_json::from_str(&text) {
            Ok(v) => v,
            Err(e) => {
                send_protocol_error(&tx, Value::Null, -32700, &format!("parse error: {e}")).await;
                continue;
            }
        };
        let req: RpcRequest = match serde_json::from_value(raw.clone()) {
            Ok(r) => r,
            Err(e) => {
                let id = raw.get("id").cloned().unwrap_or(Value::Null);
                send_protocol_error(&tx, id, -32600, &format!("invalid request: {e}")).await;
                continue;
            }
        };
        match req.method.as_str() {
            // 连接级方法：直接在读循环处理（快路径，无阻塞）
            method::EVT_SUBSCRIBE => {
                let resp = handle_subscribe(req, &subs, &sub_seq);
                send_response(&tx, &resp).await;
            }
            method::EVT_UNSUBSCRIBE => {
                let resp = handle_unsubscribe(req, &subs);
                send_response(&tx, &resp).await;
            }
            // 其余全部并发分发（evt.wait 等慢调用不能阻塞连接）
            _ => {
                let d = dispatcher.clone();
                let tx = tx.clone();
                tokio::spawn(async move {
                    let resp = d.dispatch(req).await;
                    send_response(&tx, &resp).await;
                });
            }
        }
    }
}

#[derive(Deserialize)]
struct SubscribeParams {
    pid: Option<String>,
    #[serde(default)]
    topics: Vec<String>,
}

fn handle_subscribe(req: RpcRequest, subs: &SharedSubs, seq: &AtomicU64) -> RpcResponse {
    let id = req.id.clone();
    let p: SubscribeParams = match serde_json::from_value(req.params) {
        Ok(p) => p,
        Err(e) => {
            return RpcResponse::failure(
                id,
                AbiError::new(ErrorCode::InvalidArg, format!("invalid params: {e}")),
            );
        }
    };
    let pid = match p.pid {
        Some(s) => match s.parse::<Pid>() {
            Ok(pid) => Some(pid),
            Err(e) => {
                return RpcResponse::failure(
                    id,
                    AbiError::new(ErrorCode::InvalidArg, format!("{e}")),
                );
            }
        },
        None => None,
    };
    let sub_id = format!("sub-{}", seq.fetch_add(1, Ordering::Relaxed) + 1);
    lock(subs).subs.insert(
        sub_id.clone(),
        Subscription {
            pid,
            topics: p.topics,
        },
    );
    RpcResponse::success(id, json!({ "sub_id": sub_id }))
}

#[derive(Deserialize)]
struct UnsubscribeParams {
    sub_id: String,
}

fn handle_unsubscribe(req: RpcRequest, subs: &SharedSubs) -> RpcResponse {
    let id = req.id.clone();
    let p: UnsubscribeParams = match serde_json::from_value(req.params) {
        Ok(p) => p,
        Err(e) => {
            return RpcResponse::failure(
                id,
                AbiError::new(ErrorCode::InvalidArg, format!("invalid params: {e}")),
            );
        }
    };
    if lock(subs).subs.remove(&p.sub_id).is_none() {
        return RpcResponse::failure(
            id,
            AbiError::new(
                ErrorCode::InvalidArg,
                format!("unknown sub_id: {}", p.sub_id),
            ),
        );
    }
    RpcResponse::success(id, json!({ "ok": true }))
}

async fn send_response(tx: &mpsc::Sender<String>, resp: &RpcResponse) {
    if let Ok(frame) = serde_json::to_string(resp) {
        let _ = tx.send(frame).await;
    }
}

/// JSON-RPC 协议层错误（-32700/-32600）：id 可能为 null，手工构帧。
async fn send_protocol_error(tx: &mpsc::Sender<String>, id: Value, code: i64, message: &str) {
    let frame = json!({
        "jsonrpc": "2.0",
        "id": id,
        "error": { "code": code, "message": message },
    });
    let _ = tx.send(frame.to_string()).await;
}
