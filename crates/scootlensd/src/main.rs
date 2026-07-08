//! scootlensd：守护进程组装点（driver → kernel → dispatcher → gateway）。

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use clap::{Parser, ValueEnum};
use scootlens_abi::{ApprovalMode, TokenClaims, TokenConstraints};
use scootlens_driver_chromium::ChromiumDriver;
use scootlens_driver_mock::MockDriver;
use scootlens_gateway::{Gateway, GatewayConfig};
use scootlens_hal::EngineDriver;
use scootlens_kernel::{Dispatcher, Kernel, KernelConfig};

#[derive(Debug, Clone, Copy, ValueEnum)]
enum Engine {
    Chromium,
    Mock,
}

#[derive(Parser)]
#[command(
    name = "scootlensd",
    version,
    about = "ScootLens daemon: web sessions as processes, one JSON-RPC ABI"
)]
struct Args {
    /// 监听地址。
    #[arg(long, default_value = "127.0.0.1:9910")]
    listen: String,

    /// 浏览器引擎。
    #[arg(long, value_enum, default_value_t = Engine::Chromium)]
    engine: Engine,

    /// 状态目录（密钥/journal/vault/downloads/uploads）。缺省 = 纯内存模式。
    #[arg(long, env = "SCOOTLENS_STATE_DIR")]
    state_dir: Option<PathBuf>,

    /// Web Console 静态目录；设置后在 `/` 托管。
    #[arg(long)]
    console_dir: Option<PathBuf>,

    /// 最大并发引擎进程数。
    #[arg(long, default_value_t = 8)]
    max_procs: usize,
}

fn main() -> Result<(), String> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .init();

    let args = Args::parse();
    let runtime = tokio::runtime::Runtime::new().map_err(|e| format!("runtime: {e}"))?;
    runtime.block_on(run(args))
}

async fn run(args: Args) -> Result<(), String> {
    let driver: Arc<dyn EngineDriver> = match args.engine {
        Engine::Mock => Arc::new(MockDriver::standard_fixture()),
        Engine::Chromium => Arc::new(ChromiumDriver::discover().map_err(|e| e.to_string())?),
    };

    let config = KernelConfig {
        max_procs: args.max_procs,
        state_dir: args.state_dir.clone(),
        ..KernelConfig::default()
    };
    let kernel = Kernel::open(driver, config).map_err(|e| format!("kernel: {e}"))?;

    // 管理员令牌：全作用域 + 全自动审批。仅打印一次，不落盘。
    let mut constraints = TokenConstraints::default();
    constraints.approval.insert("*".into(), ApprovalMode::Auto);
    let admin = TokenClaims {
        subject: "user:admin".into(),
        scopes: vec!["*".parse().map_err(|e| format!("{e}"))?],
        constraints,
        issued_by: "scootlensd".into(),
        issued_at: SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or_default(),
    };
    let token = kernel.security().issue(&admin);
    println!("admin token: {token}");

    let gateway = Gateway::new(
        Dispatcher::new(kernel),
        GatewayConfig {
            console_dir: args.console_dir,
        },
    );

    let listener = tokio::net::TcpListener::bind(&args.listen)
        .await
        .map_err(|e| format!("bind {}: {e}", args.listen))?;
    let addr = listener.local_addr().map_err(|e| e.to_string())?;
    tracing::info!(engine = ?args.engine, %addr, "scootlensd listening (ws endpoint: /ws)");
    println!("listening on ws://{addr}/ws");

    gateway.serve(listener).await.map_err(|e| e.to_string())
}
