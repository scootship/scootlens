//! scootlens-mcp：stdio MCP server → ScootLens gateway 薄代理（ADR-0005）。
//!
//! MCP 客户端（Claude Desktop / 任意 MCP host）spawn 本进程；本进程以
//! capability 令牌连接 scootlensd gateway，把工具调用逐一转发为 ABI 调用。
//! 权限模型单点在内核——本层零授权能力。

use clap::Parser;
use rmcp::ServiceExt as _;
use scootlens_mcp::{AbiClient, ScootLensMcp};

#[derive(Parser)]
#[command(
    name = "scootlens-mcp",
    version,
    about = "ScootLens MCP server: stdio <-> gateway ABI projection"
)]
struct Args {
    /// Gateway WS 端点。
    #[arg(long, env = "SCOOTLENS_URL", default_value = "ws://127.0.0.1:9910/ws")]
    url: String,

    /// Capability 令牌（slt1…）。作用域即本 MCP 会话的全部权限上限。
    #[arg(long, env = "SCOOTLENS_TOKEN")]
    token: String,
}

#[tokio::main]
async fn main() -> Result<(), String> {
    // stdout 是 MCP 协议通道；日志一律走 stderr
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .init();

    let args = Args::parse();
    let sep = if args.url.contains('?') { '&' } else { '?' };
    let url = format!("{}{sep}token={}", args.url, args.token);
    let client = AbiClient::connect(&url)
        .await
        .map_err(|e| format!("gateway: {e}"))?;
    tracing::info!(url = %args.url, "connected to scootlensd gateway");

    let service = ScootLensMcp::new(client)
        .serve(rmcp::transport::io::stdio())
        .await
        .map_err(|e| format!("mcp serve: {e}"))?;
    service.waiting().await.map_err(|e| format!("mcp: {e}"))?;
    Ok(())
}
