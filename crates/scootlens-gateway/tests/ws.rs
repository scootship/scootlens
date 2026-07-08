//! gateway WS 集成测试：真实 TCP + WS 握手 + JSON-RPC 往返 + 事件推送。

use std::sync::Arc;

use futures_util::{SinkExt, StreamExt};
use scootlens_driver_mock::MockDriver;
use scootlens_gateway::{Gateway, GatewayConfig};
use scootlens_kernel::{Dispatcher, Kernel, KernelConfig};
use serde_json::{Value, json};
use tokio_tungstenite::tungstenite::Message;

const TOKEN: &str = "test-token-1";

async fn start() -> String {
    let kernel = Kernel::new(
        Arc::new(MockDriver::standard_fixture()),
        KernelConfig::default(),
    );
    let gw = Gateway::new(
        Dispatcher::new(kernel),
        GatewayConfig {
            token: TOKEN.into(),
        },
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let addr = listener.local_addr().expect("addr");
    tokio::spawn(async move { gw.serve(listener).await });
    format!("ws://{addr}/ws")
}

type WsClient = tokio_tungstenite::WebSocketStream<
    tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
>;

async fn connect(base: &str) -> WsClient {
    let (ws, _) = tokio_tungstenite::connect_async(format!("{base}?token={TOKEN}"))
        .await
        .expect("connect");
    ws
}

async fn rpc(ws: &mut WsClient, id: u64, method: &str, params: Value) -> Value {
    let req = json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params});
    ws.send(Message::Text(req.to_string().into()))
        .await
        .expect("send");
    // 跳过通知帧，等待匹配 id 的响应
    loop {
        let msg = recv_json(ws).await;
        if msg["id"] == json!(id) {
            return msg;
        }
    }
}

async fn recv_json(ws: &mut WsClient) -> Value {
    loop {
        let msg = tokio::time::timeout(std::time::Duration::from_secs(3), ws.next())
            .await
            .expect("timeout waiting for frame")
            .expect("stream ended")
            .expect("ws error");
        match msg {
            Message::Text(t) => return serde_json::from_str(&t).expect("json"),
            Message::Ping(_) | Message::Pong(_) => continue,
            other => panic!("unexpected frame: {other:?}"),
        }
    }
}

#[tokio::test]
async fn rejects_bad_token() {
    let base = start().await;
    let err = tokio_tungstenite::connect_async(format!("{base}?token=wrong"))
        .await
        .expect_err("should reject");
    let msg = err.to_string();
    assert!(msg.contains("401"), "expected 401, got: {msg}");
}

#[tokio::test]
async fn rejects_missing_token() {
    let base = start().await;
    assert!(tokio_tungstenite::connect_async(base).await.is_err());
}

#[tokio::test]
async fn rpc_roundtrip_spawn_and_list() {
    let base = start().await;
    let mut ws = connect(&base).await;

    let resp = rpc(&mut ws, 1, "proc.spawn", json!({})).await;
    let pid = resp["result"]["pid"].as_str().expect("pid").to_owned();

    let resp = rpc(&mut ws, 2, "proc.list", json!({})).await;
    assert_eq!(resp["result"]["procs"][0]["pid"], pid.as_str());
}

#[tokio::test]
async fn parse_error_returns_32700() {
    let base = start().await;
    let mut ws = connect(&base).await;
    ws.send(Message::Text("{not json".to_string().into()))
        .await
        .expect("send");
    let msg = recv_json(&mut ws).await;
    assert_eq!(msg["error"]["code"], -32700);
    assert_eq!(msg["id"], Value::Null);
}

#[tokio::test]
async fn subscribe_receives_nav_notifications() {
    let base = start().await;
    let mut ws = connect(&base).await;

    let resp = rpc(&mut ws, 1, "proc.spawn", json!({})).await;
    let pid = resp["result"]["pid"].as_str().expect("pid").to_owned();

    let resp = rpc(&mut ws, 2, "evt.subscribe", json!({"topics": ["nav"]})).await;
    let sub_id = resp["result"]["sub_id"]
        .as_str()
        .expect("sub_id")
        .to_owned();

    rpc(
        &mut ws,
        3,
        "nav.goto",
        json!({"pid": pid, "url": "http://fixture.test/login"}),
    )
    .await;

    // 应收到 evt.event 通知
    let note = recv_notification(&mut ws).await;
    assert_eq!(note["method"], "evt.event");
    assert_eq!(note["params"]["sub_id"], sub_id.as_str());
    assert_eq!(note["params"]["event"]["topic"], "nav");
    assert!(
        note["params"]["event"]["url"]
            .as_str()
            .expect("url")
            .contains("/login")
    );
}

async fn recv_notification(ws: &mut WsClient) -> Value {
    loop {
        let msg = recv_json(ws).await;
        if msg.get("method").is_some() {
            return msg;
        }
    }
}

#[tokio::test]
async fn topic_filter_excludes_other_topics() {
    let base = start().await;
    let mut ws = connect(&base).await;

    // 只订 console：nav 与 proc.lifecycle 都不该推
    rpc(&mut ws, 1, "evt.subscribe", json!({"topics": ["console"]})).await;

    let resp = rpc(&mut ws, 2, "proc.spawn", json!({})).await;
    let pid = resp["result"]["pid"].as_str().expect("pid").to_owned();
    rpc(
        &mut ws,
        3,
        "nav.goto",
        json!({"pid": pid, "url": "http://fixture.test/"}),
    )
    .await;

    // 发一个 sys.info 作为"栅栏"，若前面有通知会先到
    let req = json!({"jsonrpc":"2.0","id":4,"method":"sys.info","params":{}});
    ws.send(Message::Text(req.to_string().into()))
        .await
        .expect("send");
    let msg = recv_json(&mut ws).await;
    assert_eq!(msg["id"], 4, "expected fence response, got: {msg}");
}

#[tokio::test]
async fn unsubscribe_stops_notifications() {
    let base = start().await;
    let mut ws = connect(&base).await;

    let resp = rpc(&mut ws, 1, "proc.spawn", json!({})).await;
    let pid = resp["result"]["pid"].as_str().expect("pid").to_owned();

    let resp = rpc(&mut ws, 2, "evt.subscribe", json!({"topics": ["nav"]})).await;
    let sub_id = resp["result"]["sub_id"]
        .as_str()
        .expect("sub_id")
        .to_owned();

    let resp = rpc(&mut ws, 3, "evt.unsubscribe", json!({"sub_id": sub_id})).await;
    assert_eq!(resp["result"]["ok"], true);

    rpc(
        &mut ws,
        4,
        "nav.goto",
        json!({"pid": pid, "url": "http://fixture.test/login"}),
    )
    .await;

    // 栅栏：sys.info 响应先于任何（不该存在的）通知
    let req = json!({"jsonrpc":"2.0","id":5,"method":"sys.info","params":{}});
    ws.send(Message::Text(req.to_string().into()))
        .await
        .expect("send");
    let msg = recv_json(&mut ws).await;
    assert_eq!(msg["id"], 5, "expected fence response, got: {msg}");
}

#[tokio::test]
async fn unknown_sub_id_errors() {
    let base = start().await;
    let mut ws = connect(&base).await;
    let resp = rpc(&mut ws, 1, "evt.unsubscribe", json!({"sub_id": "sub-999"})).await;
    assert_eq!(resp["error"]["data"]["code"], "E_INVALID_ARG");
}

#[tokio::test]
async fn pid_filter_scopes_subscription() {
    let base = start().await;
    let mut ws = connect(&base).await;

    let resp = rpc(&mut ws, 1, "proc.spawn", json!({})).await;
    let pid_a = resp["result"]["pid"].as_str().expect("pid").to_owned();
    let resp = rpc(&mut ws, 2, "proc.spawn", json!({})).await;
    let pid_b = resp["result"]["pid"].as_str().expect("pid").to_owned();

    // 只订 pid_b 的 nav
    rpc(
        &mut ws,
        3,
        "evt.subscribe",
        json!({"pid": pid_b, "topics": ["nav"]}),
    )
    .await;

    rpc(
        &mut ws,
        4,
        "nav.goto",
        json!({"pid": pid_a, "url": "http://fixture.test/login"}),
    )
    .await;
    rpc(
        &mut ws,
        5,
        "nav.goto",
        json!({"pid": pid_b, "url": "http://fixture.test/welcome"}),
    )
    .await;

    let note = recv_notification(&mut ws).await;
    assert_eq!(note["params"]["event"]["pid"], pid_b.as_str());
    assert!(
        note["params"]["event"]["url"]
            .as_str()
            .expect("url")
            .contains("/welcome")
    );
}

#[tokio::test]
async fn kill_roundtrip_over_ws() {
    let base = start().await;
    let mut ws = connect(&base).await;
    let resp = rpc(&mut ws, 1, "proc.spawn", json!({})).await;
    let pid = resp["result"]["pid"].as_str().expect("pid").to_owned();
    let resp = tokio::time::timeout(
        std::time::Duration::from_secs(3),
        rpc(&mut ws, 2, "proc.kill", json!({"pid": pid})),
    )
    .await
    .expect("kill must not hang");
    assert_eq!(resp["result"]["ok"], true);
}
