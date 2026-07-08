//! 跨驱动一致性测试套件（docs/05-engine-hal.md）。
//!
//! 同一套检查跑所有驱动。驱动 crate 中一行注册：
//!
//! ```ignore
//! scootlens_hal::conformance::run_all!(|| async {
//!     scootlens_hal::conformance::Target { driver: Box::new(MyDriver::new()), base_url: fixture_base() }
//! });
//! ```
//!
//! # 标准 fixture 站点语义
//!
//! 驱动必须让 `base_url` 上存在如下站点（mock 内建；真实引擎由本地 fixtures 站点提供）：
//!
//! - `/`：document "Fixture Home"，含 heading "Fixture Home" 与 link "Go to Login"（点击 → `/login`）
//! - `/login`：document "Login"，含 textbox "Username"、textbox "Password"、button "Sign in"（点击 → `/welcome`）
//! - `/welcome`：document "Welcome"，含 heading "Welcome"

use scootlens_abi::ErrorCode;
use url::Url;

use crate::{
    EngineDriver, EngineHandle, HistoryDir, InputAction, ProfileSpec, SnapshotOpts, StateBundle,
};

/// 一致性测试目标：驱动 + 标准 fixture 站点基地址。
pub struct Target {
    pub driver: Box<dyn EngineDriver>,
    pub base_url: Url,
}

impl Target {
    async fn spawn(&self) -> Box<dyn EngineHandle> {
        self.driver
            .spawn(&ProfileSpec::default())
            .await
            .expect("conformance: spawn must succeed")
    }

    fn url(&self, path: &str) -> Url {
        self.base_url.join(path).expect("conformance: valid path")
    }
}

/// 导航返回正确的 url 与非空标题。
pub async fn navigation_reports_url_and_title(t: &Target) {
    let h = t.spawn().await;
    let nav = h.navigate(&t.url("/")).await.expect("navigate");
    assert_eq!(nav.url, t.url("/"));
    assert_eq!(nav.title, "Fixture Home");
    let info = h.page_info().await.expect("page_info");
    assert_eq!(info.url, t.url("/"));
}

/// 快照代数单调递增。
pub async fn snapshot_generation_increments(t: &Target) {
    let h = t.spawn().await;
    h.navigate(&t.url("/")).await.expect("navigate");
    let s1 = h.snapshot(&SnapshotOpts::default()).await.expect("snap1");
    let s2 = h.snapshot(&SnapshotOpts::default()).await.expect("snap2");
    assert_eq!(s2.generation, s1.generation + 1);
}

/// 快照中的交互节点携带元素引用。
pub async fn snapshot_contains_interactive_refs(t: &Target) {
    let h = t.spawn().await;
    h.navigate(&t.url("/")).await.expect("navigate");
    let s = h.snapshot(&SnapshotOpts::default()).await.expect("snap");
    let link = s.find("link", "Go to Login").expect("link present");
    assert!(link.elem_ref.is_some(), "interactive node must carry ref");
    let heading = s.find("heading", "Fixture Home").expect("heading present");
    assert!(
        heading.elem_ref.is_none(),
        "non-interactive node carries no ref"
    );
}

/// 对过期代数的 ref 操作返回 E_REF_STALE。
pub async fn stale_ref_is_rejected(t: &Target) {
    let h = t.spawn().await;
    h.navigate(&t.url("/")).await.expect("navigate");
    let s1 = h.snapshot(&SnapshotOpts::default()).await.expect("snap1");
    let old_ref = s1
        .find("link", "Go to Login")
        .and_then(|n| n.elem_ref.clone())
        .expect("ref");
    h.snapshot(&SnapshotOpts::default()).await.expect("snap2");
    let err = h
        .dispatch(&InputAction::Click { target: old_ref })
        .await
        .expect_err("stale ref must fail");
    assert_eq!(err.code, ErrorCode::RefStale);
}

/// 点击链接引发导航。
pub async fn click_navigates(t: &Target) {
    let h = t.spawn().await;
    h.navigate(&t.url("/")).await.expect("navigate");
    let s = h.snapshot(&SnapshotOpts::default()).await.expect("snap");
    let link = s
        .find("link", "Go to Login")
        .and_then(|n| n.elem_ref.clone())
        .expect("ref");
    let r = h
        .dispatch(&InputAction::Click { target: link })
        .await
        .expect("click");
    assert!(r.nav_occurred);
    assert_eq!(h.page_info().await.expect("info").url, t.url("/login"));
}

/// 输入文本在下一次快照中可见。
pub async fn type_updates_value(t: &Target) {
    let h = t.spawn().await;
    h.navigate(&t.url("/login")).await.expect("navigate");
    let s = h.snapshot(&SnapshotOpts::default()).await.expect("snap");
    let user = s
        .find("textbox", "Username")
        .and_then(|n| n.elem_ref.clone())
        .expect("ref");
    h.dispatch(&InputAction::Type {
        target: user,
        text: "alice".into(),
    })
    .await
    .expect("type");
    let s2 = h.snapshot(&SnapshotOpts::default()).await.expect("snap2");
    let val = s2.find("textbox", "Username").and_then(|n| n.value.clone());
    assert_eq!(val.as_deref(), Some("alice"));
}

/// 完整表单流：填写 → 提交 → 到达 welcome。
pub async fn form_flow_completes(t: &Target) {
    let h = t.spawn().await;
    h.navigate(&t.url("/login")).await.expect("navigate");
    let s = h.snapshot(&SnapshotOpts::default()).await.expect("snap");
    for (field, text) in [("Username", "alice"), ("Password", "s3cret")] {
        let r = s
            .find("textbox", field)
            .and_then(|n| n.elem_ref.clone())
            .expect("field ref");
        h.dispatch(&InputAction::Type {
            target: r,
            text: text.into(),
        })
        .await
        .expect("type");
    }
    let s2 = h.snapshot(&SnapshotOpts::default()).await.expect("snap2");
    let submit = s2
        .find("button", "Sign in")
        .and_then(|n| n.elem_ref.clone())
        .expect("submit ref");
    let r = h
        .dispatch(&InputAction::Click { target: submit })
        .await
        .expect("click");
    assert!(r.nav_occurred);
    let s3 = h.snapshot(&SnapshotOpts::default()).await.expect("snap3");
    assert!(s3.find("heading", "Welcome").is_some());
}

/// 状态导入导出往返；无 state 能力的驱动必须返回 E_UNSUPPORTED。
pub async fn state_roundtrip_or_unsupported(t: &Target) {
    let h = t.spawn().await;
    h.navigate(&t.url("/")).await.expect("navigate");
    let mut bundle = StateBundle::default();
    bundle
        .entries
        .insert("cookie:sid".into(), serde_json::json!("abc123"));

    if t.driver.capabilities().state {
        h.import_state(&bundle).await.expect("import");
        let out = h.export_state().await.expect("export");
        assert_eq!(
            out.entries.get("cookie:sid"),
            Some(&serde_json::json!("abc123"))
        );
    } else {
        let err = h
            .import_state(&bundle)
            .await
            .expect_err("must be unsupported");
        assert_eq!(err.code, ErrorCode::Unsupported);
    }
}

/// history back/forward 与 reload 语义。
pub async fn history_and_reload_work(t: &Target) {
    let h = t.spawn().await;
    h.navigate(&t.url("/")).await.expect("nav home");
    h.navigate(&t.url("/login")).await.expect("nav login");

    let back = h.history(HistoryDir::Back).await.expect("back");
    assert_eq!(back.url, t.url("/"));
    let fwd = h.history(HistoryDir::Forward).await.expect("forward");
    assert_eq!(fwd.url, t.url("/login"));
    // 历史耗尽：no-op，停留当前页
    let fwd2 = h.history(HistoryDir::Forward).await.expect("forward eol");
    assert_eq!(fwd2.url, t.url("/login"));

    let re = h.reload().await.expect("reload");
    assert_eq!(re.url, t.url("/login"));
    assert_eq!(re.title, "Login");
}

/// 注册全部一致性测试为 `#[tokio::test]`。
///
/// 参数为返回 `Target` 的异步工厂闭包表达式；每个测试独立创建 Target。
#[macro_export]
macro_rules! conformance_run_all {
    ($factory:expr) => {
        mod hal_conformance {
            use super::*;

            #[tokio::test]
            async fn navigation_reports_url_and_title() {
                let t = ($factory)().await;
                $crate::conformance::navigation_reports_url_and_title(&t).await;
            }

            #[tokio::test]
            async fn snapshot_generation_increments() {
                let t = ($factory)().await;
                $crate::conformance::snapshot_generation_increments(&t).await;
            }

            #[tokio::test]
            async fn snapshot_contains_interactive_refs() {
                let t = ($factory)().await;
                $crate::conformance::snapshot_contains_interactive_refs(&t).await;
            }

            #[tokio::test]
            async fn stale_ref_is_rejected() {
                let t = ($factory)().await;
                $crate::conformance::stale_ref_is_rejected(&t).await;
            }

            #[tokio::test]
            async fn click_navigates() {
                let t = ($factory)().await;
                $crate::conformance::click_navigates(&t).await;
            }

            #[tokio::test]
            async fn type_updates_value() {
                let t = ($factory)().await;
                $crate::conformance::type_updates_value(&t).await;
            }

            #[tokio::test]
            async fn form_flow_completes() {
                let t = ($factory)().await;
                $crate::conformance::form_flow_completes(&t).await;
            }

            #[tokio::test]
            async fn state_roundtrip_or_unsupported() {
                let t = ($factory)().await;
                $crate::conformance::state_roundtrip_or_unsupported(&t).await;
            }

            #[tokio::test]
            async fn history_and_reload_work() {
                let t = ($factory)().await;
                $crate::conformance::history_and_reload_work(&t).await;
            }
        }
    };
}

pub use conformance_run_all as run_all;
