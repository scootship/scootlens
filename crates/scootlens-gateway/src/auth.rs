//! Console 登录认证（docs/06-security-model.md §Console 认证）。
//!
//! 动机：管理员经 `?token=slt1…` URL 参数接入会把长期凭据泄漏进浏览器历史 /
//! 代理日志 / Referrer。本模块为**人类用户**提供会话 cookie 登录：
//!
//! - **用户名密码**：sha256 摘要常时比较，凭据只经 POST body，不进 URL
//! - **Microsoft Entra ID**（OAuth2 authorization-code flow，手工 fetch 风格，
//!   不引 msal 类重依赖；参考个人工具 asuc 的同款实现）
//!
//! 会话仅存内存（重启即失效，个人规模可接受）；cookie 为
//! `HttpOnly + SameSite=Strict`，WS 握手在缺失 `?token=` 时回退 cookie 会话。
//! Agent / 自动化接入路径不变：仍走 capability 令牌握手。

use std::collections::HashMap;
use std::sync::{Mutex, PoisonError};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use axum::http::HeaderMap;
use scootlens_abi::{ApprovalMode, TokenClaims, TokenConstraints};
use sha2::{Digest, Sha256};

/// 会话 cookie 名。
pub const SESSION_COOKIE: &str = "sl_session";
/// 会话存活时长（12h，与个人工具惯例一致）。
pub const SESSION_TTL: Duration = Duration::from_secs(12 * 60 * 60);
/// OAuth `state` 参数存活时长。
const STATE_TTL: Duration = Duration::from_secs(5 * 60);
/// Microsoft Graph 请求的 OIDC 作用域。
const MS_SCOPES: &str = "openid profile email User.Read";

/// 用户名密码认证配置。
#[derive(Debug, Clone)]
pub struct PasswordConfig {
    /// 登录用户名（如 `admin`）。
    pub username: String,
    /// 密码的 SHA-256 摘要（32 字节）。
    pub password_sha256: [u8; 32],
}

impl PasswordConfig {
    /// 从明文密码构造（启动时立即摘要，明文不驻留）。
    pub fn from_plain(username: impl Into<String>, password: &str) -> Self {
        Self {
            username: username.into(),
            password_sha256: sha256(password.as_bytes()),
        }
    }
}

/// Microsoft Entra ID（Azure AD）OAuth2 配置。
#[derive(Debug, Clone)]
pub struct MicrosoftConfig {
    /// App Registration 的 client id。
    pub client_id: String,
    /// 租户（`organizations` / `common` / 具体 tenant id）。
    pub tenant: String,
    /// 回调地址，必须与 App Registration 一致（`…/auth/ms/callback`）。
    pub redirect_uri: String,
    /// client secret（从 env / 文件解析后传入，不落盘配置）。
    pub client_secret: String,
    /// 允许登录的邮箱（小写比较）。
    pub allowed_emails: Vec<String>,
    /// 允许登录的邮箱域（如 `example.com`）。
    pub allowed_domains: Vec<String>,
}

/// 认证配置：两种方式各自可选；都缺省时登录端点报 “未启用”。
#[derive(Debug, Clone, Default)]
pub struct AuthConfig {
    pub password: Option<PasswordConfig>,
    pub microsoft: Option<MicrosoftConfig>,
    /// 经 HTTPS 反代部署时置 true，为 cookie 附加 `Secure`。
    pub cookie_secure: bool,
}

impl AuthConfig {
    /// 是否配置了任一登录方式。
    pub fn enabled(&self) -> bool {
        self.password.is_some() || self.microsoft.is_some()
    }
}

struct Session {
    claims: TokenClaims,
    created: Instant,
}

/// 认证状态：配置 + 内存会话表 + OAuth state 表。
pub(crate) struct AuthState {
    pub(crate) config: AuthConfig,
    sessions: Mutex<HashMap<String, Session>>,
    pending_states: Mutex<HashMap<String, Instant>>,
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

fn sha256(data: &[u8]) -> [u8; 32] {
    let mut h = Sha256::new();
    h.update(data);
    h.finalize().into()
}

/// 常时比较（长度相同则逐字节 OR 差异），避免早退时序侧信道。
fn ct_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

/// 解析 16 进制 SHA-256 摘要（64 个 hex 字符）。
pub fn parse_sha256_hex(s: &str) -> Result<[u8; 32], String> {
    let bytes = hex::decode(s.trim()).map_err(|e| format!("invalid sha256 hex: {e}"))?;
    <[u8; 32]>::try_from(bytes.as_slice())
        .map_err(|_| format!("sha256 digest must be 32 bytes, got {}", bytes.len()))
}

/// Console 登录会话的 claims：全作用域 + 全自动审批（与 `scootlensd`
/// 启动打印的管理员令牌同权；登录方式只是凭据形态不同）。
fn console_claims(subject: String) -> TokenClaims {
    let mut constraints = TokenConstraints::default();
    constraints.approval.insert("*".into(), ApprovalMode::Auto);
    TokenClaims {
        subject,
        // "*" 是合法作用域字面量（scootlensd 管理员令牌同款）；解析失败不可能，
        // 但仍防御性回退为空作用域（等价于无权限）。
        scopes: "*".parse().map(|s| vec![s]).unwrap_or_default(),
        constraints,
        issued_by: "gateway:auth".into(),
        issued_at: SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or_default(),
    }
}

fn random_id() -> String {
    let bytes: [u8; 32] = rand::random();
    hex::encode(bytes)
}

impl AuthState {
    pub(crate) fn new(config: AuthConfig) -> Self {
        Self {
            config,
            sessions: Mutex::new(HashMap::new()),
            pending_states: Mutex::new(HashMap::new()),
        }
    }

    /// 校验用户名密码；成功返回新会话 id。失败方（用户名或密码）不区分。
    pub(crate) fn login_password(&self, username: &str, password: &str) -> Option<String> {
        let cfg = self.config.password.as_ref()?;
        let user_ok = cfg.username == username;
        let pass_ok = ct_eq(&sha256(password.as_bytes()), &cfg.password_sha256);
        if !(user_ok && pass_ok) {
            return None;
        }
        Some(self.create_session(format!("user:{username}")))
    }

    /// 为 subject 建立会话，返回会话 id。
    pub(crate) fn create_session(&self, subject: String) -> String {
        let id = random_id();
        let mut sessions = lock(&self.sessions);
        sessions.retain(|_, s| s.created.elapsed() < SESSION_TTL);
        sessions.insert(
            id.clone(),
            Session {
                claims: console_claims(subject),
                created: Instant::now(),
            },
        );
        id
    }

    /// 查会话（过期即删）。
    pub(crate) fn session_claims(&self, id: &str) -> Option<TokenClaims> {
        let mut sessions = lock(&self.sessions);
        match sessions.get(id) {
            Some(s) if s.created.elapsed() < SESSION_TTL => Some(s.claims.clone()),
            Some(_) => {
                sessions.remove(id);
                None
            }
            None => None,
        }
    }

    pub(crate) fn destroy_session(&self, id: &str) {
        lock(&self.sessions).remove(id);
    }

    /// 生成一次性 OAuth `state`。
    pub(crate) fn create_login_state(&self) -> String {
        let state = random_id();
        let mut states = lock(&self.pending_states);
        states.retain(|_, exp| *exp > Instant::now());
        states.insert(state.clone(), Instant::now() + STATE_TTL);
        state
    }

    /// 消费 `state`（单次有效）。
    pub(crate) fn consume_login_state(&self, state: &str) -> bool {
        lock(&self.pending_states)
            .remove(state)
            .is_some_and(|exp| exp > Instant::now())
    }
}

// ---------- cookie ----------

/// 从请求头解析会话 cookie 值。
pub(crate) fn session_cookie(headers: &HeaderMap) -> Option<String> {
    let raw = headers.get(axum::http::header::COOKIE)?.to_str().ok()?;
    raw.split(';').find_map(|kv| {
        let (k, v) = kv.split_once('=')?;
        (k.trim() == SESSION_COOKIE).then(|| v.trim().to_owned())
    })
}

/// 组装 Set-Cookie 值（HttpOnly + SameSite=Strict；HTTPS 部署加 Secure）。
pub(crate) fn set_cookie(sid: &str, secure: bool, max_age: Duration) -> String {
    let mut v = format!(
        "{SESSION_COOKIE}={sid}; Path=/; HttpOnly; SameSite=Strict; Max-Age={}",
        max_age.as_secs()
    );
    if secure {
        v.push_str("; Secure");
    }
    v
}

/// 清除 cookie 的 Set-Cookie 值。
pub(crate) fn clear_cookie(secure: bool) -> String {
    set_cookie("", secure, Duration::ZERO)
}

// ---------- Microsoft Entra ID（OAuth2 authorization-code flow） ----------

/// Graph `/me` 返回的用户信息子集。
#[derive(Debug, serde::Deserialize)]
pub(crate) struct GraphUser {
    #[serde(default)]
    pub mail: Option<String>,
    #[serde(rename = "userPrincipalName", default)]
    pub user_principal_name: Option<String>,
}

/// 登录邮箱：优先 `mail`，回退 UPN；统一小写。
pub(crate) fn login_email(user: &GraphUser) -> String {
    let mail = user.mail.as_deref().unwrap_or("").trim();
    if !mail.is_empty() {
        return mail.to_lowercase();
    }
    user.user_principal_name
        .as_deref()
        .unwrap_or("")
        .trim()
        .to_lowercase()
}

/// 白名单判定：邮箱精确匹配或域匹配；两个名单皆空 = 拒绝一切（安全默认）。
pub(crate) fn is_allowed_email(cfg: &MicrosoftConfig, email: &str) -> bool {
    let needle = email.trim().to_lowercase();
    if needle.is_empty() {
        return false;
    }
    if cfg
        .allowed_emails
        .iter()
        .any(|e| e.trim().to_lowercase() == needle)
    {
        return true;
    }
    let Some((_, domain)) = needle.rsplit_once('@') else {
        return false;
    };
    cfg.allowed_domains
        .iter()
        .any(|d| d.trim().to_lowercase().trim_start_matches('@') == domain)
}

/// 授权跳转 URL。
pub(crate) fn authorize_url(cfg: &MicrosoftConfig, state: &str) -> String {
    let mut url = url_base(&cfg.tenant, "authorize");
    url.query_pairs_mut()
        .append_pair("client_id", cfg.client_id.trim())
        .append_pair("response_type", "code")
        .append_pair("redirect_uri", cfg.redirect_uri.trim())
        .append_pair("response_mode", "query")
        .append_pair("scope", MS_SCOPES)
        .append_pair("state", state);
    url.to_string()
}

fn url_base(tenant: &str, endpoint: &str) -> url::Url {
    let t: String = url::form_urlencoded::byte_serialize(tenant.trim().as_bytes()).collect();
    url::Url::parse(&format!(
        "https://login.microsoftonline.com/{t}/oauth2/v2.0/{endpoint}"
    ))
    .unwrap_or_else(|_| {
        // tenant 已 percent-encode，解析失败不可能；防御性回退到 organizations。
        url::Url::parse("https://login.microsoftonline.com/organizations/oauth2/v2.0/authorize")
            .expect("static url")
    })
}

/// 用授权码换取用户信息：token endpoint → Graph `/me`。
pub(crate) async fn exchange_code_for_user(
    cfg: &MicrosoftConfig,
    code: &str,
) -> Result<GraphUser, String> {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(15))
        .build()
        .map_err(|e| format!("http client: {e}"))?;

    #[derive(serde::Deserialize)]
    struct TokenBody {
        access_token: Option<String>,
    }

    let token_res = client
        .post(url_base(&cfg.tenant, "token").as_str())
        .form(&[
            ("client_id", cfg.client_id.trim()),
            ("client_secret", cfg.client_secret.as_str()),
            ("grant_type", "authorization_code"),
            ("code", code),
            ("redirect_uri", cfg.redirect_uri.trim()),
            ("scope", MS_SCOPES),
        ])
        .send()
        .await
        .map_err(|e| format!("token exchange: {e}"))?;
    if !token_res.status().is_success() {
        return Err(format!("token exchange failed ({})", token_res.status()));
    }
    let body: TokenBody = token_res
        .json()
        .await
        .map_err(|e| format!("token body: {e}"))?;
    let access_token = body
        .access_token
        .ok_or_else(|| "token response missing access_token".to_owned())?;

    let me_res = client
        .get("https://graph.microsoft.com/v1.0/me")
        .bearer_auth(access_token)
        .send()
        .await
        .map_err(|e| format!("graph /me: {e}"))?;
    if !me_res.status().is_success() {
        return Err(format!("graph /me failed ({})", me_res.status()));
    }
    me_res.json().await.map_err(|e| format!("graph body: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ms_cfg() -> MicrosoftConfig {
        MicrosoftConfig {
            client_id: "cid-123".into(),
            tenant: "organizations".into(),
            redirect_uri: "http://127.0.0.1:9910/auth/ms/callback".into(),
            client_secret: "s3cret".into(),
            allowed_emails: vec!["Boss@Example.com".into()],
            allowed_domains: vec!["@corp.test".into()],
        }
    }

    #[test]
    fn password_login_roundtrip() {
        let st = AuthState::new(AuthConfig {
            password: Some(PasswordConfig::from_plain("admin", "hunter2")),
            ..AuthConfig::default()
        });
        assert!(st.login_password("admin", "wrong").is_none());
        assert!(st.login_password("nobody", "hunter2").is_none());
        let sid = st.login_password("admin", "hunter2").expect("session");
        let claims = st.session_claims(&sid).expect("claims");
        assert_eq!(claims.subject, "user:admin");
        assert_eq!(claims.issued_by, "gateway:auth");
        st.destroy_session(&sid);
        assert!(st.session_claims(&sid).is_none());
    }

    #[test]
    fn password_disabled_rejects() {
        let st = AuthState::new(AuthConfig::default());
        assert!(st.login_password("admin", "x").is_none());
    }

    #[test]
    fn parse_sha256_hex_roundtrip() {
        let hexstr = hex::encode(sha256(b"hunter2"));
        let cfg = PasswordConfig {
            username: "admin".into(),
            password_sha256: parse_sha256_hex(&hexstr).expect("parse"),
        };
        assert_eq!(cfg.password_sha256, sha256(b"hunter2"));
        assert!(parse_sha256_hex("zz").is_err());
        assert!(parse_sha256_hex("abcd").is_err());
    }

    #[test]
    fn ct_eq_basics() {
        assert!(ct_eq(b"abc", b"abc"));
        assert!(!ct_eq(b"abc", b"abd"));
        assert!(!ct_eq(b"abc", b"ab"));
    }

    #[test]
    fn login_state_single_use_and_expiry() {
        let st = AuthState::new(AuthConfig::default());
        let s = st.create_login_state();
        assert!(st.consume_login_state(&s));
        assert!(!st.consume_login_state(&s), "state must be single-use");
        assert!(!st.consume_login_state("unknown"));
    }

    #[test]
    fn cookie_parse_and_format() {
        let mut headers = HeaderMap::new();
        headers.insert(
            axum::http::header::COOKIE,
            "a=1; sl_session=deadbeef; b=2".parse().expect("hv"),
        );
        assert_eq!(session_cookie(&headers).as_deref(), Some("deadbeef"));

        let v = set_cookie("abc", false, Duration::from_secs(60));
        assert!(v.contains("sl_session=abc"));
        assert!(v.contains("HttpOnly"));
        assert!(v.contains("SameSite=Strict"));
        assert!(!v.contains("Secure"));
        assert!(set_cookie("abc", true, Duration::from_secs(60)).contains("Secure"));
        assert!(clear_cookie(false).contains("Max-Age=0"));
    }

    #[test]
    fn authorize_url_contains_expected_params() {
        let u = authorize_url(&ms_cfg(), "st4te");
        assert!(
            u.starts_with("https://login.microsoftonline.com/organizations/oauth2/v2.0/authorize?")
        );
        assert!(u.contains("client_id=cid-123"));
        assert!(u.contains("response_type=code"));
        assert!(u.contains("state=st4te"));
        assert!(u.contains("response_mode=query"));
        // client_secret 绝不出现在授权 URL
        assert!(!u.contains("s3cret"));
    }

    #[test]
    fn allowed_email_matrix() {
        let cfg = ms_cfg();
        assert!(is_allowed_email(&cfg, "boss@example.com"));
        assert!(is_allowed_email(&cfg, "BOSS@EXAMPLE.COM"));
        assert!(is_allowed_email(&cfg, "anyone@corp.test"));
        assert!(!is_allowed_email(&cfg, "intruder@other.test"));
        assert!(!is_allowed_email(&cfg, ""));
        assert!(!is_allowed_email(&cfg, "no-at-sign"));

        // 双名单皆空 = 拒绝一切（安全默认）
        let empty = MicrosoftConfig {
            allowed_emails: vec![],
            allowed_domains: vec![],
            ..ms_cfg()
        };
        assert!(!is_allowed_email(&empty, "boss@example.com"));
    }

    #[test]
    fn graph_user_email_fallback() {
        let with_mail = GraphUser {
            mail: Some(" Boss@Example.com ".into()),
            user_principal_name: Some("upn@corp.test".into()),
        };
        assert_eq!(login_email(&with_mail), "boss@example.com");
        let upn_only = GraphUser {
            mail: None,
            user_principal_name: Some("UPN@corp.test".into()),
        };
        assert_eq!(login_email(&upn_only), "upn@corp.test");
    }

    #[test]
    fn console_claims_full_scope_auto_approval() {
        let c = console_claims("user:x".into());
        assert_eq!(c.scopes.len(), 1);
        assert_eq!(c.constraints.approval.get("*"), Some(&ApprovalMode::Auto));
    }
}
