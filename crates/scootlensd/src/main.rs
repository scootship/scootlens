//! scootlensd：守护进程组装点（driver → kernel → dispatcher → gateway）。

use std::sync::Arc;

use clap::{Parser, ValueEnum};
use rand::RngCore;
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

    /// 全权令牌（P1 骨架）。缺省时随机生成并打印。
    #[arg(long, env = "SCOOTLENS_TOKEN")]
    token: Option<String>,

    /// 最大并发引擎进程数。
    #[arg(long, default_value_t = 8)]
    max_procs: usize,
}

fn main() -> Result<(), String> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info".into()),
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

    let token = args.token.unwrap_or_else(|| {
        let mut r = rand::rng();
        let t = format!("sl-{:016x}{:016x}", r.next_u64(), r.next_u64());
        // 缺省令牌必须让操作者看到一次（仅打印，不落盘）
        println!("generated token: {t}");
        t
    });

    let kernel = Kernel::new(
        driver,
        KernelConfig {
            max_procs: args.max_procs,
            ..KernelConfig::default()
        },
    );
    let gateway = Gateway::new(Dispatcher::new(kernel), GatewayConfig { token });

    let listener = tokio::net::TcpListener::bind(&args.listen)
        .await
        .map_err(|e| format!("bind {}: {e}", args.listen))?;
    let addr = listener.local_addr().map_err(|e| e.to_string())?;
    tracing::info!(engine = ?args.engine, %addr, "scootlensd listening (ws endpoint: /ws)");
    println!("listening on ws://{addr}/ws");

    gateway.serve(listener).await.map_err(|e| e.to_string())
}
