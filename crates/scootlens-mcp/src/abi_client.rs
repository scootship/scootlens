//! WS JSON-RPC ABI 客户端（tokio-tungstenite）。
//!
//! MCP 工具调用 → 一帧 `RpcRequest` → gateway → 一帧 `RpcResponse`。
//! 支持并发调用（pending 表按 id 配对）；`evt.event` 通知帧忽略
//! （MCP 投影无订阅语义，条件等待走 `evt.wait`）。
//!
//! 链路健康（远程部署经反代/NAT 时长连会被静默回收）：
//! - 周期 WS Ping；入站空闲超过阈值判定连接死亡
//! - 连接死亡时在途调用立即以 `Transport` 失败（绝不无限挂起）
//! - 下一次调用自动重连（MCP 会话长命，凭一次网络抖动永久报废不可接受）

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use scootlens_abi::{RpcId, RpcRequest};
use serde_json::Value;
use tokio::sync::{mpsc, oneshot};
use tokio::time::Instant;
use tokio_tungstenite::tungstenite::Message;

const PING_INTERVAL: Duration = Duration::from_secs(15);
const IDLE_TIMEOUT: Duration = Duration::from_secs(45);

/// 客户端保活参数。
#[derive(Debug, Clone, Copy)]
pub struct KeepaliveConfig {
    /// WS Ping 间隔。
    pub ping_interval: Duration,
    /// 入站空闲超时：超过即判定链路死亡。
    pub idle_timeout: Duration,
}

impl Default for KeepaliveConfig {
    fn default() -> Self {
        Self {
            ping_interval: PING_INTERVAL,
            idle_timeout: IDLE_TIMEOUT,
        }
    }
}

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

/// ABI 客户端。廉价 Clone。连接死亡后按需自动重连。
#[derive(Clone)]
pub struct AbiClient {
    url: Arc<str>,
    seq: Arc<AtomicU64>,
    keepalive: KeepaliveConfig,
    /// 当前活跃链路（None = 待重连）。
    link: Arc<tokio::sync::Mutex<Option<Link>>>,
}

/// 一条活跃 WS 链路。
#[derive(Clone)]
struct Link {
    tx: mpsc::Sender<String>,
    pending: Pending,
    /// 判死标志：置位后一切新调用快速失败（与 pending 排空原子配合防挂起竞态）。
    dead: Arc<AtomicBool>,
}

impl Link {
    async fn open(url: &str, ka: KeepaliveConfig) -> Result<Self, CallError> {
        let (ws, _) = tokio_tungstenite::connect_async(url)
            .await
            .map_err(|e| CallError::Transport(format!("connect {e}")))?;
        let (mut sink, mut stream) = ws.split();
        let (tx, mut rx) = mpsc::channel::<String>(64);
        let pending: Pending = Arc::default();
        let dead = Arc::new(AtomicBool::new(false));
        let last_rx = Arc::new(Mutex::new(Instant::now()));

        let reader_pending = Arc::clone(&pending);
        let reader_dead = Arc::clone(&dead);
        let reader_last_rx = Arc::clone(&last_rx);
        let reader = tokio::spawn(async move {
            while let Some(msg) = stream.next().await {
                *reader_last_rx
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner) = Instant::now();
                let text = match msg {
                    Ok(Message::Text(t)) => t,
                    Ok(Message::Close(_)) | Err(_) => break,
                    Ok(_) => continue, // Ping/Pong/Binary：tungstenite 自动回 Pong
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
            fail_all(&reader_dead, &reader_pending);
        });

        // 写循环：出站帧 + 周期 Ping + 空闲判死
        let writer_pending = Arc::clone(&pending);
        let writer_dead = Arc::clone(&dead);
        tokio::spawn(async move {
            let mut tick = tokio::time::interval(ka.ping_interval);
            tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            loop {
                tokio::select! {
                    frame = rx.recv() => {
                        let Some(text) = frame else { break };
                        if sink.send(Message::Text(text.into())).await.is_err() {
                            break;
                        }
                    }
                    _ = tick.tick() => {
                        let idle = last_rx
                            .lock()
                            .unwrap_or_else(PoisonError::into_inner)
                            .elapsed();
                        if idle >= ka.idle_timeout {
                            // 链路半开：入站长期静默，判死
                            break;
                        }
                        if sink.send(Message::Ping(Vec::new().into())).await.is_err() {
                            break;
                        }
                    }
                }
            }
            // 半开链路上 reader 可能永远等不到 EOF，直接中止
            reader.abort();
            let _ = sink.close().await;
            fail_all(&writer_dead, &writer_pending);
        });

        Ok(Self { tx, pending, dead })
    }

    async fn call(&self, id: i64, frame: String) -> Result<Value, CallError> {
        let (done_tx, done_rx) = oneshot::channel();
        lock(&self.pending).insert(id, done_tx);
        // 判死后（或恰在排空后）插入的 waiter 由本检查兜底，杜绝永久挂起
        if self.tx.send(frame).await.is_err() || self.dead.load(Ordering::SeqCst) {
            lock(&self.pending).remove(&id);
            return Err(CallError::Transport("connection closed".into()));
        }
        done_rx
            .await
            .map_err(|_| CallError::Transport("connection closed".into()))?
    }
}

/// 判死 + 排空在途调用。先置位再排空：与 [`Link::call`] 的后置检查构成防挂起屏障。
fn fail_all(dead: &AtomicBool, pending: &Pending) {
    dead.store(true, Ordering::SeqCst);
    for (_, waiter) in lock(pending).drain() {
        let _ = waiter.send(Err(CallError::Transport("connection closed".into())));
    }
}

impl AbiClient {
    /// 连接 gateway（`url` 须含 `?token=<slt1…>` 握手参数）。
    pub async fn connect(url: &str) -> Result<Self, CallError> {
        Self::connect_with_keepalive(url, KeepaliveConfig::default()).await
    }

    /// 指定保活参数连接（部署链路的空闲回收阈值各异；测试亦用）。
    pub async fn connect_with_keepalive(
        url: &str,
        keepalive: KeepaliveConfig,
    ) -> Result<Self, CallError> {
        let link = Link::open(url, keepalive).await?;
        Ok(Self {
            url: Arc::from(url),
            seq: Arc::new(AtomicU64::new(0)),
            keepalive,
            link: Arc::new(tokio::sync::Mutex::new(Some(link))),
        })
    }

    /// 取活跃链路；已判死则重连一次。
    async fn link(&self) -> Result<Link, CallError> {
        let mut guard = self.link.lock().await;
        if let Some(link) = guard.as_ref()
            && !link.tx.is_closed()
            && !link.dead.load(Ordering::SeqCst)
        {
            return Ok(link.clone());
        }
        *guard = None;
        let link = Link::open(&self.url, self.keepalive).await?;
        *guard = Some(link.clone());
        tracing::info!("abi client reconnected to gateway");
        Ok(link)
    }

    /// 一次 ABI 调用。审批挂起等慢路径由内核语义决定（本层不加应用超时，
    /// 链路死亡由保活机制判定并快速失败）。
    pub async fn call(&self, method: &str, params: Value) -> Result<Value, CallError> {
        let id = i64::try_from(self.seq.fetch_add(1, Ordering::Relaxed) + 1).unwrap_or(i64::MAX);
        let req = RpcRequest::new(RpcId::Num(id), method, params);
        let frame =
            serde_json::to_string(&req).map_err(|e| CallError::Transport(format!("encode {e}")))?;
        let link = self.link().await?;
        link.call(id, frame).await
    }
}
