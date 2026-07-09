//! # scootlens-gateway
//!
//! WS JSON-RPC 网关（docs/06-security-model.md、docs/03-abi-spec.md）。P2 范围：
//!
//! - `GET /ws?token=<slt1...>`：capability 令牌握手（ed25519 验签 → [`Caller`]）；
//!   验签失败/缺失 → 401，绝不落到匿名身份；配置认证后额外接受会话 cookie
//! - `/auth/*`：Console 登录（用户名密码 / Microsoft Entra ID → 会话 cookie），
//!   见 [`auth`] 模块 —— 人类用户不再需要把令牌贴进 URL
//! - 每帧一个 `RpcRequest` → [`scootlens_kernel::Dispatcher`] → 一帧 `RpcResponse`
//! - 请求并发处理（`evt.wait`、审批挂起等慢调用不阻塞同连接的后续请求）
//! - `evt.subscribe` / `evt.unsubscribe` 是**连接级**语义：订阅表挂在连接上，
//!   命中的总线事件以 `evt.event` server notification 推送
//! - `console_dir` 配置后在 `/` 托管 Web Console 静态文件
//! - 服务端 WS 保活：周期 Ping + 入站空闲超时回收（反代/NAT 静默断链防护）
//! - 非法 JSON → `-32700`；合法 JSON 但非法请求 → `-32600`

pub mod auth;
mod conn;

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use axum::Router;
use axum::extract::{Query, State, WebSocketUpgrade};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Redirect, Response};
use axum::routing::{get, post};
use scootlens_kernel::{Caller, Dispatcher};
use serde::Deserialize;
use serde_json::json;

pub use auth::{AuthConfig, MicrosoftConfig, PasswordConfig, parse_sha256_hex};

/// 网关配置。
#[derive(Debug, Clone)]
pub struct GatewayConfig {
    /// Web Console 静态目录（`None` = 不托管）。
    pub console_dir: Option<PathBuf>,
    /// 服务端 WS Ping 间隔（保活反代/NAT 链路）。
    pub ws_ping_interval: Duration,
    /// 入站空闲超时：超过该时长无任何入站帧（含 Pong）即关闭连接。
    pub ws_idle_timeout: Duration,
    /// Console 登录认证（用户名密码 / Microsoft Entra ID）。
    pub auth: AuthConfig,
}

impl Default for GatewayConfig {
    fn default() -> Self {
        Self {
            console_dir: None,
            ws_ping_interval: Duration::from_secs(15),
            ws_idle_timeout: Duration::from_secs(45),
            auth: AuthConfig::default(),
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
    auth: Arc<auth::AuthState>,
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
                auth: Arc::new(auth::AuthState::new(config.auth.clone())),
            },
            config,
        }
    }

    /// 构建 axum Router（测试/组装用）。
    pub fn router(&self) -> Router {
        let mut router = Router::new()
            .route("/ws", get(ws_handler))
            .route("/auth/providers", get(auth_providers))
            .route("/auth/login", post(auth_login))
            .route("/auth/logout", post(auth_logout))
            .route("/auth/me", get(auth_me))
            .route("/auth/ms/login", get(ms_login))
            .route("/auth/ms/callback", get(ms_callback));
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
    headers: HeaderMap,
    State(state): State<AppState>,
) -> Response {
    // 认证优先级：capability 令牌（Agent / 自动化）→ 会话 cookie（Console 登录）。
    // 两者皆缺 / 皆无效 → 401，绝不落到匿名身份。
    let claims = if let Some(token) = q.token {
        match state.dispatcher.kernel().security().verify(&token) {
            Ok(c) => c,
            Err(_) => return (StatusCode::UNAUTHORIZED, "invalid token").into_response(),
        }
    } else if let Some(claims) =
        auth::session_cookie(&headers).and_then(|sid| state.auth.session_claims(&sid))
    {
        claims
    } else {
        return (StatusCode::UNAUTHORIZED, "missing token or session").into_response();
    };
    let caller = Arc::new(Caller::from_claims(claims));
    let keepalive = state.keepalive;
    ws.on_upgrade(move |socket| conn::run(socket, state.dispatcher, caller, keepalive))
}

// ---------- /auth/* 路由（Console 登录；docs/06-security-model.md） ----------

async fn auth_providers(State(state): State<AppState>) -> Response {
    let cfg = &state.auth.config;
    axum::Json(json!({
        "password": cfg.password.is_some(),
        "microsoft": cfg.microsoft.is_some(),
    }))
    .into_response()
}

#[derive(Deserialize)]
struct LoginBody {
    username: String,
    password: String,
}

async fn auth_login(
    State(state): State<AppState>,
    axum::Json(body): axum::Json<LoginBody>,
) -> Response {
    if state.auth.config.password.is_none() {
        return (StatusCode::NOT_IMPLEMENTED, "password login not configured").into_response();
    }
    match state.auth.login_password(&body.username, &body.password) {
        Some(sid) => {
            let subject = format!("user:{}", body.username);
            (
                [(
                    header::SET_COOKIE,
                    auth::set_cookie(&sid, state.auth.config.cookie_secure, auth::SESSION_TTL),
                )],
                axum::Json(json!({ "subject": subject })),
            )
                .into_response()
        }
        None => {
            // 失败方不区分用户名/密码；固定小延迟钝化在线穷举
            tokio::time::sleep(Duration::from_millis(400)).await;
            (StatusCode::UNAUTHORIZED, "invalid credentials").into_response()
        }
    }
}

async fn auth_logout(State(state): State<AppState>, headers: HeaderMap) -> Response {
    if let Some(sid) = auth::session_cookie(&headers) {
        state.auth.destroy_session(&sid);
    }
    (
        [(
            header::SET_COOKIE,
            auth::clear_cookie(state.auth.config.cookie_secure),
        )],
        axum::Json(json!({ "ok": true })),
    )
        .into_response()
}

async fn auth_me(State(state): State<AppState>, headers: HeaderMap) -> Response {
    match auth::session_cookie(&headers).and_then(|sid| state.auth.session_claims(&sid)) {
        Some(claims) => axum::Json(json!({ "subject": claims.subject })).into_response(),
        None => (StatusCode::UNAUTHORIZED, "no session").into_response(),
    }
}

async fn ms_login(State(state): State<AppState>) -> Response {
    let Some(ms) = &state.auth.config.microsoft else {
        return (
            StatusCode::NOT_IMPLEMENTED,
            "microsoft login not configured",
        )
            .into_response();
    };
    let url = auth::authorize_url(ms, &state.auth.create_login_state());
    Redirect::temporary(&url).into_response()
}

#[derive(Deserialize)]
struct MsCallbackQuery {
    code: Option<String>,
    state: Option<String>,
    error: Option<String>,
}

/// OAuth 回调：state 单次校验 → code 换用户 → 白名单判定 → 会话 cookie → 回 `/`。
/// 所有失败路径统一 303 到 `/?login_error=…`（Console 登录页展示）。
async fn ms_callback(State(state): State<AppState>, Query(q): Query<MsCallbackQuery>) -> Response {
    fn fail(reason: &str) -> Response {
        Redirect::to(&format!("/?login_error={}", urlencode(reason))).into_response()
    }
    let Some(ms) = state.auth.config.microsoft.clone() else {
        return (
            StatusCode::NOT_IMPLEMENTED,
            "microsoft login not configured",
        )
            .into_response();
    };
    if let Some(err) = q.error {
        tracing::warn!(%err, "microsoft login returned error");
        return fail("provider_error");
    }
    if !q
        .state
        .as_deref()
        .is_some_and(|s| state.auth.consume_login_state(s))
    {
        return fail("state_mismatch");
    }
    let Some(code) = q.code else {
        return fail("missing_code");
    };
    let user = match auth::exchange_code_for_user(&ms, &code).await {
        Ok(u) => u,
        Err(e) => {
            tracing::warn!(error = %e, "microsoft code exchange failed");
            return fail("exchange_failed");
        }
    };
    let email = auth::login_email(&user);
    if !auth::is_allowed_email(&ms, &email) {
        tracing::warn!(%email, "microsoft login rejected: not in allowlist");
        return fail("not_allowed");
    }
    let sid = state.auth.create_session(format!("user:{email}"));
    (
        [(
            header::SET_COOKIE,
            auth::set_cookie(&sid, state.auth.config.cookie_secure, auth::SESSION_TTL),
        )],
        Redirect::to("/"),
    )
        .into_response()
}

fn urlencode(s: &str) -> String {
    url::form_urlencoded::byte_serialize(s.as_bytes()).collect()
}
