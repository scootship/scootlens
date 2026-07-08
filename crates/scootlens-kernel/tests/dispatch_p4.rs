//! P4 dispatch coverage：人工接管（takeover）与回放导出（obs.replay.export）。
//!
//! 验收对应 docs/09-roadmap.md P4 门禁：
//! - 接管 e2e：Agent 操作中 → 人接管 → Agent 输入挂起 → 人输入 → 归还控制 →
//!   Agent 恢复执行；`act.takeover` 事件序列正确
//! - 回放包：journal 哈希链段可离线重放验证 + 画面帧对齐

use std::sync::Arc;
use std::time::Duration;

use scootlens_abi::{ApprovalMode, RpcId, RpcOutcome, RpcRequest, RpcResponse, TokenConstraints};
use scootlens_driver_mock::MockDriver;
use scootlens_kernel::{BusEvent, BusReceiver, Caller, Dispatcher, Kernel, KernelConfig};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

fn admin() -> Caller {
    limited("user:admin", &["*"])
}

/// 受限主体（审批 Auto，聚焦纯授权/接管语义）。
fn limited(subject: &str, scopes: &[&str]) -> Caller {
    let mut constraints = TokenConstraints::default();
    constraints.approval.insert("*".into(), ApprovalMode::Auto);
    Caller {
        subject: subject.into(),
        scopes: scopes.iter().map(|s| s.parse().expect("scope")).collect(),
        constraints,
    }
}

fn agent() -> Caller {
    limited(
        "agent:worker",
        &[
            "proc:list",
            "nav@fixture.test",
            "view@fixture.test",
            "act@fixture.test",
        ],
    )
}

fn dispatcher_with(config: KernelConfig) -> Dispatcher {
    Dispatcher::new(Kernel::new(
        Arc::new(MockDriver::standard_fixture()),
        config,
    ))
}

fn dispatcher() -> Dispatcher {
    dispatcher_with(KernelConfig::default())
}

async fn call_as(d: &Dispatcher, who: &Caller, m: &str, params: Value) -> RpcResponse {
    d.dispatch(who, RpcRequest::new(RpcId::Num(1), m, params))
        .await
}

async fn call(d: &Dispatcher, m: &str, params: Value) -> RpcResponse {
    call_as(d, &admin(), m, params).await
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

/// spawn 并导航到 fixtures 登录页，返回 (pid, 一个可点击 ref)。
async fn spawn_on_login(d: &Dispatcher) -> (String, String) {
    let pid = ok(&call(d, "proc.spawn", json!({})).await)["pid"]
        .as_str()
        .expect("pid")
        .to_owned();
    ok(&call(
        d,
        "nav.goto",
        json!({ "pid": pid, "url": "http://fixture.test/login" }),
    )
    .await);
    let snap = ok(&call(d, "view.snapshot", json!({ "pid": pid })).await)["text"]
        .as_str()
        .expect("text")
        .to_owned();
    let line = snap
        .lines()
        .find(|l| l.contains("\"Username\"") && l.contains('['))
        .expect("username line");
    let start = line.rfind('[').expect("[") + 1;
    let end = line.rfind(']').expect("]");
    (pid, line[start..end].to_owned())
}

async fn wait_topic(rx: &mut BusReceiver, topic: &str) -> BusEvent {
    for _ in 0..500 {
        let ev = tokio::time::timeout(Duration::from_secs(5), rx.recv())
            .await
            .unwrap_or_else(|_| panic!("timed out waiting for {topic}"))
            .expect("bus closed");
        if ev.payload.topic() == topic {
            return ev;
        }
    }
    panic!("no {topic} within 500 events");
}

// ================= takeover =================

/// 接管 e2e：Agent 操作中 → 人接管 → Agent 输入挂起 → 人输入 → 归还控制 →
/// Agent 恢复执行。事件序列：active=true → active=false，act 全部成功。
#[tokio::test]
async fn takeover_holds_agent_input_and_resumes_on_release() {
    let d = dispatcher();
    let (pid, elem) = spawn_on_login(&d).await;
    let mut rx = d.kernel().subscribe();

    // Agent 正常操作
    ok(&call_as(
        &d,
        &agent(),
        "act.type",
        json!({ "pid": pid, "ref": elem, "text": "a" }),
    )
    .await);

    // 人接管
    let r = ok(&call(&d, "act.takeover.start", json!({ "pid": pid })).await).clone();
    assert_eq!(r["holder"], "user:admin");
    let ev = wait_topic(&mut rx, "act.takeover").await;
    assert_eq!(
        serde_json::to_value(&ev.payload).expect("json")["active"],
        true
    );

    // 幂等：holder 重复 start 不报错
    ok(&call(&d, "act.takeover.start", json!({ "pid": pid })).await);

    // Agent 的输入被挂起（任务不结束）
    let held = {
        let d = d.clone();
        let pid = pid.clone();
        let elem = elem.clone();
        tokio::spawn(async move {
            call_as(
                &d,
                &agent(),
                "act.type",
                json!({ "pid": pid, "ref": elem, "text": "b" }),
            )
            .await
        })
    };
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert!(
        !held.is_finished(),
        "agent input must be held during takeover"
    );

    // 人（holder）输入直接放行
    ok(&call(
        &d,
        "act.type",
        json!({ "pid": pid, "ref": elem, "text": "human" }),
    )
    .await);

    // 归还控制 → Agent 挂起的调用恢复并成功
    ok(&call(&d, "act.takeover.end", json!({ "pid": pid })).await);
    let ev = wait_topic(&mut rx, "act.takeover").await;
    assert_eq!(
        serde_json::to_value(&ev.payload).expect("json")["active"],
        false
    );
    let resumed = held.await.expect("join");
    ok(&resumed);

    // 归还后 Agent 直接操作无阻塞
    ok(&call_as(
        &d,
        &agent(),
        "act.click",
        json!({ "pid": pid, "ref": elem }),
    )
    .await);
}

#[tokio::test]
async fn takeover_is_exclusive_and_holder_scoped() {
    let d = dispatcher();
    let (pid, _) = spawn_on_login(&d).await;
    let alice = limited("user:alice", &["act:takeover"]);
    let bob = limited("user:bob", &["act:takeover"]);

    ok(&call_as(&d, &alice, "act.takeover.start", json!({ "pid": pid })).await);
    // 他人抢占 → E_INVALID_ARG；他人归还 → E_CAP_DENIED
    let resp = call_as(&d, &bob, "act.takeover.start", json!({ "pid": pid })).await;
    assert_eq!(err_code(&resp), "E_INVALID_ARG");
    let resp = call_as(&d, &bob, "act.takeover.end", json!({ "pid": pid })).await;
    assert_eq!(err_code(&resp), "E_CAP_DENIED");
    // holder 正常归还；重复归还 → E_INVALID_ARG
    ok(&call_as(&d, &alice, "act.takeover.end", json!({ "pid": pid })).await);
    let resp = call_as(&d, &alice, "act.takeover.end", json!({ "pid": pid })).await;
    assert_eq!(err_code(&resp), "E_INVALID_ARG");
}

#[tokio::test]
async fn takeover_requires_running_proc_and_scope() {
    let d = dispatcher();
    let (pid, _) = spawn_on_login(&d).await;

    // act@origin 不覆盖 act:takeover（origin 授权不得升格为系统级接管）
    let resp = call_as(&d, &agent(), "act.takeover.start", json!({ "pid": pid })).await;
    assert_eq!(err_code(&resp), "E_CAP_DENIED");

    // 挂起态不可接管
    ok(&call(&d, "proc.suspend", json!({ "pid": pid })).await);
    let resp = call(&d, "act.takeover.start", json!({ "pid": pid })).await;
    assert_eq!(err_code(&resp), "E_INVALID_ARG");
    ok(&call(&d, "proc.resume", json!({ "pid": pid })).await);

    // 未知 pid
    let resp = call(&d, "act.takeover.start", json!({ "pid": "p-zz" })).await;
    assert_eq!(err_code(&resp), "E_PROC_NOT_FOUND");
}

#[tokio::test]
async fn held_input_times_out_with_e_timeout() {
    let d = dispatcher_with(KernelConfig {
        takeover_hold_timeout: Duration::from_millis(50),
        ..KernelConfig::default()
    });
    let (pid, elem) = spawn_on_login(&d).await;
    ok(&call(&d, "act.takeover.start", json!({ "pid": pid })).await);
    let resp = call_as(
        &d,
        &agent(),
        "act.click",
        json!({ "pid": pid, "ref": elem }),
    )
    .await;
    assert_eq!(err_code(&resp), "E_TIMEOUT");
}

/// 进程终止自动清除接管：挂起的输入被唤醒并按进程状态失败，事件序列正确。
#[tokio::test]
async fn kill_clears_takeover_and_wakes_held_input() {
    let d = dispatcher();
    let (pid, elem) = spawn_on_login(&d).await;
    let mut rx = d.kernel().subscribe();
    ok(&call(&d, "act.takeover.start", json!({ "pid": pid })).await);
    wait_topic(&mut rx, "act.takeover").await;

    let held = {
        let d = d.clone();
        let pid = pid.clone();
        tokio::spawn(async move {
            call_as(
                &d,
                &agent(),
                "act.click",
                json!({ "pid": pid, "ref": elem }),
            )
            .await
        })
    };
    tokio::time::sleep(Duration::from_millis(30)).await;
    ok(&call(&d, "proc.kill", json!({ "pid": pid })).await);

    let ev = wait_topic(&mut rx, "act.takeover").await;
    assert_eq!(
        serde_json::to_value(&ev.payload).expect("json")["active"],
        false,
        "kill must broadcast takeover release"
    );
    let resp = held.await.expect("join");
    assert!(
        matches!(resp.outcome, RpcOutcome::Failure { .. }),
        "woken call fails against terminated proc"
    );
}

// ================= obs.replay.export =================

/// 离线验证链段：首行之后 prev 必须链接前行 hash，且每行 hash = sha256(prev+raw)。
fn assert_chain_replays(journal: &[Value]) {
    assert!(!journal.is_empty(), "bundle journal must not be empty");
    let mut prev_hash: Option<String> = None;
    for line in journal {
        let (prev, hash, raw) = (
            line["prev"].as_str().expect("prev"),
            line["hash"].as_str().expect("hash"),
            line["raw"].as_str().expect("raw"),
        );
        if let Some(p) = &prev_hash {
            assert_eq!(prev, p, "chain must link consecutive lines");
        }
        let mut h = Sha256::new();
        h.update(prev.as_bytes());
        h.update(raw.as_bytes());
        assert_eq!(
            hex::encode(h.finalize()),
            hash,
            "hash must replay from prev+raw"
        );
        prev_hash = Some(hash.to_owned());
    }
}

#[tokio::test]
async fn replay_bundle_carries_verifiable_chain_and_frames() {
    let d = dispatcher();
    let (pid, elem) = spawn_on_login(&d).await;
    ok(&call(
        &d,
        "act.type",
        json!({ "pid": pid, "ref": elem, "text": "user" }),
    )
    .await);
    // 两次截图 → 两帧
    ok(&call(&d, "view.screenshot", json!({ "pid": pid })).await);
    ok(&call(&d, "view.screenshot", json!({ "pid": pid })).await);

    let resp = call(&d, "obs.replay.export", json!({ "pid": pid })).await;
    let bundle = ok(&resp)["bundle"].clone();
    assert_eq!(bundle["format_version"], 1);
    assert_eq!(bundle["pid"], pid);
    assert_eq!(bundle["engine"], "mock");
    let frames = bundle["frames"].as_array().expect("frames");
    assert_eq!(frames.len(), 2, "each screenshot contributes one frame");
    assert_eq!(frames[0]["format"], "png");
    assert!(frames[0]["ts_ms"].as_u64().expect("ts") <= frames[1]["ts_ms"].as_u64().expect("ts"));

    let journal = bundle["journal"].as_array().expect("journal");
    assert_chain_replays(journal);
    // 链段涵盖本会话的 syscall 轨迹（含导出调用自身的 call 记录）
    let raws: Vec<&str> = journal
        .iter()
        .map(|l| l["raw"].as_str().expect("raw"))
        .collect();
    assert!(raws.iter().any(|r| r.contains("nav.goto")));
    assert!(raws.iter().any(|r| r.contains("obs.replay.export")));
}

#[tokio::test]
async fn replay_export_survives_proc_termination() {
    let d = dispatcher();
    let (pid, _) = spawn_on_login(&d).await;
    ok(&call(&d, "view.screenshot", json!({ "pid": pid })).await);
    ok(&call(&d, "proc.kill", json!({ "pid": pid })).await);

    let resp = call(
        &d,
        "obs.replay.export",
        json!({ "pid": pid, "journal_limit": 8 }),
    )
    .await;
    let bundle = ok(&resp)["bundle"].clone();
    assert_eq!(bundle["frames"].as_array().expect("frames").len(), 1);
    let journal = bundle["journal"].as_array().expect("journal");
    assert!(journal.len() <= 8, "journal_limit caps the segment");
    assert_chain_replays(journal);
}

#[tokio::test]
async fn replay_export_validates_pid_and_scope() {
    let d = dispatcher();
    let resp = call(&d, "obs.replay.export", json!({ "pid": "p-zz" })).await;
    assert_eq!(err_code(&resp), "E_PROC_NOT_FOUND");

    let (pid, _) = spawn_on_login(&d).await;
    // obs:replay 是敏感作用域；未持有 → E_CAP_DENIED
    let resp = call_as(&d, &agent(), "obs.replay.export", json!({ "pid": pid })).await;
    assert_eq!(err_code(&resp), "E_CAP_DENIED");
}
