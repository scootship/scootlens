//! P2 dispatch happy-path coverage: every new syscall exercised end to end
//! through the mock engine with a real on-disk state dir (vault / uploads /
//! journal). Denial paths live in `redteam.rs`; this file proves the success
//! branches (state.* / net.* / cap.* / obs.* / dom.extract / act.select /
//! act.upload / act.type vault_ref).

use std::sync::Arc;

use scootlens_abi::{ApprovalMode, RpcId, RpcOutcome, RpcRequest, RpcResponse, TokenConstraints};
use scootlens_driver_mock::MockDriver;
use scootlens_kernel::{Caller, Dispatcher, Kernel, KernelConfig};
use serde_json::{Value, json};

fn admin() -> Caller {
    let mut constraints = TokenConstraints::default();
    constraints.approval.insert("*".into(), ApprovalMode::Auto);
    Caller {
        subject: "user:admin".into(),
        scopes: vec!["*".parse().expect("scope")],
        constraints,
    }
}

/// Dispatcher backed by a real state dir so vault/uploads/journal hit disk.
fn stateful() -> (Dispatcher, tempfile::TempDir) {
    let dir = tempfile::tempdir().expect("tempdir");
    let kernel = Kernel::open(
        Arc::new(MockDriver::standard_fixture()),
        KernelConfig {
            state_dir: Some(dir.path().to_path_buf()),
            ..KernelConfig::default()
        },
    )
    .expect("open kernel");
    (Dispatcher::new(kernel), dir)
}

async fn call(d: &Dispatcher, m: &str, params: Value) -> RpcResponse {
    d.dispatch(&admin(), RpcRequest::new(RpcId::Num(1), m, params))
        .await
}

fn ok(r: &RpcResponse) -> &Value {
    match &r.outcome {
        RpcOutcome::Success { result } => result,
        RpcOutcome::Failure { error } => panic!("expected success, got: {error:?}"),
    }
}

fn err_code(r: &RpcResponse) -> String {
    match &r.outcome {
        RpcOutcome::Failure { error } => error.data["code"].as_str().unwrap_or_default().to_owned(),
        RpcOutcome::Success { result } => panic!("expected error, got: {result:?}"),
    }
}

async fn pid_at(d: &Dispatcher, path: &str) -> String {
    let pid = ok(&call(d, "proc.spawn", json!({})).await)["pid"]
        .as_str()
        .expect("pid")
        .to_owned();
    call(
        d,
        "nav.goto",
        json!({ "pid": pid, "url": format!("http://fixture.test{path}") }),
    )
    .await;
    pid
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

// ---------- state.* ----------

#[tokio::test]
async fn state_vault_write_then_list_but_read_denied() {
    let (d, _dir) = stateful();
    ok(&call(
        &d,
        "state.write",
        json!({ "namespace": "vault", "key": "pw", "value": "s3cret" }),
    )
    .await);
    let list = call(&d, "state.list", json!({ "namespace": "vault" })).await;
    assert_eq!(ok(&list)["names"][0], "pw");
    // Even admin cannot read a vault value back out.
    let read = call(
        &d,
        "state.read",
        json!({ "namespace": "vault", "key": "pw" }),
    )
    .await;
    assert_eq!(err_code(&read), "E_CAP_DENIED");
}

#[tokio::test]
async fn state_cookies_write_read_list_roundtrip() {
    let (d, _dir) = stateful();
    let pid = pid_at(&d, "/login").await;
    ok(&call(
        &d,
        "state.write",
        json!({ "pid": pid, "namespace": "cookies", "key": "sid", "value": "abc123" }),
    )
    .await);
    let read = call(
        &d,
        "state.read",
        json!({ "pid": pid, "namespace": "cookies", "key": "sid" }),
    )
    .await;
    assert_eq!(ok(&read)["value"], "abc123");
    let list = call(
        &d,
        "state.list",
        json!({ "pid": pid, "namespace": "cookies" }),
    )
    .await;
    assert_eq!(ok(&list)["names"][0], "sid");
    // Bulk read (no key) returns the namespaced map without the prefix.
    let all = call(
        &d,
        "state.read",
        json!({ "pid": pid, "namespace": "cookies" }),
    )
    .await;
    assert_eq!(ok(&all)["entries"]["sid"], "abc123");
}

#[tokio::test]
async fn state_storage_roundtrip() {
    let (d, _dir) = stateful();
    let pid = pid_at(&d, "/").await;
    ok(&call(
        &d,
        "state.write",
        json!({ "pid": pid, "namespace": "storage", "key": "theme", "value": "dark" }),
    )
    .await);
    let read = call(
        &d,
        "state.read",
        json!({ "pid": pid, "namespace": "storage", "key": "theme" }),
    )
    .await;
    assert_eq!(ok(&read)["value"], "dark");
}

#[tokio::test]
async fn state_list_downloads_is_empty_initially() {
    let (d, _dir) = stateful();
    let list = call(&d, "state.list", json!({ "namespace": "downloads" })).await;
    assert_eq!(ok(&list)["names"].as_array().expect("array").len(), 0);
}

// ---------- net.* ----------

#[tokio::test]
async fn net_rules_set_get_and_log() {
    let (d, _dir) = stateful();
    let rules = json!({
        "default": "deny",
        "rules": [{ "action": "allow", "host": "api.test" }],
    });
    ok(&call(&d, "net.rules.set", rules.clone()).await);
    let got = call(&d, "net.rules.get", json!({})).await;
    assert_eq!(ok(&got)["rules"]["default"], "deny");
    // Log starts empty (no requests flowed yet on the mock).
    let log = call(&d, "net.log", json!({ "limit": 10 })).await;
    assert!(ok(&log)["entries"].as_array().expect("array").is_empty());
}

// ---------- cap.* ----------

#[tokio::test]
async fn cap_list_reports_caller_scopes() {
    let (d, _dir) = stateful();
    let resp = call(&d, "cap.list", json!({})).await;
    assert_eq!(ok(&resp)["subject"], "user:admin");
    assert_eq!(ok(&resp)["scopes"][0], "*");
}

#[tokio::test]
async fn cap_grant_then_revoke() {
    let (d, _dir) = stateful();
    ok(&call(
        &d,
        "cap.grant",
        json!({ "subject": "agent:bot", "scope": "view@app.test" }),
    )
    .await);
    ok(&call(
        &d,
        "cap.revoke",
        json!({ "subject": "agent:bot", "scope": "view@app.test" }),
    )
    .await);
}

#[tokio::test]
async fn cap_request_appears_in_pending() {
    let (d, _dir) = stateful();
    let req = call(
        &d,
        "cap.request",
        json!({ "scope": "js:exec@fixture.test", "reason": "debugging" }),
    )
    .await;
    let id = ok(&req)["approval_id"].as_str().expect("id").to_owned();
    let pending = call(&d, "cap.pending", json!({})).await;
    let list = ok(&pending)["pending"].as_array().expect("array");
    assert!(list.iter().any(|p| p["id"] == id));
}

#[tokio::test]
async fn cap_approve_unknown_id_errors() {
    let (d, _dir) = stateful();
    let resp = call(
        &d,
        "cap.approve",
        json!({ "approval_id": "apr-does-not-exist", "decision": "allow" }),
    )
    .await;
    assert_eq!(err_code(&resp), "E_INVALID_ARG");
}

// ---------- obs.* ----------

#[tokio::test]
async fn obs_journal_records_calls_and_trace_filters_by_pid() {
    let (d, _dir) = stateful();
    let pid = pid_at(&d, "/login").await;
    call(&d, "view.snapshot", json!({ "pid": pid })).await;

    let journal = call(&d, "obs.journal", json!({ "limit": 50 })).await;
    let entries = ok(&journal)["entries"].as_array().expect("array");
    assert!(!entries.is_empty(), "journal must record calls");

    let trace = call(&d, "obs.trace", json!({ "pid": pid })).await;
    let traced = ok(&trace)["entries"].as_array().expect("array");
    assert!(
        traced.iter().all(|e| e["pid"] == json!(pid)),
        "trace must be pid-scoped"
    );
}

// ---------- dom.extract ----------

#[tokio::test]
async fn dom_extract_filters_by_role() {
    let (d, _dir) = stateful();
    let pid = pid_at(&d, "/widgets").await;
    let resp = call(&d, "dom.extract", json!({ "pid": pid, "role": "combobox" })).await;
    let nodes = ok(&resp)["nodes"].as_array().expect("array");
    assert_eq!(nodes.len(), 1);
    assert_eq!(nodes[0]["role"], "combobox");
    assert_eq!(nodes[0]["name"], "Color");
    assert!(nodes[0]["ref"].is_string(), "extracted node carries a ref");
}

// ---------- act.select ----------

#[tokio::test]
async fn act_select_updates_combobox_value() {
    let (d, _dir) = stateful();
    let pid = pid_at(&d, "/widgets").await;
    let snap = call(&d, "view.snapshot", json!({ "pid": pid })).await;
    let text = ok(&snap)["text"].as_str().expect("text").to_owned();
    let color_ref = extract_ref(&text, "Color");

    ok(&call(
        &d,
        "act.select",
        json!({ "pid": pid, "ref": color_ref, "values": ["green"] }),
    )
    .await);

    let snap2 = call(&d, "view.snapshot", json!({ "pid": pid })).await;
    let text2 = ok(&snap2)["text"].as_str().expect("text").to_owned();
    assert!(
        text2.contains("= \"green\""),
        "select must update value:\n{text2}"
    );
}

// ---------- act.upload ----------

#[tokio::test]
async fn act_upload_accepts_sandbox_file() {
    let (d, dir) = stateful();
    std::fs::write(dir.path().join("uploads/doc.txt"), b"payload").expect("seed upload");
    let pid = pid_at(&d, "/widgets").await;
    let snap = call(&d, "view.snapshot", json!({ "pid": pid })).await;
    let text = ok(&snap)["text"].as_str().expect("text").to_owned();
    let file_ref = extract_ref(&text, "Attachment");

    let resp = call(
        &d,
        "act.upload",
        json!({ "pid": pid, "ref": file_ref, "path": "doc.txt" }),
    )
    .await;
    // Mock accepts the upload action; the point is the sandbox path resolved.
    assert!(
        matches!(resp.outcome, RpcOutcome::Success { .. }),
        "upload should succeed: {:?}",
        resp.outcome
    );
}

#[tokio::test]
async fn act_upload_rejects_escape() {
    let (d, _dir) = stateful();
    let pid = pid_at(&d, "/widgets").await;
    let snap = call(&d, "view.snapshot", json!({ "pid": pid })).await;
    let text = ok(&snap)["text"].as_str().expect("text").to_owned();
    let file_ref = extract_ref(&text, "Attachment");
    let resp = call(
        &d,
        "act.upload",
        json!({ "pid": pid, "ref": file_ref, "path": "../../etc/passwd" }),
    )
    .await;
    assert_eq!(err_code(&resp), "E_INVALID_ARG");
}

// ---------- act.type vault_ref ----------

#[tokio::test]
async fn act_type_vault_ref_injects_without_leaking() {
    let (d, _dir) = stateful();
    ok(&call(
        &d,
        "state.write",
        json!({ "namespace": "vault", "key": "pw", "value": "TOPSECRET-xyz" }),
    )
    .await);
    let pid = pid_at(&d, "/login").await;
    let snap = call(&d, "view.snapshot", json!({ "pid": pid })).await;
    let text = ok(&snap)["text"].as_str().expect("text").to_owned();
    let pass_ref = extract_ref(&text, "Password");

    let typed = call(
        &d,
        "act.type",
        json!({ "pid": pid, "ref": pass_ref, "vault_ref": "pw" }),
    )
    .await;
    assert!(matches!(typed.outcome, RpcOutcome::Success { .. }));

    // Snapshot must show the masked value, never the plaintext.
    let snap2 = call(&d, "view.snapshot", json!({ "pid": pid })).await;
    let dump = serde_json::to_string(&snap2).expect("json");
    assert!(!dump.contains("TOPSECRET-xyz"), "secret leaked:\n{dump}");
}

#[tokio::test]
async fn act_type_rejects_text_and_vault_ref_together() {
    let (d, _dir) = stateful();
    ok(&call(
        &d,
        "state.write",
        json!({ "namespace": "vault", "key": "pw", "value": "abc" }),
    )
    .await);
    let pid = pid_at(&d, "/login").await;
    let snap = call(&d, "view.snapshot", json!({ "pid": pid })).await;
    let text = ok(&snap)["text"].as_str().expect("text").to_owned();
    let pass_ref = extract_ref(&text, "Password");
    let resp = call(
        &d,
        "act.type",
        json!({ "pid": pid, "ref": pass_ref, "text": "x", "vault_ref": "pw" }),
    )
    .await;
    assert_eq!(err_code(&resp), "E_INVALID_ARG");
}
