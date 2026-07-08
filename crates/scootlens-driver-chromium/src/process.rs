//! Chromium 外部进程管理（ADR-0002）：启动、DevTools WS 端点发现、终止。

use std::path::PathBuf;
use std::process::Stdio;
use std::time::Duration;

use scootlens_abi::{AbiError, ErrorCode};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::{Child, Command};

/// 运行中的浏览器进程。
pub(crate) struct BrowserProcess {
    pub child: Child,
    pub ws_url: String,
    /// 临时 profile 目录；Drop 即清理。
    _user_data_dir: tempfile::TempDir,
}

impl BrowserProcess {
    /// 启动 headless Chromium 并解析 stderr 中的 DevTools 端点。
    pub async fn launch(binary: &PathBuf) -> Result<Self, AbiError> {
        let user_data_dir = tempfile::tempdir()
            .map_err(|e| AbiError::new(ErrorCode::Internal, format!("tempdir: {e}")))?;

        let mut cmd = Command::new(binary);
        cmd.arg("--headless=new")
            .arg("--remote-debugging-port=0")
            .arg(format!(
                "--user-data-dir={}",
                user_data_dir.path().display()
            ))
            .arg("--no-first-run")
            .arg("--no-default-browser-check")
            .arg("--disable-background-networking")
            .arg("--disable-extensions")
            .arg("--disable-sync")
            .arg("--mute-audio")
            .arg(format!(
                "--window-size={},{}",
                crate::handle::VIEWPORT_WIDTH,
                crate::handle::VIEWPORT_HEIGHT
            ));
        // CI 容器等环境的附加启动参数（如 --no-sandbox），空白分隔。
        if let Ok(extra) = std::env::var("SCOOTLENS_CHROMIUM_EXTRA_ARGS") {
            for a in extra.split_whitespace() {
                cmd.arg(a);
            }
        }
        let mut child = cmd
            .arg("about:blank")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .map_err(|e| {
                AbiError::new(
                    ErrorCode::Internal,
                    format!("failed to launch chromium at {}: {e}", binary.display()),
                )
            })?;

        let stderr = child
            .stderr
            .take()
            .ok_or_else(|| AbiError::new(ErrorCode::Internal, "no stderr pipe"))?;

        let ws_url = tokio::time::timeout(
            Duration::from_secs(15),
            discover_ws_url(BufReader::new(stderr)),
        )
        .await
        .map_err(|_| {
            AbiError::new(
                ErrorCode::Timeout,
                "chromium did not report DevTools endpoint within 15s",
            )
        })??;

        Ok(Self {
            child,
            ws_url,
            _user_data_dir: user_data_dir,
        })
    }
}

/// 从 stderr 行流中解析 `DevTools listening on ws://…`。
async fn discover_ws_url(
    mut reader: BufReader<tokio::process::ChildStderr>,
) -> Result<String, AbiError> {
    let mut line = String::new();
    loop {
        line.clear();
        let n = reader
            .read_line(&mut line)
            .await
            .map_err(|e| AbiError::new(ErrorCode::Internal, format!("read stderr: {e}")))?;
        if n == 0 {
            return Err(AbiError::new(
                ErrorCode::EngineCrash,
                "chromium exited before reporting DevTools endpoint",
            ));
        }
        if let Some(rest) = line.trim().strip_prefix("DevTools listening on ") {
            // 之后的 stderr 交给后台任务排空，防止管道写满阻塞浏览器
            tokio::spawn(drain_stderr(reader));
            return Ok(rest.trim().to_owned());
        }
    }
}

async fn drain_stderr(mut reader: BufReader<tokio::process::ChildStderr>) {
    let mut line = String::new();
    loop {
        line.clear();
        match reader.read_line(&mut line).await {
            Ok(0) | Err(_) => break,
            Ok(_) => tracing::trace!(target: "chromium_stderr", "{}", line.trim_end()),
        }
    }
}

/// 定位 Chromium 可执行文件：环境变量优先，其次常见安装路径。
pub(crate) fn find_binary() -> Option<PathBuf> {
    if let Ok(p) = std::env::var("SCOOTLENS_CHROMIUM_BIN") {
        let p = PathBuf::from(p);
        return p.is_file().then_some(p);
    }
    let candidates: &[&str] = &[
        "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome",
        "/Applications/Chromium.app/Contents/MacOS/Chromium",
        "/usr/bin/google-chrome",
        "/usr/bin/google-chrome-stable",
        "/usr/bin/chromium",
        "/usr/bin/chromium-browser",
        "/snap/bin/chromium",
    ];
    candidates.iter().map(PathBuf::from).find(|p| p.is_file())
}
