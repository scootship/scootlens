//! gateway 认证集成测试：/auth/* 路由 + 会话 cookie 的 WS 握手。
//!
//! 全部走真实 TCP，验证**拒绝路径**（无凭据/错误凭据 → 401）与
//! 登录成功后的 cookie 会话闭环。

use std::sync::Arc;

use futures_util::{SinkExt, StreamExt};
use scootlens_driver_mock::MockDriver;
use scootlens_gateway::{AuthConfig, Gateway, GatewayConfig, PasswordConfig};
use scootlens_kernel::{Dispatcher, Kernel, KernelConfig};
use serde_json::{Value, json};
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;

/// 起一个带密码认证的 gateway，返回 http base（如 `http://127.0.0.1:PORT`）。
async fn start(auth: AuthConfig) -> String {
    let kernel = Kernel::new(
        Arc::new(MockDriver::standard_fixture()),
        KernelConfig::default(),
    );
    let gw = Gateway::new(
        Dispatcher::new(kernel),
        GatewayConfig {
            auth,
            ..GatewayConfig::default()
        },
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let addr = listener.local_addr().expect("addr");
    tokio::spawn(async move { gw.serve(listener).await });
    format!("http://{addr}")
}

fn password_auth() -> AuthConfig {
    AuthConfig {
        password: Some(PasswordConfig::from_plain("admin", "hunter2")),
        ..AuthConfig::default()
    }
}

async fn http(
    method: &str,
    url: &str,
    body: Option<Value>,
    cookie: Option<&str>,
) -> (u16, Vec<(String, String)>, Value) {
    let client = reqwest_lite();
    let mut req = match method {
        "POST" => client.post(url),
        _ => client.get(url),
    };
    if let Some(b) = body {
        req = req.json(&b);
    }
    if let Some(c) = cookie {
        req = req.header("cookie", c);
    }
    let res = req.send().await.expect("http");
    let status = res.status().as_u16();
    let headers = res
        .headers()
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_str().unwrap_or("").to_owned()))
        .collect();
    let text = res.text().await.unwrap_or_default();
    let json = serde_json::from_str(&text).unwrap_or(Value::String(text));
    (status, headers, json)
}

/// 测试内不复用连接池（每次新建），避免测试间串扰。
fn reqwest_lite() -> reqwest::Client {
    reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .expect("client")
}

fn set_cookie_value(headers: &[(String, String)]) -> &str {
    headers
        .iter()
        .find(|(k, _)| k == "set-cookie")
        .map(|(_, v)| v.as_str())
        .expect("set-cookie header")
}

/// `sl_session=<sid>` 段。
fn cookie_pair(set_cookie: &str) -> String {
    set_cookie
        .split(';')
        .next()
        .expect("cookie pair")
        .to_owned()
}

#[tokio::test]
async fn providers_reflect_config() {
    let base = start(password_auth()).await;
    let (status, _, body) = http("GET", &format!("{base}/auth/providers"), None, None).await;
    assert_eq!(status, 200);
    assert_eq!(body["password"], json!(true));
    assert_eq!(body["microsoft"], json!(false));

    let none = start(AuthConfig::default()).await;
    let (_, _, body) = http("GET", &format!("{none}/auth/providers"), None, None).await;
    assert_eq!(body["password"], json!(false));
    assert_eq!(body["microsoft"], json!(false));
}

#[tokio::test]
async fn login_rejects_bad_credentials() {
    let base = start(password_auth()).await;
    let (status, _, _) = http(
        "POST",
        &format!("{base}/auth/login"),
        Some(json!({"username": "admin", "password": "wrong"})),
        None,
    )
    .await;
    assert_eq!(status, 401);
}

#[tokio::test]
async fn login_disabled_when_not_configured() {
    let base = start(AuthConfig::default()).await;
    let (status, _, _) = http(
        "POST",
        &format!("{base}/auth/login"),
        Some(json!({"username": "admin", "password": "x"})),
        None,
    )
    .await;
    assert_eq!(status, 501);
}

#[tokio::test]
async fn login_me_logout_roundtrip() {
    let base = start(password_auth()).await;

    // 登录 → HttpOnly cookie
    let (status, headers, body) = http(
        "POST",
        &format!("{base}/auth/login"),
        Some(json!({"username": "admin", "password": "hunter2"})),
        None,
    )
    .await;
    assert_eq!(status, 200);
    assert_eq!(body["subject"], json!("user:admin"));
    let set_cookie = set_cookie_value(&headers);
    assert!(set_cookie.contains("HttpOnly"), "cookie must be HttpOnly");
    assert!(set_cookie.contains("SameSite=Strict"));
    let cookie = cookie_pair(set_cookie);

    // /auth/me 带 cookie → subject
    let (status, _, body) = http("GET", &format!("{base}/auth/me"), None, Some(&cookie)).await;
    assert_eq!(status, 200);
    assert_eq!(body["subject"], json!("user:admin"));

    // 无 cookie → 401
    let (status, _, _) = http("GET", &format!("{base}/auth/me"), None, None).await;
    assert_eq!(status, 401);

    // 注销 → 会话销毁
    let (status, _, _) = http("POST", &format!("{base}/auth/logout"), None, Some(&cookie)).await;
    assert_eq!(status, 200);
    let (status, _, _) = http("GET", &format!("{base}/auth/me"), None, Some(&cookie)).await;
    assert_eq!(status, 401, "session must be destroyed after logout");
}

#[tokio::test]
async fn ws_accepts_session_cookie_and_rejects_garbage() {
    let base = start(password_auth()).await;
    let (_, headers, _) = http(
        "POST",
        &format!("{base}/auth/login"),
        Some(json!({"username": "admin", "password": "hunter2"})),
        None,
    )
    .await;
    let cookie = cookie_pair(set_cookie_value(&headers));
    let ws_url = format!("{}/ws", base.replace("http://", "ws://"));

    // cookie 会话 → 握手成功，且 cap.list 返回登录主体
    let mut req = ws_url.as_str().into_client_request().expect("req");
    req.headers_mut()
        .insert("cookie", cookie.parse().expect("hv"));
    let (mut ws, _) = tokio_tungstenite::connect_async(req).await.expect("ws");
    ws.send(Message::Text(
        json!({"jsonrpc": "2.0", "id": 1, "method": "cap.list"})
            .to_string()
            .into(),
    ))
    .await
    .expect("send");
    let reply = loop {
        let msg = tokio::time::timeout(std::time::Duration::from_secs(3), ws.next())
            .await
            .expect("timeout")
            .expect("stream")
            .expect("frame");
        if let Message::Text(t) = msg {
            let v: Value = serde_json::from_str(&t).expect("json");
            if v["id"] == json!(1) {
                break v;
            }
        }
    };
    assert_eq!(reply["result"]["subject"], json!("user:admin"));

    // 伪造 cookie → 401
    let mut bad = ws_url.as_str().into_client_request().expect("req");
    bad.headers_mut()
        .insert("cookie", "sl_session=forged".parse().expect("hv"));
    let err = tokio_tungstenite::connect_async(bad)
        .await
        .expect_err("forged session must be rejected");
    assert!(err.to_string().contains("401"), "got: {err}");

    // 无凭据 → 401
    let err = tokio_tungstenite::connect_async(ws_url.as_str())
        .await
        .expect_err("credential-less handshake must be rejected");
    assert!(err.to_string().contains("401"), "got: {err}");
}

#[tokio::test]
async fn ms_login_redirects_or_501() {
    // 未配置 → 501
    let base = start(password_auth()).await;
    let (status, _, _) = http("GET", &format!("{base}/auth/ms/login"), None, None).await;
    assert_eq!(status, 501);

    // 配置后 → 302 到 login.microsoftonline.com，且带 state
    let ms = scootlens_gateway::MicrosoftConfig {
        client_id: "cid".into(),
        tenant: "organizations".into(),
        redirect_uri: "http://127.0.0.1:9910/auth/ms/callback".into(),
        client_secret: "s".into(),
        allowed_emails: vec!["a@b.c".into()],
        allowed_domains: vec![],
    };
    let base = start(AuthConfig {
        microsoft: Some(ms),
        ..AuthConfig::default()
    })
    .await;
    let (status, headers, _) = http("GET", &format!("{base}/auth/ms/login"), None, None).await;
    assert_eq!(status, 307);
    let loc = headers
        .iter()
        .find(|(k, _)| k == "location")
        .map(|(_, v)| v.as_str())
        .expect("location");
    assert!(loc.starts_with("https://login.microsoftonline.com/organizations/"));
    assert!(loc.contains("state="));

    // 回调 state 不匹配 → 定向回登录页并带错误码，绝不建会话
    let (status, headers, _) = http(
        "GET",
        &format!("{base}/auth/ms/callback?code=x&state=forged"),
        None,
        None,
    )
    .await;
    assert_eq!(status, 303);
    let loc = headers
        .iter()
        .find(|(k, _)| k == "location")
        .map(|(_, v)| v.as_str())
        .expect("location");
    assert!(loc.contains("login_error=state_mismatch"), "got: {loc}");
}
