//! # scootlens-gateway
//!
//! WS JSON-RPC 网关（docs/06-security-model.md、docs/03-abi-spec.md）。P2 范围：
//!
//! - `GET /ws?token=<slt1...>`：capability 令牌握手（ed25519 验签 → [`Caller`]）；
//!   验签失败/缺失 → 401，绝不落到匿名身份
//! - 每帧一个 `RpcRequest` → [`scootlens_kernel::Dispatcher`] → 一帧 `RpcResponse`
//! - 请求并发处理（`evt.wait`、审批挂起等慢调用不阻塞同连接的后续请求）
//! - `evt.subscribe` / `evt.unsubscribe` 是**连接级**语义：订阅表挂在连接上，
//!   命中的总线事件以 `evt.event` server notification 推送
//! - `console_dir` 配置后在 `/` 托管 Web Console 静态文件
//! - 服务端 WS 保活：周期 Ping + 入站空闲超时回收（反代/NAT 静默断链防护）
//! - 非法 JSON → `-32700`；合法 JSON 但非法请求 → `-32600`

mod conn;

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use axum::Router;
use axum::extract::{Query, State, WebSocketUpgrade};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use scootlens_kernel::{Caller, Dispatcher};
use serde::Deserialize;

/// 网关配置。
#[derive(Debug, Clone)]
pub struct GatewayConfig {
    /// Web Console 静态目录（`None` = 不托管）。
    pub console_dir: Option<PathBuf>,
    /// 服务端 WS Ping 间隔（保活反代/NAT 链路）。
    pub ws_ping_interval: Duration,
    /// 入站空闲超时：超过该时长无任何入站帧（含 Pong）即关闭连接。
    pub ws_idle_timeout: Duration,
}

impl Default for GatewayConfig {
    fn default() -> Self {
        Self {
            console_dir: None,
            ws_ping_interval: Duration::from_secs(15),
            ws_idle_timeout: Duration::from_secs(45),
        }
    }
}

/// WS JSON-RPC 网关。
pub struct Gateway {
    state: AppState,
    config: GatewayConfig,
}

#[derive(Clone)]
struct AppState {
    dispatcher: Dispatcher,
    keepalive: conn::Keepalive,
}

impl Gateway {
    pub fn new(dispatcher: Dispatcher, config: GatewayConfig) -> Self {
        Self {
            state: AppState {
                dispatcher,
                keepalive: conn::Keepalive {
                    ping_interval: config.ws_ping_interval,
                    idle_timeout: config.ws_idle_timeout,
                },
            },
            config,
        }
    }

    /// 构建 axum Router（测试/组装用）。
    pub fn router(&self) -> Router {
        let mut router = Router::new().route("/ws", get(ws_handler));
        if let Some(dir) = &self.config.console_dir {
            router = router.fallback_service(
                tower_http::services::ServeDir::new(dir).append_index_html_on_directories(true),
            );
        }
        router.with_state(self.state.clone())
    }

    /// 在给定 listener 上服务，直到进程退出。
    pub async fn serve(self, listener: tokio::net::TcpListener) -> std::io::Result<()> {
        axum::serve(listener, self.router()).await
    }
}

#[derive(Deserialize)]
struct WsQuery {
    token: Option<String>,
}

async fn ws_handler(
    ws: WebSocketUpgrade,
    Query(q): Query<WsQuery>,
    State(state): State<AppState>,
) -> Response {
    let Some(token) = q.token else {
        return (StatusCode::UNAUTHORIZED, "missing token").into_response();
    };
    let claims = match state.dispatcher.kernel().security().verify(&token) {
        Ok(c) => c,
        Err(_) => return (StatusCode::UNAUTHORIZED, "invalid token").into_response(),
    };
    let caller = Arc::new(Caller::from_claims(claims));
    let keepalive = state.keepalive;
    ws.on_upgrade(move |socket| conn::run(socket, state.dispatcher, caller, keepalive))
}
