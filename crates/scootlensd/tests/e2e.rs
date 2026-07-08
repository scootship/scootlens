//! P1 端到端验收（docs/09-roadmap.md 验收门禁）。
//!
//! 全栈：fixtures 站点 + 真实 Chromium + kernel + gateway，客户端只说 WS JSON-RPC。
//! 需要本机 Chromium，全部 `#[ignore]`；e2e 门禁跑：
//!
//! ```text
//! cargo test -p scootlensd --test e2e -- --ignored --test-threads=1
//! ```

use std::sync::Arc;
use std::time::{Duration, Instant};

use futures_util::{SinkExt, StreamExt};
use scootlens_abi::{ApprovalMode, TokenClaims, TokenConstraints};
use scootlens_driver_chromium::ChromiumDriver;
use scootlens_gateway::{Gateway, GatewayConfig};
use scootlens_kernel::{Dispatcher, Kernel, KernelConfig};
use scootlens_test_support::FixtureSite;
use serde_json::{Value, json};
use tokio_tungstenite::tungstenite::Message;

struct Stack {
    ws_url: String,
    site: FixtureSite,
}

async fn start_stack() -> Stack {
    let site = FixtureSite::start_default().await.expect("fixture site");
    let driver = ChromiumDriver::discover().expect("chromium binary");
    let kernel = Kernel::new(Arc::new(driver), KernelConfig::default());
    let mut constraints = TokenConstraints::default();
    constraints.approval.insert("*".into(), ApprovalMode::Auto);
    let token = kernel.security().issue(&TokenClaims {
        subject: "user:e2e".into(),
        scopes: vec!["*".parse().expect("scope")],
        constraints,
        issued_by: "e2e".into(),
        issued_at: 0,
    });
    let gw = Gateway::new(Dispatcher::new(kernel), GatewayConfig::default());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let addr = listener.local_addr().expect("addr");
    tokio::spawn(async move { gw.serve(listener).await });
    Stack {
        ws_url: format!("ws://{addr}/ws?token={token}"),
        site,
    }
}

type Ws =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

async fn connect(stack: &Stack) -> Ws {
    let (ws, _) = tokio_tungstenite::connect_async(&stack.ws_url)
        .await
        .expect("connect");
    ws
}

async fn rpc(ws: &mut Ws, id: u64, method: &str, params: Value) -> Value {
    let req = json!({"jsonrpc":"2.0","id":id,"method":method,"params":params});
    ws.send(Message::Text(req.to_string().into()))
        .await
        .expect("send");
    loop {
        let msg = tokio::time::timeout(Duration::from_secs(30), ws.next())
            .await
            .expect("rpc timeout")
            .expect("stream ended")
            .expect("ws error");
        if let Message::Text(t) = msg {
            let v: Value = serde_json::from_str(&t).expect("json");
            if v["id"] == json!(id) {
                return v;
            }
        }
    }
}

fn ok(resp: &Value) -> &Value {
    assert!(resp.get("error").is_none(), "rpc failed: {}", resp["error"]);
    &resp["result"]
}

fn extract_ref(text: &str, name: &str) -> String {
    let line = text
        .lines()
        .find(|l| l.contains(&format!("\"{name}\"")) && l.contains('['))
        .unwrap_or_else(|| panic!("no interactive line with {name} in:\n{text}"));
    let start = line.rfind('[').expect("[") + 1;
    let end = line.rfind(']').expect("]");
    line[start..end].to_owned()
}

/// 验收门禁 #1：登录表单场景（导航→填表→提交→断言跳转）。
#[tokio::test]
#[ignore = "requires chromium; run in e2e job"]
async fn login_flow_end_to_end() {
    let stack = start_stack().await;
    let mut ws = connect(&stack).await;

    let pid = ok(&rpc(&mut ws, 1, "proc.spawn", json!({})).await)["pid"]
        .as_str()
        .expect("pid")
        .to_owned();

    let nav = rpc(
        &mut ws,
        2,
        "nav.goto",
        json!({"pid": pid, "url": stack.site.url("/login")}),
    )
    .await;
    assert_eq!(ok(&nav)["title"], "Login");

    let snap = rpc(&mut ws, 3, "view.snapshot", json!({"pid": pid})).await;
    let text = ok(&snap)["text"].as_str().expect("text").to_owned();
    let user = extract_ref(&text, "Username");
    let pass = extract_ref(&text, "Password");

    for (id, (r, v)) in [(&user, "alice"), (&pass, "s3cret")].iter().enumerate() {
        let resp = rpc(
            &mut ws,
            10 + id as u64,
            "act.type",
            json!({"pid": pid, "ref": r, "text": v}),
        )
        .await;
        assert_eq!(ok(&resp)["nav_occurred"], false);
    }

    // 重新快照拿提交按钮（代数递增，旧 ref 过期）
    let snap = rpc(&mut ws, 20, "view.snapshot", json!({"pid": pid})).await;
    let text = ok(&snap)["text"].as_str().expect("text").to_owned();
    assert!(text.contains("= \"alice\""), "typed value visible:\n{text}");
    let submit = extract_ref(&text, "Sign in");

    let click = rpc(&mut ws, 21, "act.click", json!({"pid": pid, "ref": submit})).await;
    assert_eq!(ok(&click)["nav_occurred"], true, "submit must navigate");

    let snap = rpc(&mut ws, 22, "view.snapshot", json!({"pid": pid})).await;
    let text = ok(&snap)["text"].as_str().expect("text").to_owned();
    assert!(
        text.contains("heading \"Welcome\""),
        "must land on welcome:\n{text}"
    );

    ok(&rpc(&mut ws, 30, "proc.kill", json!({"pid": pid})).await);
}

/// 验收门禁 #4：kill -9 引擎进程 → Crashed + 事件广播，内核零 panic。
#[tokio::test]
#[ignore = "requires chromium; run in e2e job"]
async fn crash_recovery_end_to_end() {
    let stack = start_stack().await;
    let mut ws = connect(&stack).await;

    let before = chromium_children();
    let pid = ok(&rpc(&mut ws, 1, "proc.spawn", json!({})).await)["pid"]
        .as_str()
        .expect("pid")
        .to_owned();
    ok(&rpc(
        &mut ws,
        2,
        "nav.goto",
        json!({"pid": pid, "url": stack.site.url("/")}),
    )
    .await);

    // 找到新出现的 chromium 主进程
    let after = chromium_children();
    let target: Vec<u32> = after
        .iter()
        .filter(|p| !before.contains(p))
        .copied()
        .collect();
    assert!(!target.is_empty(), "must find spawned chromium process");

    // 并发：先挂 evt.wait(lifecycle=crashed)，再 SIGKILL
    let mut ws2 = connect(&stack).await;
    let pid2 = pid.clone();
    let waiter = tokio::spawn(async move {
        rpc(
            &mut ws2,
            100,
            "evt.wait",
            json!({"pid": pid2, "cond": {"lifecycle": "crashed"}, "timeout_ms": 10000}),
        )
        .await
    });
    tokio::time::sleep(Duration::from_millis(300)).await;
    for p in &target {
        sigkill(*p);
    }

    let evt = waiter.await.expect("join");
    let evt = ok(&evt);
    assert_eq!(evt["event"]["topic"], "proc.lifecycle");
    assert_eq!(evt["event"]["state"], "crashed");

    // 内核状态一致，引擎操作报 E_ENGINE_CRASH
    let info = rpc(&mut ws, 3, "proc.info", json!({"pid": pid})).await;
    assert_eq!(ok(&info)["state"], "crashed");
    let nav = rpc(
        &mut ws,
        4,
        "nav.goto",
        json!({"pid": pid, "url": stack.site.url("/")}),
    )
    .await;
    assert_eq!(nav["error"]["data"]["code"], "E_ENGINE_CRASH");

    // 崩溃进程可被 kill 清理；daemon 仍健康（零 panic）
    ok(&rpc(&mut ws, 5, "proc.kill", json!({"pid": pid})).await);
    let si = rpc(&mut ws, 6, "sys.info", json!({})).await;
    assert_eq!(ok(&si)["engine"], "chromium");
}

/// 验收门禁 #3：性能预算 spawn <1.5s、snapshot <300ms、act <50ms。
#[tokio::test]
#[ignore = "requires chromium; run in e2e job"]
async fn performance_budget() {
    let stack = start_stack().await;
    let mut ws = connect(&stack).await;

    let t0 = Instant::now();
    let pid = ok(&rpc(&mut ws, 1, "proc.spawn", json!({})).await)["pid"]
        .as_str()
        .expect("pid")
        .to_owned();
    let spawn_ms = t0.elapsed().as_millis();

    ok(&rpc(
        &mut ws,
        2,
        "nav.goto",
        json!({"pid": pid, "url": stack.site.url("/login")}),
    )
    .await);

    let t1 = Instant::now();
    let snap = rpc(&mut ws, 3, "view.snapshot", json!({"pid": pid})).await;
    let snapshot_ms = t1.elapsed().as_millis();
    let text = ok(&snap)["text"].as_str().expect("text").to_owned();
    let user = extract_ref(&text, "Username");

    let t2 = Instant::now();
    ok(&rpc(
        &mut ws,
        4,
        "act.type",
        json!({"pid": pid, "ref": user, "text": "x"}),
    )
    .await);
    let act_ms = t2.elapsed().as_millis();

    ok(&rpc(&mut ws, 5, "proc.kill", json!({"pid": pid})).await);

    println!("budget: spawn={spawn_ms}ms snapshot={snapshot_ms}ms act={act_ms}ms");
    // The coverage job re-runs this ignored test under llvm-cov instrumentation
    // (via --include-ignored), which systematically inflates wall-clock timings
    // and makes a hard budget flaky. The budget is enforced for real in the
    // non-instrumented e2e job; under instrumentation we still exercise every
    // RPC (for coverage) but skip the timing assertions.
    if std::env::var_os("LLVM_PROFILE_FILE").is_some() {
        eprintln!("coverage instrumentation detected; perf budget assertions skipped");
        return;
    }
    assert!(spawn_ms < 1500, "spawn {spawn_ms}ms exceeds 1.5s budget");
    assert!(
        snapshot_ms < 300,
        "snapshot {snapshot_ms}ms exceeds 300ms budget"
    );
    assert!(act_ms < 50, "act {act_ms}ms exceeds 50ms budget");
}

/// 当前测试进程的直接子进程里的 chromium 主进程。
fn chromium_children() -> Vec<u32> {
    let my_pid = std::process::id();
    let out = std::process::Command::new("pgrep")
        .args(["-P", &my_pid.to_string()])
        .output();
    let Ok(out) = out else { return Vec::new() };
    String::from_utf8_lossy(&out.stdout)
        .split_whitespace()
        .filter_map(|s| s.parse::<u32>().ok())
        .filter(|p| {
            let cmd = std::process::Command::new("ps")
                .args(["-p", &p.to_string(), "-o", "command="])
                .output();
            match cmd {
                Ok(c) => {
                    let s = String::from_utf8_lossy(&c.stdout).to_lowercase();
                    s.contains("chrom") && s.contains("remote-debugging-port")
                }
                Err(_) => false,
            }
        })
        .collect()
}

/// SIGKILL（模拟引擎崩溃；测试专用）。
fn sigkill(pid: u32) {
    let _ = std::process::Command::new("kill")
        .args(["-9", &pid.to_string()])
        .status();
}
