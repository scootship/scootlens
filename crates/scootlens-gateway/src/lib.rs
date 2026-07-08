//! # scootlens-gateway
//!
//! WS JSON-RPC 网关（docs/06-security-model.md、docs/03-abi-spec.md）。P1 范围：
//!
//! - `GET /ws?token=<t>`：单一全权令牌握手（正式 capability 模型在 P2）
//! - 每帧一个 `RpcRequest` → [`scootlens_kernel::Dispatcher`] → 一帧 `RpcResponse`
//! - 请求并发处理（`evt.wait` 等慢调用不阻塞同连接的后续请求）
//! - `evt.subscribe` / `evt.unsubscribe` 是**连接级**语义：订阅表挂在连接上，
//!   命中的总线事件以 `evt.event` server notification 推送
//! - 非法 JSON → `-32700`；合法 JSON 但非法请求 → `-32600`

mod conn;

use std::sync::Arc;

use axum::Router;
use axum::extract::{Query, State, WebSocketUpgrade};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use scootlens_kernel::Dispatcher;
use serde::Deserialize;

/// 网关配置。
#[derive(Debug, Clone)]
pub struct GatewayConfig {
    /// 单一全权令牌（P1 骨架；P2 换 capability token）。
    pub token: String,
}

/// WS JSON-RPC 网关。
pub struct Gateway {
    state: AppState,
}

#[derive(Clone)]
struct AppState {
    dispatcher: Dispatcher,
    config: Arc<GatewayConfig>,
}

impl Gateway {
    pub fn new(dispatcher: Dispatcher, config: GatewayConfig) -> Self {
        Self {
            state: AppState {
                dispatcher,
                config: Arc::new(config),
            },
        }
    }

    /// 构建 axum Router（测试/组装用）。
    pub fn router(&self) -> Router {
        Router::new()
            .route("/ws", get(ws_handler))
            .with_state(self.state.clone())
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
    if q.token.as_deref() != Some(state.config.token.as_str()) {
        return (StatusCode::UNAUTHORIZED, "invalid or missing token").into_response();
    }
    ws.on_upgrade(move |socket| conn::run(socket, state.dispatcher))
}
