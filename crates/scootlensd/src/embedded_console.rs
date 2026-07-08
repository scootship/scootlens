//! 嵌入式 Web Console（`embed-console` feature）：
//! 构建时把 `console/dist` 打进二进制，运行时在 `/` 直接托管，无需 `--console-dir`。

use axum::http::{StatusCode, Uri, header};
use axum::response::{IntoResponse, Response};
use include_dir::{Dir, include_dir};

static DIST: Dir<'_> = include_dir!("$CARGO_MANIFEST_DIR/../../console/dist");

/// 兜底路由处理器：按 URI 路径从嵌入资源中取文件，`/` 映射到 `index.html`。
pub async fn serve(uri: Uri) -> Response {
    let path = uri.path().trim_start_matches('/');
    let path = if path.is_empty() { "index.html" } else { path };
    match DIST.get_file(path) {
        Some(file) => {
            let mime = mime_guess::from_path(path).first_or_octet_stream();
            (
                [(header::CONTENT_TYPE, mime.essence_str().to_owned())],
                file.contents(),
            )
                .into_response()
        }
        None => (StatusCode::NOT_FOUND, "not found").into_response(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dist_contains_index() {
        assert!(DIST.get_file("index.html").is_some(), "console/dist 未构建");
    }

    #[tokio::test]
    async fn serves_index_at_root() {
        let resp = serve(Uri::from_static("/")).await;
        assert_eq!(resp.status(), StatusCode::OK);
        assert_eq!(
            resp.headers()[header::CONTENT_TYPE.as_str()],
            "text/html".parse::<axum::http::HeaderValue>().expect("hv")
        );
    }

    #[tokio::test]
    async fn unknown_path_is_404() {
        let resp = serve(Uri::from_static("/no-such-file.xyz")).await;
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }
}
