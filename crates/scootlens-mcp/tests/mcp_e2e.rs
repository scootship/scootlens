//! MCP e2e（docs/09-roadmap.md P4 门禁 #1）：
//! 一个真实 MCP 客户端（本测试进程，stdio + MCP JSON-RPC 帧）通过
//! `scootlens-mcp` 完成完整任务：initialize → tools/list → spawn → goto →
//! snapshot → act.type，其间 `js.exec`（敏感作用域）挂起人工审批，
//! 管理员经 gateway 批准后调用恢复成功。
//!
//! 拓扑：test(MCP client) ⇄ scootlens-mcp(stdio, agent 令牌) ⇄ gateway ⇄ kernel(mock)。

use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use scootlens_abi::{ApprovalMode, TokenClaims, TokenConstraints, method};
use scootlens_driver_mock::MockDriver;
use scootlens_gateway::{Gateway, GatewayConfig};
use scootlens_kernel::{Dispatcher, Kernel, KernelConfig};
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, ChildStdout, Command};
use tokio_tungstenite::tungstenite::Message;

fn claims(subject: &str, scopes: &[&str], auto: bool) -> TokenClaims {
    let mut constraints = TokenConstraints::default();
    if auto {
        constraints.approval.insert("*".into(), ApprovalMode::Auto);
    }
    TokenClaims {
        subject: subject.into(),
        scopes: scopes.iter().map(|s| s.parse().expect("scope")).collect(),
        constraints,
        issued_by: "test".into(),
        issued_at: 0,
    }
}

/// 起 gateway，返回 (ws base, agent 令牌, admin 令牌)。
async fn start_gateway() -> (String, String, String) {
    let kernel = Kernel::new(
        Arc::new(MockDriver::standard_fixture()),
        KernelConfig::default(),
    );
    // Agent：最小任务作用域；js:exec 为敏感作用域 → 默认人工审批
    let agent = kernel.security().issue(&claims(
        "agent:mcp-e2e",
        &[
            "proc:spawn",
            "proc:list",
            "proc:kill",
            "nav@fixture.test",
            "view@fixture.test",
            "act@fixture.test",
            "js:exec@fixture.test",
        ],
        false,
    ));
    let admin = kernel.security().issue(&claims("user:admin", &["*"], true));
    let gw = Gateway::new(Dispatcher::new(kernel), GatewayConfig::default());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let addr = listener.local_addr().expect("addr");
    tokio::spawn(async move { gw.serve(listener).await });
    (format!("ws://{addr}/ws"), agent, admin)
}

struct McpClient {
    child: Child,
    stdin: Option<ChildStdin>,
    stdout: BufReader<ChildStdout>,
    seq: i64,
}

impl McpClient {
    /// spawn `scootlens-mcp` 二进制（stdio 传输）。
    fn spawn(url: &str, token: &str) -> Self {
        let mut child = Command::new(env!("CARGO_BIN_EXE_scootlens-mcp"))
            .arg("--url")
            .arg(url)
            .arg("--token")
            .arg(token)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .spawn()
            .expect("spawn scootlens-mcp");
        let stdin = child.stdin.take().expect("stdin");
        let stdout = BufReader::new(child.stdout.take().expect("stdout"));
        Self {
            child,
            stdin: Some(stdin),
            stdout,
            seq: 0,
        }
    }

    async fn send(&mut self, frame: Value) {
        let mut text = frame.to_string();
        text.push('\n');
        let stdin = self.stdin.as_mut().expect("stdin open");
        stdin.write_all(text.as_bytes()).await.expect("write");
        stdin.flush().await.expect("flush");
    }

    /// 读帧直到出现指定 id 的响应。
    async fn recv_id(&mut self, id: i64) -> Value {
        loop {
            let mut line = String::new();
            let n = tokio::time::timeout(Duration::from_secs(90), self.stdout.read_line(&mut line))
                .await
                .expect("mcp response timeout")
                .expect("read");
            assert!(n > 0, "mcp server closed stdout");
            let Ok(v) = serde_json::from_str::<Value>(&line) else {
                continue;
            };
            if v.get("id").and_then(Value::as_i64) == Some(id) {
                return v;
            }
        }
    }

    async fn request(&mut self, method: &str, params: Value) -> Value {
        self.seq += 1;
        let id = self.seq;
        self.send(json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params }))
            .await;
        self.recv_id(id).await
    }

    /// tools/call；返回 (is_error, 解析后的首个 text content)。
    async fn call_tool(&mut self, tool: &str, args: Value) -> (bool, Value) {
        let resp = self
            .request("tools/call", json!({ "name": tool, "arguments": args }))
            .await;
        let result = resp
            .get("result")
            .unwrap_or_else(|| panic!("tool {tool} protocol error: {resp}"));
        let is_error = result["isError"].as_bool().unwrap_or(false);
        let text = result["content"][0]["text"].as_str().unwrap_or("null");
        let parsed = serde_json::from_str(text).unwrap_or(Value::Null);
        (is_error, parsed)
    }

    /// MCP 握手（initialize + initialized 通知）。
    async fn handshake(&mut self) -> Value {
        let init = self
            .request(
                "initialize",
                json!({
                    "protocolVersion": "2025-06-18",
                    "capabilities": {},
                    "clientInfo": { "name": "e2e-client", "version": "0.0.0" },
                }),
            )
            .await;
        self.send(json!({ "jsonrpc": "2.0", "method": "notifications/initialized" }))
            .await;
        init
    }

    /// 优雅关停：关闭 stdin（EOF → serve 循环退出），等待进程结束。
    async fn shutdown(mut self) {
        drop(self.stdin.take());
        let waited = tokio::time::timeout(Duration::from_secs(10), self.child.wait()).await;
        if waited.is_err() {
            self.child.kill().await.ok();
        }
    }
}

type WsClient =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

async fn ws_connect(base: &str, token: &str) -> WsClient {
    let (ws, _) = tokio_tungstenite::connect_async(format!("{base}?token={token}"))
        .await
        .expect("ws connect");
    ws
}

async fn ws_rpc(ws: &mut WsClient, id: u64, method: &str, params: Value) -> Value {
    ws.send(Message::Text(
        json!({"jsonrpc":"2.0","id":id,"method":method,"params":params})
            .to_string()
            .into(),
    ))
    .await
    .expect("ws send");
    loop {
        let msg = tokio::time::timeout(Duration::from_secs(10), ws.next())
            .await
            .expect("ws timeout")
            .expect("ws ended")
            .expect("ws error");
        if let Message::Text(t) = msg {
            let v: Value = serde_json::from_str(&t).expect("json");
            if v["id"] == json!(id) {
                return v;
            }
        }
    }
}

/// 从紧凑快照文本提取指定元素 ref。
fn extract_ref(text: &str, name: &str) -> String {
    let line = text
        .lines()
        .find(|l| l.contains(&format!("\"{name}\"")) && l.contains('['))
        .unwrap_or_else(|| panic!("no line with {name} in:\n{text}"));
    let start = line.rfind('[').expect("[") + 1;
    let end = line.rfind(']').expect("]");
    line[start..end].to_owned()
}

#[tokio::test]
async fn mcp_client_completes_task_with_human_approval() {
    let (base, agent_token, admin_token) = start_gateway().await;
    let mut mcp = McpClient::spawn(&base, &agent_token);

    // ---- MCP 握手 ----
    let init = mcp.handshake().await;
    assert_eq!(init["result"]["serverInfo"]["name"], "scootlens-mcp");
    assert!(
        init["result"]["capabilities"]["tools"].is_object(),
        "server must declare tools capability: {init}"
    );

    // ---- 工具清单 = ABI 投影 ----
    let list = mcp.request("tools/list", json!({})).await;
    let tools = list["result"]["tools"].as_array().expect("tools");
    assert_eq!(
        tools.len(),
        method::ALL.len() - 2,
        "projection covers the syscall table minus connection-scoped methods"
    );
    assert!(tools.iter().any(|t| t["name"] == "scootlens_view_snapshot"));

    // ---- 完整任务：spawn → goto → snapshot → type ----
    let (err, spawn) = mcp.call_tool("scootlens_proc_spawn", json!({})).await;
    assert!(!err, "spawn: {spawn}");
    let pid = spawn["pid"].as_str().expect("pid").to_owned();

    let (err, _) = mcp
        .call_tool(
            "scootlens_nav_goto",
            json!({ "pid": pid, "url": "http://fixture.test/login" }),
        )
        .await;
    assert!(!err);

    let (err, snap) = mcp
        .call_tool("scootlens_view_snapshot", json!({ "pid": pid }))
        .await;
    assert!(!err);
    let user_ref = extract_ref(snap["text"].as_str().expect("snapshot text"), "Username");

    let (err, _) = mcp
        .call_tool(
            "scootlens_act_type",
            json!({ "pid": pid, "ref": user_ref, "text": "alice" }),
        )
        .await;
    assert!(!err);

    // ---- 越权路径：未持有 obs:replay → 内核拒绝（工具级错误，附 ABI 码） ----
    let (err, denied) = mcp
        .call_tool("scootlens_obs_replay_export", json!({ "pid": pid }))
        .await;
    assert!(err, "unauthorized call must surface as tool error");
    assert_eq!(denied["error"]["abi_code"], "E_CAP_DENIED");

    // ---- 人工审批闭环：js.exec 经 MCP 发起 → 挂起 → 管理员批准 → 恢复成功 ----
    let js_call = {
        let (url, token, pid) = (base.clone(), agent_token.clone(), pid.clone());
        tokio::spawn(async move {
            // 第二个 MCP 客户端会话发起敏感调用（挂起期间不阻塞第一个会话）
            let mut mcp2 = McpClient::spawn(&url, &token);
            mcp2.handshake().await;
            let out = mcp2
                .call_tool("scootlens_js_exec", json!({ "pid": pid, "script": "6*7" }))
                .await;
            mcp2.child.kill().await.ok();
            out
        })
    };

    // 管理员：轮询收件箱 → 批准
    let mut admin = ws_connect(&base, &admin_token).await;
    let mut approval_id = None;
    for i in 0..300u64 {
        let resp = ws_rpc(&mut admin, 100 + i, "cap.pending", json!({})).await;
        if let Some(first) = resp["result"]["pending"].as_array().and_then(|v| v.first()) {
            assert_eq!(first["subject"], "agent:mcp-e2e");
            approval_id = Some(first["id"].as_str().expect("approval id").to_owned());
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let approval_id = approval_id.expect("approval must become pending");
    let approved = ws_rpc(
        &mut admin,
        999,
        "cap.approve",
        json!({ "approval_id": approval_id, "decision": "allow" }),
    )
    .await;
    assert_eq!(approved["result"]["ok"], true);

    // 挂起的 MCP 工具调用恢复执行并成功（mock 引擎对未编程脚本返回 null；
    // 本断言聚焦审批闭环：调用必须成功返回而非 E_APPROVAL_PENDING/E_CAP_DENIED）
    let (err, value) = js_call.await.expect("join");
    assert!(!err, "approved js.exec resumes: {value}");
    assert!(
        value.as_object().is_some_and(|o| o.contains_key("value")),
        "js.exec result carries value field: {value}"
    );

    // ---- 收尾：经 MCP kill ----
    let (err, _) = mcp
        .call_tool("scootlens_proc_kill", json!({ "pid": pid }))
        .await;
    assert!(!err);

    mcp.shutdown().await;
}

#[tokio::test]
async fn mcp_rejects_unknown_tool_as_protocol_error() {
    let (base, agent_token, _admin) = start_gateway().await;
    let mut mcp = McpClient::spawn(&base, &agent_token);
    mcp.handshake().await;
    let resp = mcp
        .request(
            "tools/call",
            json!({ "name": "scootlens_bogus", "arguments": {} }),
        )
        .await;
    assert!(
        resp.get("error").is_some(),
        "unknown tool must be a protocol error: {resp}"
    );
    mcp.shutdown().await;
}
