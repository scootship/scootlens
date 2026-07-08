//! P2 red-team suite (docs/09-roadmap.md P2 acceptance gates).
//!
//! Each attack class has >= 2 cases; every one must be blocked:
//!
//! - T1 token forgery / tampering / expiry
//! - T2 privilege escalation (scope upgrade, cross-origin action)
//! - T3 vault credential leakage (journal / snapshot / ABI return scanned)
//! - T4 network-rule bypass
//! - T5 approval bypass (deny / timeout) + approval e2e (suspend -> approve -> resume)
//!
//! Plus: exhaustive no-scope sweep -> every privileged method -> E_CAP_DENIED.

use std::sync::Arc;
use std::time::Duration;

use scootlens_abi::{
    ApprovalMode, NetAction, NetDefault, NetRequestSummary, NetRule, NetRuleSet, Pid, RpcId,
    RpcOutcome, RpcRequest, RpcResponse, TokenClaims, TokenConstraints, method,
};
use scootlens_driver_mock::MockDriver;
use scootlens_kernel::{Caller, Dispatcher, Kernel, KernelConfig};
use serde_json::{Value, json};

// ---------- harness ----------

fn kernel() -> Kernel {
    Kernel::new(
        Arc::new(MockDriver::standard_fixture()),
        KernelConfig::default(),
    )
}

fn dispatcher() -> Dispatcher {
    Dispatcher::new(kernel())
}

fn dispatcher_with(config: KernelConfig) -> Dispatcher {
    Dispatcher::new(Kernel::new(
        Arc::new(MockDriver::standard_fixture()),
        config,
    ))
}

/// Build a constrained caller from scope + approval-policy literals.
fn caller(subject: &str, scopes: &[&str], approvals: &[(&str, ApprovalMode)]) -> Caller {
    let mut constraints = TokenConstraints::default();
    for (pat, mode) in approvals {
        constraints.approval.insert((*pat).to_string(), *mode);
    }
    Caller {
        subject: subject.into(),
        scopes: scopes.iter().map(|s| s.parse().expect("scope")).collect(),
        constraints,
    }
}

/// Full-authority caller (auto-approve everything).
fn admin() -> Caller {
    caller("user:admin", &["*"], &[("*", ApprovalMode::Auto)])
}

async fn call(d: &Dispatcher, who: &Caller, m: &str, params: Value) -> RpcResponse {
    d.dispatch(who, RpcRequest::new(RpcId::Num(1), m, params))
        .await
}

fn err_code(resp: &RpcResponse) -> String {
    match &resp.outcome {
        RpcOutcome::Failure { error } => error.data["code"].as_str().unwrap_or_default().to_owned(),
        RpcOutcome::Success { result } => panic!("expected error, got success: {result:?}"),
    }
}

fn result_of(resp: &RpcResponse) -> &Value {
    match &resp.outcome {
        RpcOutcome::Success { result } => result,
        RpcOutcome::Failure { error } => panic!("expected success, got error: {error:?}"),
    }
}

fn resp_json(resp: &RpcResponse) -> Value {
    serde_json::to_value(resp).expect("serialize response")
}

/// Admin spawns a proc and navigates it so an origin is on record.
async fn spawned_pid(d: &Dispatcher) -> String {
    let a = admin();
    let pid = result_of(&call(d, &a, "proc.spawn", json!({})).await)["pid"]
        .as_str()
        .expect("pid")
        .to_owned();
    call(
        d,
        &a,
        "nav.goto",
        json!({ "pid": pid, "url": "http://fixture.test/login" }),
    )
    .await;
    pid
}

// ================= T1: token forgery / tampering / expiry =================

#[test]
fn t1_foreign_key_signature_rejected() {
    // Token minted by kernel A must not verify under kernel B's key.
    let a = kernel();
    let b = kernel();
    let token = a.security().issue(&TokenClaims {
        subject: "agent:x".into(),
        scopes: vec!["*".parse().expect("parse")],
        constraints: TokenConstraints::default(),
        issued_by: "a".into(),
        issued_at: 0,
    });
    let err = b
        .security()
        .verify(&token)
        .expect_err("foreign key must fail");
    assert_eq!(err.code, scootlens_abi::ErrorCode::CapDenied);
}

#[test]
fn t1_tampered_payload_rejected() {
    let k = kernel();
    let token = k.security().issue(&TokenClaims {
        subject: "agent:x".into(),
        scopes: vec!["view".parse().expect("parse")],
        constraints: TokenConstraints::default(),
        issued_by: "k".into(),
        issued_at: 0,
    });
    // Flip a byte in the payload segment.
    let mut parts: Vec<&str> = token.split('.').collect();
    let mut payload: Vec<u8> = parts[1].bytes().collect();
    payload[3] ^= 0x01;
    let mutated = String::from_utf8_lossy(&payload).into_owned();
    parts[1] = &mutated;
    let tampered = parts.join(".");
    assert!(
        k.security().verify(&tampered).is_err(),
        "tampered token must fail"
    );
}

#[test]
fn t1_expired_token_rejected() {
    let k = kernel();
    let constraints = TokenConstraints {
        expires_at: Some(1), // 1970 -> long expired
        ..Default::default()
    };
    let token = k.security().issue(&TokenClaims {
        subject: "agent:x".into(),
        scopes: vec!["*".parse().expect("parse")],
        constraints,
        issued_by: "k".into(),
        issued_at: 0,
    });
    let err = k.security().verify(&token).expect_err("expired must fail");
    assert_eq!(err.code, scootlens_abi::ErrorCode::CapDenied);
}

#[test]
fn t1_garbage_tokens_rejected() {
    let k = kernel();
    for bogus in ["", "not-a-token", "slt1.aaa.bbb", "a.b.c.d"] {
        assert!(k.security().verify(bogus).is_err(), "must reject {bogus:?}");
    }
}

// ================= T2: privilege escalation =================

#[tokio::test]
async fn t2_view_only_cannot_exec_js() {
    let d = dispatcher();
    let pid = spawned_pid(&d).await;
    let viewer = caller("agent:viewer", &["view", "nav"], &[]);
    let resp = call(
        &d,
        &viewer,
        "js.exec",
        json!({ "pid": pid, "script": "1+1" }),
    )
    .await;
    assert_eq!(err_code(&resp), "E_CAP_DENIED");
}

#[tokio::test]
async fn t2_cross_origin_navigation_denied() {
    let d = dispatcher();
    let pid = spawned_pid(&d).await;
    // Scope pins nav to app.example.com; target is a different origin.
    let scoped = caller("agent:scoped", &["nav@app.example.com"], &[]);
    let resp = call(
        &d,
        &scoped,
        "nav.goto",
        json!({ "pid": pid, "url": "http://evil.test/" }),
    )
    .await;
    assert_eq!(err_code(&resp), "E_CAP_DENIED");
}

#[tokio::test]
async fn t2_read_scope_cannot_write_state() {
    let d = dispatcher();
    let pid = spawned_pid(&d).await;
    let reader = caller("agent:reader", &["state:read:cookies"], &[]);
    let resp = call(
        &d,
        &reader,
        "state.write",
        json!({ "pid": pid, "namespace": "cookies", "key": "sid", "value": "x" }),
    )
    .await;
    assert_eq!(err_code(&resp), "E_CAP_DENIED");
}

#[tokio::test]
async fn t2_vault_is_write_only_even_with_read_scope() {
    // Even a caller granted state:read:vault cannot exfiltrate vault contents.
    let d = dispatcher();
    let reader = caller(
        "agent:reader",
        &["state:read:vault"],
        &[("*", ApprovalMode::Auto)],
    );
    let resp = call(
        &d,
        &reader,
        "state.read",
        json!({ "namespace": "vault", "key": "pw" }),
    )
    .await;
    assert_eq!(err_code(&resp), "E_CAP_DENIED");
}

// ================= T3: vault credential leakage =================

const SECRET: &str = "hunter2-SUPER-SECRET-a1b2c3";

#[tokio::test]
async fn t3_vault_secret_absent_from_journal() {
    let d = dispatcher();
    let a = admin();
    // Write a secret into the vault.
    let w = call(
        &d,
        &a,
        "state.write",
        json!({ "namespace": "vault", "key": "pw", "value": SECRET }),
    )
    .await;
    assert_eq!(result_of(&w)["ok"], true);

    // Pull the journal back and scan every byte.
    let j = call(&d, &a, "obs.journal", json!({ "limit": 100 })).await;
    let dump = serde_json::to_string(result_of(&j)).expect("json");
    assert!(
        !dump.contains(SECRET),
        "vault secret leaked into journal:\n{dump}"
    );
    // The listing surfaces the name, never the value.
    let l = call(&d, &a, "state.list", json!({ "namespace": "vault" })).await;
    assert_eq!(result_of(&l)["names"][0], "pw");
}

#[tokio::test]
async fn t3_vault_ref_secret_absent_from_snapshot_and_return() {
    let d = dispatcher();
    let a = admin();
    let pid = spawned_pid(&d).await;
    call(
        &d,
        &a,
        "state.write",
        json!({ "namespace": "vault", "key": "pw", "value": SECRET }),
    )
    .await;

    // Locate the password field, then inject via vault_ref (never the raw value).
    let snap = call(&d, &a, "view.snapshot", json!({ "pid": pid })).await;
    let text = result_of(&snap)["text"].as_str().expect("text").to_owned();
    let pass_ref = extract_ref(&text, "Password");

    let typed = call(
        &d,
        &a,
        "act.type",
        json!({ "pid": pid, "ref": pass_ref, "vault_ref": "pw" }),
    )
    .await;
    // The typed response itself must not carry the secret.
    let typed_dump = serde_json::to_string(result_of(&typed)).expect("json");
    assert!(!typed_dump.contains(SECRET), "secret in act.type return");

    // Re-snapshot: the injected value must be masked on the way out.
    let snap2 = call(&d, &a, "view.snapshot", json!({ "pid": pid })).await;
    let dump = resp_json(&snap2).to_string();
    assert!(
        !dump.contains(SECRET),
        "vault secret leaked into snapshot:\n{dump}"
    );
}

// ================= T4: network-rule bypass =================

fn req(host: &str) -> NetRequestSummary {
    NetRequestSummary {
        url: format!("http://{host}/x"),
        method: "GET".into(),
        resource_type: "document".into(),
    }
}

#[tokio::test]
async fn t4_global_deny_all_blocks_every_host() {
    let d = dispatcher();
    let pid: Pid = spawned_pid(&d).await.parse().expect("parse");
    d.kernel().netstack().set_rules(
        None,
        NetRuleSet {
            default: NetDefault::Deny,
            rules: vec![],
        },
    );
    for host in ["api.test", "cdn.test", "evil.test"] {
        assert!(
            !d.kernel().netstack().decide(&pid, &req(host)).allowed(),
            "deny-all must block {host}"
        );
    }
}

#[tokio::test]
async fn t4_allowlist_blocks_unlisted_host() {
    let d = dispatcher();
    let pid: Pid = spawned_pid(&d).await.parse().expect("parse");
    d.kernel().netstack().set_rules(
        None,
        NetRuleSet {
            default: NetDefault::Deny,
            rules: vec![NetRule {
                action: NetAction::Allow,
                host: "api.test".into(),
                methods: vec![],
                resource_types: vec![],
                set_headers: vec![],
            }],
        },
    );
    assert!(
        d.kernel()
            .netstack()
            .decide(&pid, &req("api.test"))
            .allowed(),
        "allowlisted host must pass"
    );
    assert!(
        !d.kernel()
            .netstack()
            .decide(&pid, &req("evil.test"))
            .allowed(),
        "unlisted host must be blocked"
    );
}

#[tokio::test]
async fn t4_setting_rules_requires_capability() {
    let d = dispatcher();
    let nobody = caller("agent:nobody", &["nav", "view"], &[]);
    let resp = call(
        &d,
        &nobody,
        "net.rules.set",
        json!({ "default": "deny", "rules": [] }),
    )
    .await;
    assert_eq!(err_code(&resp), "E_CAP_DENIED");
}

// ================= T5: approval flow =================

/// A caller holding a sensitive scope but no auto-approval policy.
fn manual_js_caller() -> Caller {
    caller("agent:manual", &["js:exec@fixture.test"], &[])
}

#[tokio::test]
async fn t5_denied_approval_yields_cap_denied() {
    let d = dispatcher();
    let pid = spawned_pid(&d).await;
    let who = manual_js_caller();

    let d2 = d.clone();
    let pid2 = pid.clone();
    let task = tokio::spawn(async move {
        call(&d2, &who, "js.exec", json!({ "pid": pid2, "script": "1" })).await
    });

    let approval_id = await_pending(&d).await;
    call(
        &d,
        &admin(),
        "cap.approve",
        json!({ "approval_id": approval_id, "decision": "deny" }),
    )
    .await;

    let resp = task.await.expect("join");
    assert_eq!(err_code(&resp), "E_CAP_DENIED");
}

#[tokio::test]
async fn t5_approval_timeout_yields_pending() {
    let d = dispatcher_with(KernelConfig {
        approval_timeout: Duration::from_millis(200),
        ..KernelConfig::default()
    });
    let pid = spawned_pid(&d).await;
    let who = manual_js_caller();
    // No approver shows up -> must time out as E_APPROVAL_PENDING.
    let resp = call(&d, &who, "js.exec", json!({ "pid": pid, "script": "1" })).await;
    assert_eq!(err_code(&resp), "E_APPROVAL_PENDING");
}

/// Acceptance gate #4: suspend -> approve -> resume, end to end.
#[tokio::test]
async fn t5_approval_e2e_allow_resumes_call() {
    let d = dispatcher();
    let pid = spawned_pid(&d).await;
    let who = manual_js_caller();

    let d2 = d.clone();
    let pid2 = pid.clone();
    let task = tokio::spawn(async move {
        call(&d2, &who, "js.exec", json!({ "pid": pid2, "script": "1" })).await
    });

    let approval_id = await_pending(&d).await;
    let ap = call(
        &d,
        &admin(),
        "cap.approve",
        json!({ "approval_id": approval_id, "decision": "allow" }),
    )
    .await;
    assert_eq!(result_of(&ap)["ok"], true);

    let resp = task.await.expect("join");
    // Suspended call resumes and completes successfully.
    assert!(
        matches!(resp.outcome, RpcOutcome::Success { .. }),
        "approved call must resume: {:?}",
        resp.outcome
    );
}

// ================= exhaustive no-scope sweep =================

/// Every privileged method, called with an empty-scope caller and otherwise
/// valid params, must return E_CAP_DENIED (never leak past authorization).
#[tokio::test]
async fn exhaustive_no_scope_is_denied() {
    let d = dispatcher();
    let pid = spawned_pid(&d).await;
    let nobody = caller("agent:nobody", &[], &[]);

    // Self-serve or connection-scoped methods are intentionally not scope-gated.
    let skip = [
        method::SYS_INFO,
        method::CAP_LIST,
        method::CAP_REQUEST,
        method::EVT_SUBSCRIBE,
        method::EVT_UNSUBSCRIBE,
    ];

    // Superset params: param structs don't deny unknown fields, so one object
    // satisfies every method's parser, letting each reach the authz gate.
    let params = json!({
        "pid": pid,
        "url": "http://blocked.test/",
        "ref": "s1e0",
        "text": "x",
        "script": "1",
        "keys": "Enter",
        "namespace": "cookies",
        "key": "k",
        "value": "v",
        "scope": "view",
        "subject": "agent:target",
        "approval_id": "apr-404",
        "decision": "allow",
        "values": ["a"],
        "path": "f.txt",
        "cond": { "url_contains": "x" },
        "timeout_ms": 50,
        "reason": "r",
    });

    for &m in method::ALL {
        if skip.contains(&m) {
            continue;
        }
        let resp = call(&d, &nobody, m, params.clone()).await;
        assert_eq!(
            err_code(&resp),
            "E_CAP_DENIED",
            "method {m} must deny an empty-scope caller"
        );
    }
}

// ---------- helpers ----------

/// Poll the approval inbox until a request appears; return its id.
async fn await_pending(d: &Dispatcher) -> String {
    for _ in 0..200 {
        if let Some(p) = d.kernel().security().pending_list().first() {
            return p.id.clone();
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    panic!("no approval became pending");
}

/// Pull an element ref out of a compact snapshot line containing `name`.
fn extract_ref(text: &str, name: &str) -> String {
    let line = text
        .lines()
        .find(|l| l.contains(&format!("\"{name}\"")) && l.contains('['))
        .unwrap_or_else(|| panic!("no interactive line with {name} in:\n{text}"));
    let start = line.rfind('[').expect("[") + 1;
    let end = line.rfind(']').expect("]");
    line[start..end].to_owned()
}
