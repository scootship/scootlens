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
//! - 非法 JSON → `-32700`；合法 JSON 但非法请求 → `-32600`

mod conn;

use std::path::PathBuf;
use std::sync::Arc;

use axum::Router;
use axum::extract::{Query, State, WebSocketUpgrade};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use scootlens_kernel::{Caller, Dispatcher};
use serde::Deserialize;

/// 网关配置。
#[derive(Debug, Clone, Default)]
pub struct GatewayConfig {
    /// Web Console 静态目录（`None` = 不托管）。
    pub console_dir: Option<PathBuf>,
}

/// WS JSON-RPC 网关。
pub struct Gateway {
    state: AppState,
    config: GatewayConfig,
}

#[derive(Clone)]
struct AppState {
    dispatcher: Dispatcher,
}

impl Gateway {
    pub fn new(dispatcher: Dispatcher, config: GatewayConfig) -> Self {
        Self {
            state: AppState { dispatcher },
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
    ws.on_upgrade(move |socket| conn::run(socket, state.dispatcher, caller))
}
