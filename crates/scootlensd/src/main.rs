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

#[cfg(feature = "embed-console")]
mod embedded_console;

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
    /// `embed-console` 特性构建的二进制缺省即托管嵌入版本，此参数可覆盖。
    #[arg(long)]
    console_dir: Option<PathBuf>,

    /// 最大并发引擎进程数。
    #[arg(long, default_value_t = 8)]
    max_procs: usize,

    /// 额外签发受限令牌（可重复）。格式：`<subject>=<scope>[,<scope>…]`，
    /// 如 `agent:demo=nav@fixture.test,view@fixture.test,act@fixture.test`。
    /// 审批策略取默认（敏感作用域 = 人工审批）；令牌仅打印一次，不落盘。
    #[arg(long = "issue", value_name = "SUBJECT=SCOPES")]
    issue: Vec<String>,
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

    // 受限令牌：默认审批策略（敏感作用域挂起人工审批），供 Agent/演示接入
    for spec in &args.issue {
        let (subject, token) = issue_scoped(&kernel, spec)?;
        println!("token[{subject}]: {token}");
    }

    let gateway = Gateway::new(
        Dispatcher::new(kernel),
        GatewayConfig {
            console_dir: args.console_dir.clone(),
            ..GatewayConfig::default()
        },
    );

    let listener = tokio::net::TcpListener::bind(&args.listen)
        .await
        .map_err(|e| format!("bind {}: {e}", args.listen))?;
    let addr = listener.local_addr().map_err(|e| e.to_string())?;
    tracing::info!(engine = ?args.engine, %addr, "scootlensd listening (ws endpoint: /ws)");
    println!("listening on ws://{addr}/ws");

    // 嵌入式 Console：未显式指定 --console-dir 时托管编译进二进制的 console/dist
    #[cfg(feature = "embed-console")]
    if args.console_dir.is_none() {
        tracing::info!("serving embedded web console at /");
        let router = gateway.router().fallback(embedded_console::serve);
        return axum::serve(listener, router)
            .await
            .map_err(|e| e.to_string());
    }

    gateway.serve(listener).await.map_err(|e| e.to_string())
}

/// 解析 `--issue <subject>=<scope,scope…>` 并签发令牌。
fn issue_scoped(kernel: &scootlens_kernel::Kernel, spec: &str) -> Result<(String, String), String> {
    let (subject, scopes_text) = spec
        .split_once('=')
        .ok_or_else(|| format!("--issue {spec:?}: expected <subject>=<scope,scope…>"))?;
    if subject.is_empty() {
        return Err(format!("--issue {spec:?}: empty subject"));
    }
    let scopes = scopes_text
        .split(',')
        .filter(|s| !s.is_empty())
        .map(|s| s.parse().map_err(|e| format!("--issue {spec:?}: {e}")))
        .collect::<Result<Vec<_>, _>>()?;
    if scopes.is_empty() {
        return Err(format!("--issue {spec:?}: at least one scope required"));
    }
    let claims = TokenClaims {
        subject: subject.to_owned(),
        scopes,
        constraints: TokenConstraints::default(),
        issued_by: "scootlensd".into(),
        issued_at: SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or_default(),
    };
    Ok((subject.to_owned(), kernel.security().issue(&claims)))
}
