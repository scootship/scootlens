//! Chromium 驱动 × HAL conformance 套件（docs/05-engine-hal.md）。
//!
//! 需要本机 Chromium（`SCOOTLENS_CHROMIUM_BIN` 或常见安装路径），
//! 因此全部 `#[ignore]`：本地/CI e2e job 用 `cargo test -- --ignored` 跑。
//! mock 驱动用 `run_all!` 宏注册同一套检查；此处手写以便挂 ignore 属性，
//! 并让每个测试自持 fixtures 站点的生命周期。

use scootlens_driver_chromium::ChromiumDriver;
use scootlens_hal::conformance::{self, Target};
use scootlens_test_support::FixtureSite;

async fn target() -> (Target, FixtureSite) {
    let site = FixtureSite::start_default().await.expect("fixture site");
    let driver = ChromiumDriver::discover().expect("chromium binary");
    let base_url = site.base_url().parse().expect("base url");
    (
        Target {
            driver: Box::new(driver),
            base_url,
        },
        site,
    )
}

macro_rules! conformance_case {
    ($name:ident) => {
        #[tokio::test]
        #[ignore = "requires chromium binary; run with --ignored in e2e job"]
        async fn $name() {
            let (t, _site) = target().await;
            conformance::$name(&t).await;
        }
    };
}

conformance_case!(navigation_reports_url_and_title);
conformance_case!(snapshot_generation_increments);
conformance_case!(snapshot_contains_interactive_refs);
conformance_case!(stale_ref_is_rejected);
conformance_case!(click_navigates);
conformance_case!(type_updates_value);
conformance_case!(form_flow_completes);
conformance_case!(state_roundtrip_or_unsupported);
conformance_case!(history_and_reload_work);
