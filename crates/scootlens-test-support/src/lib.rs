//! # scootlens-test-support
//!
//! 测试基建。当前唯一职责：把 `fixtures/site/` 起成本地静态站点，
//! 供 chromium 驱动 conformance 与 e2e 测试访问。
//!
//! 语义必须与 `MockDriver::standard_fixture()` 保持一致
//! （同一套 conformance 断言要在两种驱动上通过）。

use std::net::SocketAddr;
use std::path::{Path, PathBuf};

use axum::Router;
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::get;

/// 运行中的 fixture 站点。Drop 即停。
pub struct FixtureSite {
    addr: SocketAddr,
    task: tokio::task::JoinHandle<()>,
}

impl FixtureSite {
    /// 在随机端口起站点。`root` 是 fixtures/site 目录。
    pub async fn start(root: impl Into<PathBuf>) -> std::io::Result<Self> {
        let root = root.into();
        let app = Router::new()
            .route("/", get(page("index.html")))
            .route("/login", get(page("login.html")))
            .route("/welcome", get(page("welcome.html")))
            .with_state(root);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
        let addr = listener.local_addr()?;
        let task = tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });
        Ok(Self { addr, task })
    }

    /// 从仓库根定位 `fixtures/site` 并启动（测试便捷入口）。
    pub async fn start_default() -> std::io::Result<Self> {
        Self::start(workspace_fixtures()).await
    }

    pub fn base_url(&self) -> String {
        format!("http://{}", self.addr)
    }

    pub fn url(&self, path: &str) -> String {
        format!("http://{}{}", self.addr, path)
    }
}

impl Drop for FixtureSite {
    fn drop(&mut self) {
        self.task.abort();
    }
}

/// 仓库根的 fixtures/site（依赖 CARGO_MANIFEST_DIR 定位，不依赖 cwd）。
pub fn workspace_fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("workspace root")
        .join("fixtures/site")
}

fn page(
    file: &'static str,
) -> impl Fn(
    axum::extract::State<PathBuf>,
) -> std::pin::Pin<Box<dyn Future<Output = Response> + Send>>
+ Clone {
    move |axum::extract::State(root): axum::extract::State<PathBuf>| {
        Box::pin(async move {
            match tokio::fs::read_to_string(root.join(file)).await {
                Ok(body) => (
                    [(header::CONTENT_TYPE, "text/html; charset=utf-8")],
                    body,
                )
                    .into_response(),
                Err(e) => {
                    (StatusCode::NOT_FOUND, format!("{file}: {e}")).into_response()
                }
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn serves_all_three_pages() {
        let site = FixtureSite::start_default().await.expect("start");
        for (path, needle) in [
            ("/", "Go to Login"),
            ("/login", "Sign in"),
            ("/welcome", "<h1>Welcome</h1>"),
        ] {
            let body = http_get(&site.url(path)).await;
            assert!(body.contains(needle), "{path} missing {needle}: {body}");
        }
    }

    #[tokio::test]
    async fn unknown_path_is_404() {
        let site = FixtureSite::start_default().await.expect("start");
        let (status, _) = http_get_status(&site.url("/nope")).await;
        assert_eq!(status, 404);
    }

    async fn http_get(url: &str) -> String {
        http_get_status(url).await.1
    }

    /// 最小 HTTP GET（避免为测试引入 http 客户端依赖）。
    async fn http_get_status(url: &str) -> (u16, String) {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let rest = url.strip_prefix("http://").expect("http url");
        let (host, path) = rest.split_once('/').expect("path");
        let mut conn = tokio::net::TcpStream::connect(host).await.expect("connect");
        let req = format!("GET /{path} HTTP/1.0\r\nHost: {host}\r\n\r\n");
        conn.write_all(req.as_bytes()).await.expect("write");
        let mut buf = String::new();
        conn.read_to_string(&mut buf).await.expect("read");
        let status: u16 = buf
            .split_whitespace()
            .nth(1)
            .and_then(|s| s.parse().ok())
            .expect("status");
        let body = buf
            .split_once("\r\n\r\n")
            .map(|(_, b)| b.to_owned())
            .unwrap_or_default();
        (status, body)
    }
}
