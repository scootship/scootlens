//! syscall 分发层测试（TDD）：RpcRequest → Dispatcher → RpcResponse。

use std::sync::Arc;

use scootlens_abi::{RpcId, RpcOutcome, RpcRequest, RpcResponse};
use scootlens_driver_mock::MockDriver;
use scootlens_kernel::{Dispatcher, Kernel, KernelConfig};
use serde_json::{Value, json};

fn dispatcher() -> Dispatcher {
    Dispatcher::new(Kernel::new(
        Arc::new(MockDriver::standard_fixture()),
        KernelConfig::default(),
    ))
}

async fn call(d: &Dispatcher, method: &str, params: Value) -> RpcResponse {
    d.dispatch(RpcRequest::new(RpcId::Num(1), method, params))
        .await
}

fn result(resp: &RpcResponse) -> &Value {
    match &resp.outcome {
        RpcOutcome::Success { result } => result,
        RpcOutcome::Failure { error } => panic!("expected success, got error: {error:?}"),
    }
}

fn error_code(resp: &RpcResponse) -> (i64, String) {
    match &resp.outcome {
        RpcOutcome::Failure { error } => (
            error.code,
            error.data["code"].as_str().unwrap_or_default().to_owned(),
        ),
        RpcOutcome::Success { result } => panic!("expected error, got success: {result:?}"),
    }
}

// ---------- 完整闭环 ----------

#[tokio::test]
async fn full_login_flow_via_rpc() {
    let d = dispatcher();

    // spawn
    let resp = call(&d, "proc.spawn", json!({})).await;
    let pid = result(&resp)["pid"].as_str().expect("pid").to_owned();

    // goto /login
    let resp = call(
        &d,
        "nav.goto",
        json!({"pid": pid, "url": "http://fixture.test/login"}),
    )
    .await;
    assert_eq!(result(&resp)["title"], "Login");

    // snapshot：紧凑文本含 ref
    let resp = call(&d, "view.snapshot", json!({"pid": pid})).await;
    let snap = result(&resp);
    let text = snap["text"].as_str().expect("text");
    assert!(text.contains("textbox \"Username\""));
    let generation = snap["generation"].as_u64().expect("generation");
    assert_eq!(generation, 1);

    // 从文本抽 ref（格式 [s1eN]）
    let user_ref = extract_ref(text, "Username");
    let pass_ref = extract_ref(text, "Password");

    // 填表
    for (r, v) in [(&user_ref, "alice"), (&pass_ref, "s3cret")] {
        let resp = call(
            &d,
            "act.type",
            json!({"pid": pid, "ref": r, "text": v}),
        )
        .await;
        assert_eq!(result(&resp)["nav_occurred"], false);
    }

    // 重新快照拿按钮 ref（代数递增）
    let resp = call(&d, "view.snapshot", json!({"pid": pid})).await;
    let text = result(&resp)["text"].as_str().expect("text").to_owned();
    let submit = extract_ref(&text, "Sign in");

    // 点击提交 → 导航
    let resp = call(&d, "act.click", json!({"pid": pid, "ref": submit})).await;
    assert_eq!(result(&resp)["nav_occurred"], true);

    // 断言到达 welcome
    let resp = call(&d, "nav.reload", json!({"pid": pid})).await;
    assert_eq!(result(&resp)["title"], "Welcome");

    // kill
    let resp = call(&d, "proc.kill", json!({"pid": pid})).await;
    assert_eq!(result(&resp)["ok"], true);
}

fn extract_ref(text: &str, name: &str) -> String {
    let line = text
        .lines()
        .find(|l| l.contains(&format!("\"{name}\"")))
        .unwrap_or_else(|| panic!("line with {name}"));
    let start = line.rfind('[').expect("ref bracket") + 1;
    let end = line.rfind(']').expect("ref bracket end");
    line[start..end].to_owned()
}

// ---------- 方法路由 ----------

#[tokio::test]
async fn unknown_method_returns_method_not_found() {
    let d = dispatcher();
    let resp = call(&d, "bogus.method", json!({})).await;
    let (code, _) = error_code(&resp);
    assert_eq!(code, -32601);
}

#[tokio::test]
async fn known_but_unimplemented_returns_unsupported() {
    let d = dispatcher();
    // state.read 属 P2
    let resp = call(&d, "state.read", json!({"path": "proc://x/cookies"})).await;
    let (_, code) = error_code(&resp);
    assert_eq!(code, "E_UNSUPPORTED");
}

#[tokio::test]
async fn subscribe_is_gateway_scoped() {
    let d = dispatcher();
    let resp = call(&d, "evt.subscribe", json!({"topics": ["nav"]})).await;
    let (_, code) = error_code(&resp);
    assert_eq!(code, "E_UNSUPPORTED");
}

// ---------- 参数校验 ----------

#[tokio::test]
async fn missing_params_return_invalid_arg() {
    let d = dispatcher();
    let resp = call(&d, "nav.goto", json!({"pid": "p-x"})).await; // 缺 url
    let (code, scode) = error_code(&resp);
    assert_eq!(code, -32602);
    assert_eq!(scode, "E_INVALID_ARG");
}

#[tokio::test]
async fn malformed_pid_returns_invalid_arg() {
    let d = dispatcher();
    let resp = call(&d, "proc.info", json!({"pid": "NOT-A-PID"})).await;
    let (_, scode) = error_code(&resp);
    assert_eq!(scode, "E_INVALID_ARG");
}

#[tokio::test]
async fn unknown_pid_maps_to_proc_not_found() {
    let d = dispatcher();
    let resp = call(&d, "proc.info", json!({"pid": "p-ghost"})).await;
    let (code, scode) = error_code(&resp);
    assert_eq!(code, -32003);
    assert_eq!(scode, "E_PROC_NOT_FOUND");
}

// ---------- proc.list / sys.info / view.screenshot ----------

#[tokio::test]
async fn proc_list_and_sys_info() {
    let d = dispatcher();
    let resp = call(&d, "proc.spawn", json!({"profile": "work"})).await;
    let pid = result(&resp)["pid"].as_str().expect("pid").to_owned();

    let resp = call(&d, "proc.list", json!({})).await;
    let procs = result(&resp)["procs"].as_array().expect("procs");
    assert_eq!(procs.len(), 1);
    assert_eq!(procs[0]["pid"], pid.as_str());
    assert_eq!(procs[0]["profile"], "work");
    assert_eq!(procs[0]["state"], "running");

    let resp = call(&d, "sys.info", json!({})).await;
    let si = result(&resp);
    assert_eq!(si["engine"], "mock");
    assert!(si["abi_version"].is_string());
}

#[tokio::test]
async fn screenshot_returns_base64() {
    let d = dispatcher();
    let resp = call(&d, "proc.spawn", json!({})).await;
    let pid = result(&resp)["pid"].as_str().expect("pid").to_owned();
    call(&d, "nav.goto", json!({"pid": pid, "url": "http://fixture.test/"})).await;

    let resp = call(&d, "view.screenshot", json!({"pid": pid})).await;
    let shot = result(&resp);
    assert_eq!(shot["format"], "png");
    // mock 返回 PNG 魔数前 4 字节
    assert_eq!(shot["data_base64"], "iVBORw==");
}

// ---------- evt.wait ----------

#[tokio::test]
async fn evt_wait_matches_navigation() {
    let d = Arc::new(dispatcher());
    let resp = call(&d, "proc.spawn", json!({})).await;
    let pid = result(&resp)["pid"].as_str().expect("pid").to_owned();

    // 并发：先挂起 wait，再触发导航
    let d2 = Arc::clone(&d);
    let pid2 = pid.clone();
    let waiter = tokio::spawn(async move {
        call(
            &d2,
            "evt.wait",
            json!({"pid": pid2, "cond": {"url_contains": "/welcome"}, "timeout_ms": 2000}),
        )
        .await
    });
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    call(
        &d,
        "nav.goto",
        json!({"pid": pid, "url": "http://fixture.test/welcome"}),
    )
    .await;

    let resp = waiter.await.expect("join");
    let evt = result(&resp);
    assert_eq!(evt["event"]["topic"], "nav");
    assert!(
        evt["event"]["url"]
            .as_str()
            .expect("url")
            .contains("/welcome")
    );
}

#[tokio::test]
async fn evt_wait_times_out() {
    let d = dispatcher();
    let resp = call(&d, "proc.spawn", json!({})).await;
    let pid = result(&resp)["pid"].as_str().expect("pid").to_owned();

    let resp = call(
        &d,
        "evt.wait",
        json!({"pid": pid, "cond": {"url_contains": "/never"}, "timeout_ms": 100}),
    )
    .await;
    let (_, scode) = error_code(&resp);
    assert_eq!(scode, "E_TIMEOUT");
}
