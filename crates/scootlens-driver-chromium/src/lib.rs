//! # scootlens-driver-chromium
//!
//! Chromium 引擎驱动（ADR-0002）：**外部进程 + CDP**。
//!
//! - 每个 proc 一个独立 headless Chromium 进程（`--remote-debugging-port=0`，
//!   临时 user-data-dir，进程退出即隔离回收）
//! - 薄 CDP 客户端（无 puppeteer/playwright 依赖），只用到的域：
//!   Target / Page / Runtime / DOM / Accessibility / Input
//! - 语义快照来自 `Accessibility.getFullAXTree`，剪枝后携带 `ElementRef`
//! - 崩溃检测：进程 wait + `Inspector.targetCrashed` + WS 断连，任一触发 `Crashed`
//!
//! 一致性：必须通过与 mock 驱动相同的 conformance 套件
//! （`tests/conformance.rs`，`#[ignore]` 标注，e2e 门禁跑 `--ignored`）。

mod cdp;
mod handle;
mod process;
mod snapshot;

use std::path::PathBuf;

use async_trait::async_trait;
use scootlens_abi::{AbiError, ErrorCode};
use scootlens_hal::{EngineCaps, EngineDriver, EngineHandle, HalResult, ProfileSpec};

/// Chromium 驱动。
pub struct ChromiumDriver {
    binary: PathBuf,
}

impl ChromiumDriver {
    /// 显式指定二进制路径。
    pub fn with_binary(binary: impl Into<PathBuf>) -> Self {
        Self {
            binary: binary.into(),
        }
    }

    /// 自动发现二进制（`SCOOTLENS_CHROMIUM_BIN` 环境变量优先，其次常见安装路径）。
    pub fn discover() -> HalResult<Self> {
        let binary = process::find_binary().ok_or_else(|| {
            AbiError::new(
                ErrorCode::Internal,
                "chromium binary not found; set SCOOTLENS_CHROMIUM_BIN",
            )
        })?;
        Ok(Self { binary })
    }
}

#[async_trait]
impl EngineDriver for ChromiumDriver {
    fn id(&self) -> &'static str {
        "chromium"
    }

    fn capabilities(&self) -> EngineCaps {
        EngineCaps {
            snapshot: true,
            screenshot: true,
            input: true,
            eval: true,
            net_rules: false, // P2
            state: false,     // P2
            events: true,
            metrics: false, // P2
        }
    }

    async fn spawn(&self, _profile: &ProfileSpec) -> HalResult<Box<dyn EngineHandle>> {
        let proc = process::BrowserProcess::launch(&self.binary).await?;
        let conn = cdp::CdpConn::connect(&proc.ws_url).await?;
        let handle = handle::ChromiumHandle::boot(proc, conn).await?;
        Ok(Box::new(handle))
    }
}
