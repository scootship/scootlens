//! P3 dispatch coverage: OS semantics end to end through the mock engine.
//! suspend/resume scheduling, snapshot/restore (content-addressed), profile
//! reuse (state.export/import + spawn preload), memory quotas (warn/suspend/
//! kill + quota:high gate), Workflow Daemon (manual/cron/event triggers,
//! retry, least privilege) and Event Bus backpressure semantics.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use scootlens_abi::{ApprovalMode, RpcId, RpcOutcome, RpcRequest, RpcResponse, TokenConstraints};
use scootlens_driver_mock::MockDriver;
use scootlens_kernel::{
    BusEvent, BusReceiver, Caller, Dispatcher, Kernel, KernelConfig, WfRunStatus,
};
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

/// 受限主体：仅持有给定作用域（审批 Auto，聚焦纯授权判定）。
fn limited(subject: &str, scopes: &[&str]) -> Caller {
    let mut constraints = TokenConstraints::default();
    constraints.approval.insert("*".into(), ApprovalMode::Auto);
    Caller {
        subject: subject.into(),
        scopes: scopes.iter().map(|s| s.parse().expect("scope")).collect(),
        constraints,
    }
}

/// 内存模式 dispatcher；保留驱动引用用于故障/指标注入。
fn mem_dispatcher(config: KernelConfig) -> (Dispatcher, Arc<MockDriver>) {
    let driver = Arc::new(MockDriver::standard_fixture());
    let kernel = Kernel::new(Arc::clone(&driver) as _, config);
    (Dispatcher::new(kernel), driver)
}

/// 磁盘模式 dispatcher（snapshots/profiles 真实落盘）。
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
    call_as(d, &admin(), m, params).await
}

async fn call_as(d: &Dispatcher, who: &Caller, m: &str, params: Value) -> RpcResponse {
    d.dispatch(who, RpcRequest::new(RpcId::Num(1), m, params))
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

async fn spawn_pid(d: &Dispatcher) -> String {
    ok(&call(d, "proc.spawn", json!({})).await)["pid"]
        .as_str()
        .expect("pid")
        .to_owned()
}

async fn proc_state(d: &Dispatcher, pid: &str) -> String {
    ok(&call(d, "proc.info", json!({ "pid": pid })).await)["state"]
        .as_str()
        .expect("state")
        .to_owned()
}

/// 顺序消费总线直到出现目标主题（`limit` 条事件内），返回该事件。
async fn wait_topic(rx: &mut BusReceiver, topic: &str, secs: u64) -> BusEvent {
    for _ in 0..500 {
        let ev = tokio::time::timeout(Duration::from_secs(secs), rx.recv())
            .await
            .unwrap_or_else(|_| panic!("timed out waiting for {topic}"))
            .expect("bus closed");
        if ev.payload.topic() == topic {
            return ev;
        }
    }
    panic!("no {topic} within 500 events");
}

fn wf_status(ev: &BusEvent) -> WfRunStatus {
    match serde_json::to_value(&ev.payload).expect("payload")["status"]
        .as_str()
        .expect("status")
    {
        "started" => WfRunStatus::Started,
        "step_ok" => WfRunStatus::StepOk,
        "step_retry" => WfRunStatus::StepRetry,
        "succeeded" => WfRunStatus::Succeeded,
        "failed" => WfRunStatus::Failed,
        other => panic!("unknown wf status {other}"),
    }
}

// ================= proc.suspend / proc.resume =================

#[tokio::test]
async fn suspend_blocks_engine_ops_until_resume() {
    let (d, _drv) = mem_dispatcher(KernelConfig::default());
    let pid = spawn_pid(&d).await;

    ok(&call(&d, "proc.suspend", json!({ "pid": pid })).await);
    assert_eq!(proc_state(&d, &pid).await, "suspended");

    // 挂起进程拒绝一切引擎操作
    let nav = call(
        &d,
        "nav.goto",
        json!({ "pid": pid, "url": "http://fixture.test/login" }),
    )
    .await;
    assert_eq!(err_code(&nav), "E_INVALID_ARG");
    let snap = call(&d, "view.snapshot", json!({ "pid": pid })).await;
    assert_eq!(err_code(&snap), "E_INVALID_ARG");

    ok(&call(&d, "proc.resume", json!({ "pid": pid })).await);
    assert_eq!(proc_state(&d, &pid).await, "running");
    ok(&call(
        &d,
        "nav.goto",
        json!({ "pid": pid, "url": "http://fixture.test/login" }),
    )
    .await);
}

#[tokio::test]
async fn suspend_resume_reject_wrong_states() {
    let (d, _drv) = mem_dispatcher(KernelConfig::default());
    let pid = spawn_pid(&d).await;

    // Running 不能 resume；Suspended 不能重复 suspend
    let r = call(&d, "proc.resume", json!({ "pid": pid })).await;
    assert_eq!(err_code(&r), "E_INVALID_ARG");
    ok(&call(&d, "proc.suspend", json!({ "pid": pid })).await);
    let s = call(&d, "proc.suspend", json!({ "pid": pid })).await;
    assert_eq!(err_code(&s), "E_INVALID_ARG");

    // 未知 pid
    let nf = call(&d, "proc.suspend", json!({ "pid": "p-999" })).await;
    assert_eq!(err_code(&nf), "E_PROC_NOT_FOUND");

    // 挂起进程可直接 kill
    ok(&call(&d, "proc.kill", json!({ "pid": pid })).await);
    assert_eq!(proc_state(&d, &pid).await, "terminated");
}

#[tokio::test]
async fn suspend_releases_scheduler_slot() {
    let (d, _drv) = mem_dispatcher(KernelConfig {
        max_procs: 1,
        ..KernelConfig::default()
    });
    let p1 = spawn_pid(&d).await;

    // 唯一槽被 p1 占用；挂起后让出，p2 得以启动
    ok(&call(&d, "proc.suspend", json!({ "pid": p1 })).await);
    let p2 = spawn_pid(&d).await;
    assert_ne!(p1, p2);

    // p2 归还槽位后 p1 才能恢复（FIFO 重排队）
    ok(&call(&d, "proc.kill", json!({ "pid": p2 })).await);
    ok(&call(&d, "proc.resume", json!({ "pid": p1 })).await);
    assert_eq!(proc_state(&d, &p1).await, "running");
}

#[tokio::test(start_paused = true)]
async fn suspend_survives_a_day_then_resumes() {
    let (d, _drv) = mem_dispatcher(KernelConfig::default());
    let pid = spawn_pid(&d).await;
    ok(&call(&d, "proc.suspend", json!({ "pid": pid })).await);

    tokio::time::advance(Duration::from_secs(24 * 3600)).await;

    ok(&call(&d, "proc.resume", json!({ "pid": pid })).await);
    ok(&call(
        &d,
        "nav.goto",
        json!({ "pid": pid, "url": "http://fixture.test/login" }),
    )
    .await);
}

// ================= proc.snapshot / proc.restore =================

#[tokio::test]
async fn snapshot_restore_roundtrip_preserves_state_and_url() {
    let (d, _dir) = stateful();
    let pid = spawn_pid(&d).await;
    ok(&call(
        &d,
        "nav.goto",
        json!({ "pid": pid, "url": "http://fixture.test/login" }),
    )
    .await);
    ok(&call(
        &d,
        "state.write",
        json!({ "pid": pid, "namespace": "cookies", "key": "sid", "value": "abc123" }),
    )
    .await);

    let snap1 = ok(&call(&d, "proc.snapshot", json!({ "pid": pid })).await)["snap_id"]
        .as_str()
        .expect("snap_id")
        .to_owned();
    assert!(snap1.starts_with("snap-"), "content-addressed id: {snap1}");
    // 内容寻址：同一状态再快照得到同一 id
    let snap2 = ok(&call(&d, "proc.snapshot", json!({ "pid": pid })).await)["snap_id"]
        .as_str()
        .expect("snap_id")
        .to_owned();
    assert_eq!(snap1, snap2);

    // 状态变化 → 新 id
    ok(&call(
        &d,
        "state.write",
        json!({ "pid": pid, "namespace": "cookies", "key": "theme", "value": "dark" }),
    )
    .await);
    let snap3 = ok(&call(&d, "proc.snapshot", json!({ "pid": pid })).await)["snap_id"]
        .as_str()
        .expect("snap_id")
        .to_owned();
    assert_ne!(snap1, snap3);

    // 恢复为新进程：状态与页面都回来
    let restored = ok(&call(&d, "proc.restore", json!({ "snap_id": snap1 })).await)["pid"]
        .as_str()
        .expect("pid")
        .to_owned();
    assert_ne!(restored, pid);
    assert_eq!(proc_state(&d, &restored).await, "running");
    let sid = call(
        &d,
        "state.read",
        json!({ "pid": restored, "namespace": "cookies", "key": "sid" }),
    )
    .await;
    assert_eq!(ok(&sid)["value"], "abc123");
    // snap1 先于 theme 写入 → 不应带 theme
    let theme = call(
        &d,
        "state.read",
        json!({ "pid": restored, "namespace": "cookies", "key": "theme" }),
    )
    .await;
    assert_eq!(ok(&theme)["value"], Value::Null);
    // URL 恢复：视图应是 login 页
    let view = ok(&call(&d, "view.snapshot", json!({ "pid": restored })).await)["text"]
        .as_str()
        .expect("text")
        .to_owned();
    assert!(
        view.contains("Login"),
        "restored page must be login:\n{view}"
    );
}

#[tokio::test]
async fn snapshot_works_on_suspended_proc() {
    let (d, _dir) = stateful();
    let pid = spawn_pid(&d).await;
    ok(&call(&d, "proc.suspend", json!({ "pid": pid })).await);
    let snap = ok(&call(&d, "proc.snapshot", json!({ "pid": pid })).await)["snap_id"]
        .as_str()
        .expect("snap_id")
        .to_owned();
    assert!(snap.starts_with("snap-"));
}

#[tokio::test]
async fn restore_rejects_bad_ids_and_engine_mismatch() {
    let (d, _dir) = stateful();
    let pid = spawn_pid(&d).await;
    let snap = ok(&call(&d, "proc.snapshot", json!({ "pid": pid })).await)["snap_id"]
        .as_str()
        .expect("snap_id")
        .to_owned();

    // 引擎不匹配拒绝
    let mismatch = call(
        &d,
        "proc.restore",
        json!({ "snap_id": snap, "engine": "chromium" }),
    )
    .await;
    assert_eq!(err_code(&mismatch), "E_INVALID_ARG");

    // 未知快照 / 非法格式
    let missing = call(
        &d,
        "proc.restore",
        json!({ "snap_id": "snap-00000000000000ff" }),
    )
    .await;
    assert_eq!(err_code(&missing), "E_INVALID_ARG");
    let malformed = call(&d, "proc.restore", json!({ "snap_id": "not-a-snap" })).await;
    assert_eq!(err_code(&malformed), "E_INVALID_ARG");
}

// ================= state.export / state.import / profile 复用 =================

#[tokio::test]
async fn state_export_import_then_spawn_preloads_profile() {
    let (d, _dir) = stateful();
    let pid = spawn_pid(&d).await;
    ok(&call(
        &d,
        "nav.goto",
        json!({ "pid": pid, "url": "http://fixture.test/login" }),
    )
    .await);
    ok(&call(
        &d,
        "state.write",
        json!({ "pid": pid, "namespace": "cookies", "key": "sid", "value": "sess-42" }),
    )
    .await);

    let bundle = ok(&call(&d, "state.export", json!({ "pid": pid })).await)["state"].clone();
    assert!(bundle["entries"].is_object());

    ok(&call(
        &d,
        "state.import",
        json!({ "profile": "agent-x", "state": bundle }),
    )
    .await);

    // 以同名 profile spawn → 状态预加载
    let pid2 = ok(&call(&d, "proc.spawn", json!({ "profile": "agent-x" })).await)["pid"]
        .as_str()
        .expect("pid")
        .to_owned();
    let sid = call(
        &d,
        "state.read",
        json!({ "pid": pid2, "namespace": "cookies", "key": "sid" }),
    )
    .await;
    assert_eq!(ok(&sid)["value"], "sess-42");

    // 无关 profile 不受影响
    let pid3 = ok(&call(&d, "proc.spawn", json!({ "profile": "fresh" })).await)["pid"]
        .as_str()
        .expect("pid")
        .to_owned();
    let empty = call(
        &d,
        "state.read",
        json!({ "pid": pid3, "namespace": "cookies", "key": "sid" }),
    )
    .await;
    assert_eq!(ok(&empty)["value"], Value::Null);
}

#[tokio::test]
async fn state_import_rejects_path_traversal_profile_names() {
    let (d, _dir) = stateful();
    let long = "x".repeat(65);
    for bad in ["../evil", "a/b", "", ".hidden", long.as_str()] {
        let resp = call(
            &d,
            "state.import",
            json!({ "profile": bad, "state": { "entries": {} } }),
        )
        .await;
        assert_eq!(err_code(&resp), "E_INVALID_ARG", "profile name {bad:?}");
    }
}

// ================= Scheduler 配额 =================

fn quota_config() -> KernelConfig {
    KernelConfig {
        quota_poll_interval: Duration::from_millis(20),
        ..KernelConfig::default()
    }
}

#[tokio::test(start_paused = true)]
async fn quota_warn_emits_event_and_keeps_running() {
    let (d, drv) = mem_dispatcher(quota_config());
    let mut rx = d.kernel().subscribe();
    let pid = ok(&call(
        &d,
        "proc.spawn",
        json!({ "quotas": { "max_memory_bytes": 1024, "on_exceed": "warn" } }),
    )
    .await)["pid"]
        .as_str()
        .expect("pid")
        .to_owned();

    assert!(drv.set_memory_spawned(0, 4096));
    let ev = wait_topic(&mut rx, "quota.exceeded", 3600).await;
    let payload = serde_json::to_value(&ev.payload).expect("payload");
    assert_eq!(payload["usage_bytes"], 4096);
    assert_eq!(payload["limit_bytes"], 1024);
    assert_eq!(payload["policy"], "warn");
    assert_eq!(proc_state(&d, &pid).await, "running");

    // 越界处置有去抖：回落再越界才再次告警
    assert!(drv.set_memory_spawned(0, 512));
    for _ in 0..5 {
        // 多个小步推进 + yield：确保监控任务真正观测到回落（清除 over 标志）
        tokio::time::advance(Duration::from_millis(25)).await;
        tokio::task::yield_now().await;
    }
    assert!(drv.set_memory_spawned(0, 8192));
    let ev2 = wait_topic(&mut rx, "quota.exceeded", 3600).await;
    assert_eq!(
        serde_json::to_value(&ev2.payload).expect("payload")["usage_bytes"],
        8192
    );
}

#[tokio::test(start_paused = true)]
async fn quota_suspend_policy_suspends_offender() {
    let (d, drv) = mem_dispatcher(quota_config());
    let mut rx = d.kernel().subscribe();
    let pid = ok(&call(
        &d,
        "proc.spawn",
        json!({ "quotas": { "max_memory_bytes": 1024, "on_exceed": "suspend" } }),
    )
    .await)["pid"]
        .as_str()
        .expect("pid")
        .to_owned();

    assert!(drv.set_memory_spawned(0, 10_000));
    wait_topic(&mut rx, "quota.exceeded", 3600).await;
    // quota.exceeded 后紧跟 lifecycle: suspended
    let lc = wait_topic(&mut rx, "proc.lifecycle", 3600).await;
    assert_eq!(
        serde_json::to_value(&lc.payload).expect("payload")["state"],
        "suspended"
    );
    assert_eq!(proc_state(&d, &pid).await, "suspended");

    // 越界的挂起进程可恢复（操作员减负后手工放行）
    ok(&call(&d, "proc.resume", json!({ "pid": pid })).await);
}

#[tokio::test(start_paused = true)]
async fn quota_kill_policy_terminates_offender() {
    let (d, drv) = mem_dispatcher(quota_config());
    let mut rx = d.kernel().subscribe();
    let pid = ok(&call(
        &d,
        "proc.spawn",
        json!({ "quotas": { "max_memory_bytes": 1024, "on_exceed": "kill" } }),
    )
    .await)["pid"]
        .as_str()
        .expect("pid")
        .to_owned();

    assert!(drv.set_memory_spawned(0, 10_000));
    wait_topic(&mut rx, "quota.exceeded", 3600).await;
    let lc = wait_topic(&mut rx, "proc.lifecycle", 3600).await;
    assert_eq!(
        serde_json::to_value(&lc.payload).expect("payload")["state"],
        "terminated"
    );
    assert_eq!(proc_state(&d, &pid).await, "terminated");
}

#[tokio::test]
async fn quota_exceeded_lands_in_journal() {
    let (d, drv) = mem_dispatcher(quota_config());
    let mut rx = d.kernel().subscribe();
    ok(&call(
        &d,
        "proc.spawn",
        json!({ "quotas": { "max_memory_bytes": 1, "on_exceed": "warn" } }),
    )
    .await);
    assert!(drv.set_memory_spawned(0, 2));
    wait_topic(&mut rx, "quota.exceeded", 10).await;

    let journal = ok(&call(&d, "obs.journal", json!({ "limit": 50 })).await)["entries"].clone();
    let hit = journal.as_array().expect("entries").iter().any(|e| {
        e["subject"] == "kernel:quota" && e["method"] == "quota.exceeded" && e["kind"] == "deny"
    });
    assert!(hit, "quota violation must be journaled: {journal}");
}

#[tokio::test]
async fn high_quota_requires_dedicated_scope() {
    let (d, _drv) = mem_dispatcher(KernelConfig {
        quota_high_bytes: 1000,
        ..KernelConfig::default()
    });

    // 超过高水位：仅 proc:spawn 不够
    let who = limited("agent:worker", &["proc:spawn"]);
    let denied = call_as(
        &d,
        &who,
        "proc.spawn",
        json!({ "quotas": { "max_memory_bytes": 2000 } }),
    )
    .await;
    assert_eq!(err_code(&denied), "E_CAP_DENIED");

    // 低于高水位：proc:spawn 即可
    ok(&call_as(
        &d,
        &who,
        "proc.spawn",
        json!({ "quotas": { "max_memory_bytes": 500 } }),
    )
    .await);

    // 持 quota:high 后放行
    let vip = limited("agent:vip", &["proc:spawn", "quota:high"]);
    ok(&call_as(
        &d,
        &vip,
        "proc.spawn",
        json!({ "quotas": { "max_memory_bytes": 2000 } }),
    )
    .await);
}

// ================= Workflow Daemon =================

fn wf_spec(name: &str, steps: Value, scopes: Value) -> Value {
    json!({ "spec": {
        "name": name,
        "trigger": { "kind": "manual" },
        "steps": steps,
        "scopes": scopes,
    }})
}

#[tokio::test]
async fn wf_manual_run_executes_steps_and_emits_events() {
    let (d, _drv) = mem_dispatcher(KernelConfig::default());
    let mut rx = d.kernel().subscribe();
    ok(&call(
        &d,
        "wf.create",
        wf_spec(
            "healthcheck",
            json!([{ "method": "sys.info" }, { "method": "sys.info" }]),
            json!([]),
        ),
    )
    .await);

    let run = ok(&call(&d, "wf.run", json!({ "name": "healthcheck" })).await).clone();
    assert_eq!(run["ok"], true);
    assert_eq!(run["steps"], 2);

    // 事件序列：Started → StepOk×2 → Succeeded
    let started = wait_topic(&mut rx, "wf.run", 10).await;
    assert_eq!(wf_status(&started), WfRunStatus::Started);
    assert_eq!(
        wf_status(&wait_topic(&mut rx, "wf.run", 10).await),
        WfRunStatus::StepOk
    );
    assert_eq!(
        wf_status(&wait_topic(&mut rx, "wf.run", 10).await),
        WfRunStatus::StepOk
    );
    assert_eq!(
        wf_status(&wait_topic(&mut rx, "wf.run", 10).await),
        WfRunStatus::Succeeded
    );

    // wf.list 反映注册表；cancel 后移除
    let list = ok(&call(&d, "wf.list", json!({})).await).clone();
    assert_eq!(list["workflows"][0]["name"], "healthcheck");
    ok(&call(&d, "wf.cancel", json!({ "name": "healthcheck" })).await);
    let gone = call(&d, "wf.run", json!({ "name": "healthcheck" })).await;
    assert_eq!(err_code(&gone), "E_INVALID_ARG");
}

#[tokio::test]
async fn wf_create_rejects_invalid_specs() {
    let (d, _drv) = mem_dispatcher(KernelConfig::default());

    // 空步骤 / 坏名字 / 坏 cron
    let empty = call(&d, "wf.create", wf_spec("empty", json!([]), json!([]))).await;
    assert_eq!(err_code(&empty), "E_INVALID_ARG");
    let bad_name = call(
        &d,
        "wf.create",
        wf_spec("../up", json!([{ "method": "sys.info" }]), json!([])),
    )
    .await;
    assert_eq!(err_code(&bad_name), "E_INVALID_ARG");
    let bad_cron = call(
        &d,
        "wf.create",
        json!({ "spec": {
            "name": "cronbad",
            "trigger": { "kind": "cron", "expr": "not a cron" },
            "steps": [{ "method": "sys.info" }],
            "scopes": [],
        }}),
    )
    .await;
    assert_eq!(err_code(&bad_cron), "E_INVALID_ARG");

    // 重名
    ok(&call(
        &d,
        "wf.create",
        wf_spec("dup", json!([{ "method": "sys.info" }]), json!([])),
    )
    .await);
    let dup = call(
        &d,
        "wf.create",
        wf_spec("dup", json!([{ "method": "sys.info" }]), json!([])),
    )
    .await;
    assert_eq!(err_code(&dup), "E_INVALID_ARG");
}

#[tokio::test]
async fn wf_create_denies_scope_escalation() {
    let (d, _drv) = mem_dispatcher(KernelConfig::default());
    // 创建者只有 wf:manage —— 声明 proc:kill 即提权，必须拒绝
    let who = limited("agent:limited", &["wf:manage"]);
    let resp = call_as(
        &d,
        &who,
        "wf.create",
        wf_spec(
            "sneaky",
            json!([{ "method": "proc.kill" }]),
            json!(["proc:kill"]),
        ),
    )
    .await;
    assert_eq!(err_code(&resp), "E_CAP_DENIED");
}

#[tokio::test]
async fn wf_runner_is_least_privilege() {
    let (d, _drv) = mem_dispatcher(KernelConfig::default());
    // spec.scopes 为空 → 运行主体 wf:noscope 无任何作用域，步骤被鉴权拦下
    ok(&call(
        &d,
        "wf.create",
        wf_spec("noscope", json!([{ "method": "proc.list" }]), json!([])),
    )
    .await);
    let run = call(&d, "wf.run", json!({ "name": "noscope" })).await;
    assert_eq!(err_code(&run), "E_INTERNAL");

    // journal 记录的失败主体是受限的 wf:<name>，不是创建者
    let journal = ok(&call(&d, "obs.journal", json!({ "limit": 50 })).await)["entries"].clone();
    let hit = journal
        .as_array()
        .expect("entries")
        .iter()
        .any(|e| e["subject"] == "wf:noscope" && e["method"] == "proc.list");
    assert!(hit, "wf step must be journaled under wf subject: {journal}");
}

#[tokio::test(start_paused = true)]
async fn wf_step_retries_with_backoff_then_fails() {
    let (d, _drv) = mem_dispatcher(KernelConfig::default());
    let mut rx = d.kernel().subscribe();
    ok(&call(
        &d,
        "wf.create",
        wf_spec(
            "flaky",
            json!([{
                "method": "proc.info",
                "params": { "pid": "p-404" },
                "retry": { "max_attempts": 2, "backoff_ms": 5 },
            }]),
            json!(["proc:list"]),
        ),
    )
    .await);

    let run = call(&d, "wf.run", json!({ "name": "flaky" })).await;
    assert_eq!(err_code(&run), "E_INTERNAL");

    // Started → StepRetry×2 → Failed
    assert_eq!(
        wf_status(&wait_topic(&mut rx, "wf.run", 3600).await),
        WfRunStatus::Started
    );
    assert_eq!(
        wf_status(&wait_topic(&mut rx, "wf.run", 3600).await),
        WfRunStatus::StepRetry
    );
    assert_eq!(
        wf_status(&wait_topic(&mut rx, "wf.run", 3600).await),
        WfRunStatus::StepRetry
    );
    let failed = wait_topic(&mut rx, "wf.run", 3600).await;
    assert_eq!(wf_status(&failed), WfRunStatus::Failed);
}

#[tokio::test(start_paused = true)]
async fn wf_cron_fires_on_injected_clock() {
    // 受控时钟：2024-01-15（周一）10:30:00 UTC
    let now = Arc::new(AtomicU64::new(1_705_314_600));
    let clock_src = Arc::clone(&now);
    let driver = Arc::new(MockDriver::standard_fixture());
    let kernel = Kernel::new(Arc::clone(&driver) as _, KernelConfig::default());
    let d = Dispatcher::with_wf_clock(kernel, Arc::new(move || clock_src.load(Ordering::SeqCst)));

    let mut rx = d.kernel().subscribe();
    ok(&call(
        &d,
        "wf.create",
        json!({ "spec": {
            "name": "daily",
            "trigger": { "kind": "cron", "expr": "30 10 * * *" },
            "steps": [{ "method": "sys.info" }],
            "scopes": [],
        }}),
    )
    .await);

    // 首个 tick（30s 后）命中 10:30 → 运行一轮
    assert_eq!(
        wf_status(&wait_topic(&mut rx, "wf.run", 3600).await),
        WfRunStatus::Started
    );
    assert_eq!(
        wf_status(&wait_topic(&mut rx, "wf.run", 3600).await),
        WfRunStatus::StepOk
    );
    assert_eq!(
        wf_status(&wait_topic(&mut rx, "wf.run", 3600).await),
        WfRunStatus::Succeeded
    );

    // 次日同刻 → 第二轮（同一分钟内的后续 tick 已被去重）
    now.fetch_add(86_400, Ordering::SeqCst);
    assert_eq!(
        wf_status(&wait_topic(&mut rx, "wf.run", 3600).await),
        WfRunStatus::Started
    );
}

#[tokio::test]
async fn wf_event_trigger_runs_on_topic() {
    let (d, _drv) = mem_dispatcher(KernelConfig::default());
    ok(&call(
        &d,
        "wf.create",
        json!({ "spec": {
            "name": "on-proc",
            "trigger": { "kind": "event", "topic": "proc.lifecycle" },
            "steps": [{ "method": "sys.info" }],
            "scopes": [],
        }}),
    )
    .await);

    let mut rx = d.kernel().subscribe();
    // 任何 proc.lifecycle（如 spawn）触发一轮
    spawn_pid(&d).await;
    assert_eq!(
        wf_status(&wait_topic(&mut rx, "wf.run", 10).await),
        WfRunStatus::Started
    );
    assert_eq!(
        wf_status(&wait_topic(&mut rx, "wf.run", 10).await),
        WfRunStatus::StepOk
    );
    assert_eq!(
        wf_status(&wait_topic(&mut rx, "wf.run", 10).await),
        WfRunStatus::Succeeded
    );
}

// ================= Event Bus 背压 =================

#[tokio::test]
async fn bus_drops_hot_topics_under_backpressure_with_count() {
    let (d, _drv) = mem_dispatcher(KernelConfig {
        bus_capacity: 2,
        ..KernelConfig::default()
    });
    let mut rx = d.kernel().subscribe();
    let pid = spawn_pid(&d).await;

    let total_navs = 8;
    for i in 0..total_navs {
        ok(&call(
            &d,
            "nav.goto",
            json!({ "pid": pid, "url": format!("http://fixture.test/login?i={i}") }),
        )
        .await);
    }
    // nav 事件经 supervise 异步转发；让单线程运行时排空
    for _ in 0..100 {
        tokio::task::yield_now().await;
    }
    ok(&call(&d, "proc.kill", json!({ "pid": pid })).await);

    // 排空到 terminated 为止：统计 nav 交付数与 dropped 计数
    let mut delivered_nav = 0u64;
    let mut dropped_total = 0u64;
    loop {
        let ev = tokio::time::timeout(Duration::from_secs(5), rx.recv())
            .await
            .expect("drain timeout")
            .expect("bus closed");
        dropped_total += ev.dropped.unwrap_or(0);
        match ev.payload.topic() {
            "nav" => delivered_nav += 1,
            "proc.lifecycle"
                if serde_json::to_value(&ev.payload).expect("payload")["state"] == "terminated" =>
            {
                break;
            }
            _ => {}
        }
    }
    // 守恒：交付 + 丢弃 = 全部导航事件（net.request 也可丢，允许更大计数）
    assert!(
        delivered_nav + dropped_total >= total_navs,
        "nav conservation: delivered={delivered_nav} dropped={dropped_total}"
    );
    assert!(
        dropped_total >= 1,
        "capacity 2 with {total_navs} navs must drop: delivered={delivered_nav}"
    );
    assert!(
        delivered_nav < total_navs,
        "some navs must be shed: delivered={delivered_nav}"
    );
}

#[tokio::test]
async fn bus_never_drops_critical_topics() {
    let (d, _drv) = mem_dispatcher(KernelConfig {
        bus_capacity: 1,
        ..KernelConfig::default()
    });
    let mut rx = d.kernel().subscribe();

    // 生命周期事件同步发布：3×(spawn+kill)=6 条全部在消费前入队
    for _ in 0..3 {
        let pid = spawn_pid(&d).await;
        ok(&call(&d, "proc.kill", json!({ "pid": pid })).await);
    }

    let mut lifecycle = 0;
    for _ in 0..6 {
        let ev = tokio::time::timeout(Duration::from_secs(5), rx.recv())
            .await
            .expect("recv timeout")
            .expect("bus closed");
        if ev.payload.topic() == "proc.lifecycle" {
            assert_eq!(ev.dropped, None, "critical events must not report drops");
            lifecycle += 1;
        }
    }
    assert_eq!(lifecycle, 6, "capacity 1 must not shed lifecycle events");
}
