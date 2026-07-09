//! scootlensd：守护进程组装点（driver → kernel → dispatcher → gateway）。

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use clap::{Parser, ValueEnum};
use scootlens_abi::{ApprovalMode, TokenClaims, TokenConstraints};
use scootlens_driver_chromium::ChromiumDriver;
use scootlens_driver_mock::MockDriver;
use scootlens_gateway::{AuthConfig, Gateway, GatewayConfig, MicrosoftConfig, PasswordConfig};
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

    // ---------- Console 登录（会话 cookie；替代把 admin 令牌贴进 URL） ----------
    /// Console 密码登录的用户名。
    #[arg(long, default_value = "admin")]
    admin_user: String,

    /// Console 密码登录的密码（明文，经环境变量传入；启动即摘要，不驻留明文）。
    /// 与 `--admin-password-sha256` 二选一；都缺省则密码登录不启用。
    #[arg(long, env = "SCOOTLENS_ADMIN_PASSWORD", hide_env_values = true)]
    admin_password: Option<String>,

    /// Console 密码登录的密码 SHA-256 摘要（64 位 hex；避免明文进 shell 历史）。
    #[arg(long, value_name = "HEX64")]
    admin_password_sha256: Option<String>,

    /// Microsoft Entra ID 登录：App Registration client id。设置后启用 MS 登录，
    /// 需同时配置 redirect-uri、SCOOTLENS_MSAUTH_CLIENT_SECRET 与至少一条白名单。
    #[arg(long, value_name = "GUID")]
    msauth_client_id: Option<String>,

    /// Microsoft Entra ID 租户（`organizations` / `common` / 租户 id）。
    #[arg(long, default_value = "organizations")]
    msauth_tenant: String,

    /// OAuth 回调地址（须与 App Registration 一致），如
    /// `http://127.0.0.1:9910/auth/ms/callback`。
    #[arg(long, value_name = "URL")]
    msauth_redirect_uri: Option<String>,

    /// Microsoft client secret（环境变量传入，不进 CLI 历史）。
    #[arg(long, env = "SCOOTLENS_MSAUTH_CLIENT_SECRET", hide_env_values = true)]
    msauth_client_secret: Option<String>,

    /// 允许登录的邮箱（可重复）。与 `--msauth-allow-domain` 至少配一条。
    #[arg(long = "msauth-allow-email", value_name = "EMAIL")]
    msauth_allow_emails: Vec<String>,

    /// 允许登录的邮箱域（可重复），如 `example.com`。
    #[arg(long = "msauth-allow-domain", value_name = "DOMAIN")]
    msauth_allow_domains: Vec<String>,

    /// 经 HTTPS 反代部署时置位：会话 cookie 附加 `Secure`。
    #[arg(long)]
    auth_cookie_secure: bool,
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
            auth: build_auth(&args)?,
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

/// 组装 Console 登录配置（docs/06-security-model.md §Console 认证）。
///
/// 半配置（如设了 client id 却缺 secret / 回调 / 白名单）直接启动失败，
/// 不静默降级 —— 认证配置错误必须显式暴露。
fn build_auth(args: &Args) -> Result<AuthConfig, String> {
    let password = match (&args.admin_password, &args.admin_password_sha256) {
        (Some(_), Some(_)) => {
            return Err(
                "--admin-password (env) and --admin-password-sha256 are mutually exclusive".into(),
            );
        }
        (Some(plain), None) => {
            if plain.is_empty() {
                return Err("SCOOTLENS_ADMIN_PASSWORD must not be empty".into());
            }
            Some(PasswordConfig::from_plain(&args.admin_user, plain))
        }
        (None, Some(hex)) => Some(PasswordConfig {
            username: args.admin_user.clone(),
            password_sha256: scootlens_gateway::parse_sha256_hex(hex)
                .map_err(|e| format!("--admin-password-sha256: {e}"))?,
        }),
        (None, None) => None,
    };

    let microsoft = match &args.msauth_client_id {
        None => None,
        Some(client_id) => {
            let redirect_uri = args
                .msauth_redirect_uri
                .clone()
                .ok_or("--msauth-redirect-uri is required with --msauth-client-id")?;
            let client_secret = args
                .msauth_client_secret
                .clone()
                .filter(|s| !s.is_empty())
                .ok_or("SCOOTLENS_MSAUTH_CLIENT_SECRET is required with --msauth-client-id")?;
            if args.msauth_allow_emails.is_empty() && args.msauth_allow_domains.is_empty() {
                return Err(
                    "at least one --msauth-allow-email / --msauth-allow-domain is required \
                     (empty allowlist would reject every login)"
                        .into(),
                );
            }
            Some(MicrosoftConfig {
                client_id: client_id.clone(),
                tenant: args.msauth_tenant.clone(),
                redirect_uri,
                client_secret,
                allowed_emails: args.msauth_allow_emails.clone(),
                allowed_domains: args.msauth_allow_domains.clone(),
            })
        }
    };

    if password.is_some() || microsoft.is_some() {
        let modes: Vec<&str> = [
            password.as_ref().map(|_| "password"),
            microsoft.as_ref().map(|_| "microsoft"),
        ]
        .into_iter()
        .flatten()
        .collect();
        println!("console login: {}", modes.join(" + "));
    }

    Ok(AuthConfig {
        password,
        microsoft,
        cookie_secure: args.auth_cookie_secure,
    })
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
