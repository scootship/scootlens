//! scootlens-kernel 集成测试（TDD）。
//!
//! 全部跑在 mock 驱动上（ADR-0006）；真实引擎只出现在 e2e。

use std::sync::Arc;
use std::time::Duration;

use scootlens_abi::ErrorCode;
use scootlens_driver_mock::{MockDriver, fixture_base};
use scootlens_hal::{InputAction, SnapshotOpts};
use scootlens_kernel::{BusEvent, BusPayload, Kernel, KernelConfig, ProcState};

fn kernel() -> Kernel {
    Kernel::new(
        Arc::new(MockDriver::standard_fixture()),
        KernelConfig::default(),
    )
}

fn kernel_with(config: KernelConfig) -> (Arc<MockDriver>, Kernel) {
    let driver = Arc::new(MockDriver::standard_fixture());
    (Arc::clone(&driver), Kernel::new(driver, config))
}

fn url(path: &str) -> url::Url {
    fixture_base().join(path).expect("valid path")
}

// ---------- 生命周期 ----------

#[tokio::test]
async fn spawn_creates_running_proc() {
    let k = kernel();
    let pid = k.spawn(Default::default()).await.expect("spawn");

    let info = k.info(&pid).await.expect("info");
    assert_eq!(info.pid, pid);
    assert_eq!(info.state, ProcState::Running);
    assert_eq!(info.engine, "mock");
    assert_eq!(info.profile, "default");

    let list = k.list().await;
    assert_eq!(list.len(), 1);
    assert_eq!(list[0].pid, pid);
}

#[tokio::test]
async fn pids_are_unique() {
    let k = kernel();
    let a = k.spawn(Default::default()).await.expect("spawn a");
    let b = k.spawn(Default::default()).await.expect("spawn b");
    assert_ne!(a, b);
}

#[tokio::test]
async fn kill_terminates_and_frees_slot() {
    let k = kernel();
    let pid = k.spawn(Default::default()).await.expect("spawn");
    k.kill(&pid).await.expect("kill");

    let info = k.info(&pid).await.expect("info");
    assert_eq!(info.state, ProcState::Terminated);

    // 已终止进程的引擎操作 → E_PROC_NOT_FOUND（句柄已回收）
    let err = k.page_info(&pid).await.expect_err("no engine ops");
    assert_eq!(err.code, ErrorCode::ProcNotFound);
}

#[tokio::test]
async fn unknown_pid_returns_proc_not_found() {
    let k = kernel();
    let ghost: scootlens_abi::Pid = "p-ghost".parse().expect("pid");
    assert_eq!(
        k.info(&ghost).await.expect_err("info").code,
        ErrorCode::ProcNotFound
    );
    assert_eq!(
        k.kill(&ghost).await.expect_err("kill").code,
        ErrorCode::ProcNotFound
    );
}

#[tokio::test]
async fn kill_is_idempotent_error_free_only_once() {
    let k = kernel();
    let pid = k.spawn(Default::default()).await.expect("spawn");
    k.kill(&pid).await.expect("kill once");
    // 第二次 kill：进程存在但已终止 → E_INVALID_ARG（非崩溃语义）
    let err = k.kill(&pid).await.expect_err("second kill");
    assert_eq!(err.code, ErrorCode::InvalidArg);
}

// ---------- 并发上限与排队 ----------

#[tokio::test]
async fn spawn_queues_beyond_concurrency_limit() {
    let (_d, k) = kernel_with(KernelConfig {
        max_procs: 2,
        ..Default::default()
    });
    let k = Arc::new(k);

    let p1 = k.spawn(Default::default()).await.expect("p1");
    let _p2 = k.spawn(Default::default()).await.expect("p2");

    // 第三个 spawn 必须排队
    let k2 = Arc::clone(&k);
    let third = tokio::spawn(async move { k2.spawn(Default::default()).await });
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert!(!third.is_finished(), "third spawn must be queued");

    // 释放一个槽位后完成
    k.kill(&p1).await.expect("kill");
    let p3 = tokio::time::timeout(Duration::from_secs(1), third)
        .await
        .expect("queued spawn completes")
        .expect("join")
        .expect("spawn ok");
    assert_eq!(k.info(&p3).await.expect("info").state, ProcState::Running);
}

// ---------- 引擎操作转发 ----------

#[tokio::test]
async fn full_flow_navigate_snapshot_act() {
    let k = kernel();
    let pid = k.spawn(Default::default()).await.expect("spawn");

    let nav = k.navigate(&pid, &url("/login")).await.expect("goto");
    assert_eq!(nav.title, "Login");

    let snap = k
        .snapshot(&pid, &SnapshotOpts::default())
        .await
        .expect("snapshot");
    let user = snap
        .find("textbox", "Username")
        .and_then(|n| n.elem_ref.clone())
        .expect("ref");

    let r = k
        .dispatch(
            &pid,
            &InputAction::Type {
                target: user,
                text: "alice".into(),
            },
        )
        .await
        .expect("type");
    assert!(!r.nav_occurred);

    let info = k.page_info(&pid).await.expect("page info");
    assert_eq!(info.url, url("/login"));
}

// ---------- 崩溃监督 ----------

#[tokio::test]
async fn crash_marks_proc_and_broadcasts_lifecycle() {
    let (driver, k) = kernel_with(KernelConfig::default());
    let pid = k.spawn(Default::default()).await.expect("spawn");
    let mut bus = k.subscribe();

    assert!(driver.crash_spawned(0), "inject crash");

    // 事件总线收到 lifecycle: crashed
    let evt = tokio::time::timeout(Duration::from_secs(1), async {
        loop {
            let e: BusEvent = bus.recv().await.expect("bus recv");
            if let BusPayload::ProcLifecycle { state, .. } = &e.payload {
                if *state == ProcState::Crashed {
                    break e;
                }
            }
        }
    })
    .await
    .expect("crash event within 1s");
    assert_eq!(evt.pid.as_ref(), Some(&pid));

    // 状态标记为 Crashed
    let info = k.info(&pid).await.expect("info");
    assert_eq!(info.state, ProcState::Crashed);

    // 对 Crashed 进程的引擎操作 → E_ENGINE_CRASH
    let err = k.page_info(&pid).await.expect_err("ops fail");
    assert_eq!(err.code, ErrorCode::EngineCrash);

    // kill 清理 Crashed 进程 → Terminated
    k.kill(&pid).await.expect("kill crashed");
    assert_eq!(
        k.info(&pid).await.expect("info").state,
        ProcState::Terminated
    );
}

// ---------- Event Bus ----------

#[tokio::test]
async fn bus_events_have_monotonic_seq_and_pid() {
    let k = kernel();
    let mut bus = k.subscribe();
    let pid = k.spawn(Default::default()).await.expect("spawn");
    k.navigate(&pid, &url("/")).await.expect("nav");
    k.navigate(&pid, &url("/login")).await.expect("nav2");

    let mut last_seq = 0u64;
    let mut nav_count = 0;
    while nav_count < 2 {
        let e = tokio::time::timeout(Duration::from_secs(1), bus.recv())
            .await
            .expect("bus event")
            .expect("recv");
        assert!(e.seq > last_seq, "seq monotonic");
        last_seq = e.seq;
        if matches!(e.payload, BusPayload::Navigated { .. }) {
            assert_eq!(e.pid.as_ref(), Some(&pid));
            nav_count += 1;
        }
    }
}

#[tokio::test]
async fn spawn_emits_lifecycle_running() {
    let k = kernel();
    let mut bus = k.subscribe();
    let pid = k.spawn(Default::default()).await.expect("spawn");

    let evt = tokio::time::timeout(Duration::from_secs(1), async {
        loop {
            let e: BusEvent = bus.recv().await.expect("recv");
            if let BusPayload::ProcLifecycle { state, .. } = &e.payload {
                if *state == ProcState::Running {
                    break e;
                }
            }
        }
    })
    .await
    .expect("running event");
    assert_eq!(evt.pid.as_ref(), Some(&pid));
}

// ---------- sys.info ----------

#[tokio::test]
async fn sys_info_reports_engine_and_counts() {
    let (_d, k) = kernel_with(KernelConfig {
        max_procs: 4,
        ..Default::default()
    });
    let _p = k.spawn(Default::default()).await.expect("spawn");

    let si = k.sys_info().await;
    assert_eq!(si.engine, "mock");
    assert_eq!(si.max_procs, 4);
    assert_eq!(si.running_procs, 1);
    assert!(si.caps.snapshot);
    assert_eq!(si.abi_version, scootlens_abi::ABI_VERSION);
}
