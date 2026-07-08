//! Mock 驱动测试：conformance 注册 + mock 特有行为（故障注入/截断/编程接口）。

use scootlens_abi::ErrorCode;
use scootlens_driver_mock::{MockDriver, NodeModel, PageModel, SiteBuilder, fixture_base};
use scootlens_hal::{
    EngineCaps, EngineDriver, EngineEvent, EngineHandle, InputAction, ProfileSpec, SnapshotOpts,
    conformance,
};
use serde_json::json;

// ---------- HAL 一致性套件 ----------

scootlens_hal::conformance::run_all!(|| async {
    conformance::Target {
        driver: Box::new(MockDriver::standard_fixture()) as Box<dyn EngineDriver>,
        base_url: fixture_base(),
    }
});

// ---------- mock 特有行为 ----------

async fn spawn_fixture() -> Box<dyn EngineHandle> {
    MockDriver::standard_fixture()
        .spawn(&ProfileSpec::default())
        .await
        .expect("spawn")
}

#[tokio::test]
async fn driver_reports_identity_and_caps() {
    let d = MockDriver::standard_fixture();
    assert_eq!(d.id(), "mock");
    let caps = d.capabilities();
    assert!(caps.snapshot && caps.input && caps.state && caps.events);
    assert!(!caps.net_rules);
}

#[tokio::test]
async fn crash_injection_breaks_calls_and_emits_event() {
    let d = MockDriver::standard_fixture();
    let h = d.spawn_mock(&ProfileSpec::default());
    let mut events = EngineHandle::events(&h);

    h.navigate(&fixture_base()).await.expect("navigate");
    h.inject_crash();

    let err = h.page_info().await.expect_err("crashed engine must fail");
    assert_eq!(err.code, ErrorCode::EngineCrash);

    // 事件流中应能观察到 Crashed
    let mut saw_crash = false;
    while let Ok(ev) = events.try_recv() {
        if matches!(ev, EngineEvent::Crashed) {
            saw_crash = true;
        }
    }
    assert!(saw_crash);
}

#[tokio::test]
async fn eval_is_programmable_and_defaults_to_null() {
    let d = MockDriver::standard_fixture();
    let h = d.spawn_mock(&ProfileSpec::default());
    h.navigate(&fixture_base()).await.expect("navigate");

    assert_eq!(
        h.eval("document.title", &[]).await.expect("eval"),
        json!(null)
    );
    h.program_eval("document.title", json!("Fixture Home"));
    assert_eq!(
        h.eval("document.title", &[]).await.expect("eval"),
        json!("Fixture Home")
    );
}

#[tokio::test]
async fn unknown_url_resolves_to_404_page() {
    let h = spawn_fixture().await;
    let nav = h
        .navigate(&fixture_base().join("/nope").expect("url"))
        .await
        .expect("navigate");
    assert_eq!(nav.title, "Not Found");
    let s = h.snapshot(&SnapshotOpts::default()).await.expect("snap");
    assert!(s.find("heading", "404 Not Found").is_some());
}

#[tokio::test]
async fn snapshot_truncates_at_max_nodes() {
    let base = fixture_base();
    let mut page = PageModel::document("Big");
    for i in 0..50 {
        page = page.child(NodeModel::group(
            &format!("g{i}"),
            vec![NodeModel::text(&format!("t{i}"))],
        ));
    }
    let site = SiteBuilder::new(base.clone()).page("/", page).build();
    let d = MockDriver::new(site);
    let h = d.spawn(&ProfileSpec::default()).await.expect("spawn");
    h.navigate(&base).await.expect("navigate");

    let s = h
        .snapshot(&SnapshotOpts { max_nodes: 10 })
        .await
        .expect("snap");
    assert!(s.truncated);
    let text = s.to_compact_text();
    assert!(text.ends_with("… (truncated)\n"));
}

#[tokio::test]
async fn compact_text_format_is_locked() {
    let h = spawn_fixture().await;
    h.navigate(&fixture_base().join("/login").expect("url"))
        .await
        .expect("navigate");
    let s = h.snapshot(&SnapshotOpts::default()).await.expect("snap");
    // 输出格式是 LLM 消费面，golden 锁定
    insta::assert_snapshot!("login_compact_text", s.to_compact_text());
}

#[tokio::test]
async fn disabled_caps_return_unsupported() {
    let caps = EngineCaps {
        snapshot: false,
        screenshot: false,
        input: false,
        eval: false,
        net_rules: false,
        state: false,
        events: true,
        metrics: true,
    };
    let d = MockDriver::standard_fixture().with_caps(caps);
    let h = d.spawn(&ProfileSpec::default()).await.expect("spawn");
    h.navigate(&fixture_base())
        .await
        .expect("navigate is core, always works");

    let snap_err = h
        .snapshot(&SnapshotOpts::default())
        .await
        .expect_err("snap");
    assert_eq!(snap_err.code, ErrorCode::Unsupported);
    let shot_err = h.screenshot().await.expect_err("screenshot");
    assert_eq!(shot_err.code, ErrorCode::Unsupported);
    let eval_err = h.eval("1+1", &[]).await.expect_err("eval");
    assert_eq!(eval_err.code, ErrorCode::Unsupported);
    let state_err = h.export_state().await.expect_err("state");
    assert_eq!(state_err.code, ErrorCode::Unsupported);
}

#[tokio::test]
async fn typing_appends_across_calls() {
    let h = spawn_fixture().await;
    h.navigate(&fixture_base().join("/login").expect("url"))
        .await
        .expect("navigate");
    let s = h.snapshot(&SnapshotOpts::default()).await.expect("snap");
    let user = s
        .find("textbox", "Username")
        .and_then(|n| n.elem_ref.clone())
        .expect("ref");
    h.dispatch(&InputAction::Type {
        target: user.clone(),
        text: "ali".into(),
    })
    .await
    .expect("type1");
    h.dispatch(&InputAction::Type {
        target: user,
        text: "ce".into(),
    })
    .await
    .expect("type2");
    let s2 = h.snapshot(&SnapshotOpts::default()).await.expect("snap2");
    assert_eq!(
        s2.find("textbox", "Username")
            .and_then(|n| n.value.as_deref()),
        Some("alice")
    );
}

#[tokio::test]
async fn metrics_and_shutdown_work() {
    let h = spawn_fixture().await;
    let m = h.metrics().await.expect("metrics");
    assert!(m.memory_bytes > 0);
    h.shutdown().await.expect("shutdown");
}
