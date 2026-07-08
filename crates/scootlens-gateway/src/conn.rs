//! 单条 WS 连接的生命周期：读循环 + 写通道 + 连接级订阅表 + 保活。

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use axum::extract::ws::{Message, WebSocket};
use futures_util::stream::{SplitSink, SplitStream};
use futures_util::{SinkExt, StreamExt};
use scootlens_abi::{AbiError, ErrorCode, Pid, RpcNotification, RpcRequest, RpcResponse, method};
use scootlens_kernel::{BusEvent, BusReceiver, Caller, Dispatcher};
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::sync::mpsc;
use tokio::time::Instant;

/// WS 保活参数（反代/NAT 会静默回收空闲长连，必须两端探活）。
#[derive(Debug, Clone, Copy)]
pub(crate) struct Keepalive {
    /// 服务端 Ping 间隔。
    pub ping_interval: Duration,
    /// 超过该时长无任何入站帧（含 Pong）即判定连接死亡并关闭。
    pub idle_timeout: Duration,
}

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

/// 连接主循环。socket 关闭、出错或保活超时即退出，所有派生任务随之终止。
pub(crate) async fn run(
    socket: WebSocket,
    dispatcher: Dispatcher,
    caller: Arc<Caller>,
    keepalive: Keepalive,
) {
    let (sink, stream) = socket.split();
    let (tx, rx) = mpsc::channel::<Message>(64);
    let subs: SharedSubs = Arc::default();
    let sub_seq = Arc::new(AtomicU64::new(0));
    let last_rx = Arc::new(Mutex::new(Instant::now()));

    let writer = tokio::spawn(write_loop(sink, rx));
    let pusher = tokio::spawn(push_loop(
        dispatcher.kernel().subscribe(),
        Arc::clone(&subs),
        tx.clone(),
    ));

    tokio::select! {
        () = read_loop(stream, dispatcher, caller, subs, sub_seq, tx.clone(), Arc::clone(&last_rx)) => {}
        () = keepalive_loop(tx, last_rx, keepalive) => {
            tracing::debug!("ws connection idle beyond timeout; closing");
        }
    }

    pusher.abort();
    writer.abort();
}

async fn write_loop(mut sink: SplitSink<WebSocket, Message>, mut rx: mpsc::Receiver<Message>) {
    while let Some(msg) = rx.recv().await {
        if sink.send(msg).await.is_err() {
            break;
        }
    }
}

/// 周期发送 WS Ping；入站帧长期缺席（对端死亡/链路半开）时返回，触发连接关闭。
async fn keepalive_loop(tx: mpsc::Sender<Message>, last_rx: Arc<Mutex<Instant>>, ka: Keepalive) {
    let mut tick = tokio::time::interval(ka.ping_interval);
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        tick.tick().await;
        let idle = last_rx
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .elapsed();
        if idle >= ka.idle_timeout {
            return;
        }
        if tx.send(Message::Ping(Vec::new().into())).await.is_err() {
            return;
        }
    }
}

/// 总线事件 → 命中订阅 → `evt.event` 通知帧。
async fn push_loop(mut bus: BusReceiver, subs: SharedSubs, tx: mpsc::Sender<Message>) {
    loop {
        let event = match bus.recv().await {
            Ok(e) => e,
            // 关键主题永不丢；高频主题的背压丢弃计数在 event.dropped 内随帧下发
            Err(_) => break,
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
            if tx.send(Message::Text(frame.into())).await.is_err() {
                return;
            }
        }
    }
}

async fn read_loop(
    mut stream: SplitStream<WebSocket>,
    dispatcher: Dispatcher,
    caller: Arc<Caller>,
    subs: SharedSubs,
    sub_seq: Arc<AtomicU64>,
    tx: mpsc::Sender<Message>,
    last_rx: Arc<Mutex<Instant>>,
) {
    while let Some(msg) = stream.next().await {
        *last_rx.lock().unwrap_or_else(PoisonError::into_inner) = Instant::now();
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
            // 其余全部并发分发（evt.wait、审批挂起等慢调用不能阻塞连接）
            _ => {
                let d = dispatcher.clone();
                let tx = tx.clone();
                let caller = Arc::clone(&caller);
                tokio::spawn(async move {
                    let resp = d.dispatch(&caller, req).await;
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

async fn send_response(tx: &mpsc::Sender<Message>, resp: &RpcResponse) {
    if let Ok(frame) = serde_json::to_string(resp) {
        let _ = tx.send(Message::Text(frame.into())).await;
    }
}

/// JSON-RPC 协议层错误（-32700/-32600）：id 可能为 null，手工构帧。
async fn send_protocol_error(tx: &mpsc::Sender<Message>, id: Value, code: i64, message: &str) {
    let frame = json!({
        "jsonrpc": "2.0",
        "id": id,
        "error": { "code": code, "message": message },
    });
    let _ = tx.send(Message::Text(frame.to_string().into())).await;
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 保活循环：正常期间周期发 Ping；入站静默超过 idle_timeout 即返回。
    #[tokio::test]
    async fn keepalive_pings_then_returns_on_idle() {
        let (tx, mut rx) = mpsc::channel::<Message>(32);
        let last_rx = Arc::new(Mutex::new(Instant::now()));
        let ka = Keepalive {
            ping_interval: Duration::from_millis(20),
            idle_timeout: Duration::from_millis(100),
        };
        let started = Instant::now();
        keepalive_loop(tx, last_rx, ka).await;
        assert!(
            started.elapsed() >= Duration::from_millis(100),
            "returned before idle timeout"
        );
        let mut pings = 0;
        while let Ok(msg) = rx.try_recv() {
            if matches!(msg, Message::Ping(_)) {
                pings += 1;
            }
        }
        assert!(pings >= 2, "expected periodic pings, got {pings}");
    }

    /// 入站帧持续刷新 last_rx 时保活循环不退出。
    #[tokio::test]
    async fn keepalive_stays_alive_while_traffic_flows() {
        let (tx, mut rx) = mpsc::channel::<Message>(64);
        let last_rx = Arc::new(Mutex::new(Instant::now()));
        let ka = Keepalive {
            ping_interval: Duration::from_millis(10),
            idle_timeout: Duration::from_millis(60),
        };
        let refresher = {
            let last_rx = Arc::clone(&last_rx);
            tokio::spawn(async move {
                loop {
                    tokio::time::sleep(Duration::from_millis(20)).await;
                    *last_rx.lock().unwrap_or_else(PoisonError::into_inner) = Instant::now();
                }
            })
        };
        let outcome =
            tokio::time::timeout(Duration::from_millis(300), keepalive_loop(tx, last_rx, ka)).await;
        refresher.abort();
        assert!(outcome.is_err(), "keepalive exited despite live traffic");
        assert!(rx.try_recv().is_ok(), "expected pings while alive");
    }
}
