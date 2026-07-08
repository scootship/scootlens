//! Security Manager：令牌签发/验签、有效作用域、审批流、限速
//! （docs/06-security-model.md、ADR-0007）。
//!
//! 令牌 wire 格式：`slt1.<b64url(claims json)>.<b64url(ed25519 sig)>`。

use std::collections::{HashMap, HashSet, VecDeque};
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, PoisonError};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD as B64;
use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};
use scootlens_abi::{
    AbiError, ApprovalDecision, ApprovalMode, ErrorCode, PendingApproval, Scope, TOKEN_PREFIX,
    TokenClaims, TokenConstraints, is_sensitive,
};
use tokio::sync::oneshot;

/// 已验证的调用方（gateway 验签后随连接携带）。
#[derive(Debug, Clone)]
pub struct Caller {
    pub subject: String,
    pub scopes: Vec<Scope>,
    pub constraints: TokenConstraints,
}

impl Caller {
    pub fn from_claims(claims: TokenClaims) -> Self {
        Self {
            subject: claims.subject,
            scopes: claims.scopes,
            constraints: claims.constraints,
        }
    }
}

/// 鉴权判定：放行或需要人工审批。
pub enum AuthzGate {
    Allowed,
    NeedsApproval {
        pending: PendingApproval,
        rx: oneshot::Receiver<(ApprovalDecision, bool)>,
    },
}

struct PendingEntry {
    info: PendingApproval,
    tx: Option<oneshot::Sender<(ApprovalDecision, bool)>>,
}

/// Security Manager。
pub struct SecurityManager {
    signing: SigningKey,
    grants: Mutex<HashMap<String, HashSet<Scope>>>,
    revoked: Mutex<HashMap<String, HashSet<Scope>>>,
    rates: Mutex<HashMap<String, VecDeque<Instant>>>,
    pending: Mutex<Vec<PendingEntry>>,
    approval_counter: AtomicU64,
}

impl SecurityManager {
    pub fn new(signing: SigningKey) -> Self {
        Self {
            signing,
            grants: Mutex::new(HashMap::new()),
            revoked: Mutex::new(HashMap::new()),
            rates: Mutex::new(HashMap::new()),
            pending: Mutex::new(Vec::new()),
            approval_counter: AtomicU64::new(0),
        }
    }

    /// 从 `dir/keys/signing.key`（hex seed，0600）加载或生成并持久化。
    /// `dir = None` → 每次随机（内存模式）。
    pub fn load_or_generate(dir: Option<&Path>) -> std::io::Result<Self> {
        let Some(dir) = dir else {
            return Ok(Self::new(SigningKey::from_bytes(
                &rand::random::<[u8; 32]>(),
            )));
        };
        let keys_dir = dir.join("keys");
        std::fs::create_dir_all(&keys_dir)?;
        let key_path = keys_dir.join("signing.key");
        let signing = if key_path.exists() {
            let hex_seed = std::fs::read_to_string(&key_path)?;
            let bytes = hex::decode(hex_seed.trim()).map_err(std::io::Error::other)?;
            let seed: [u8; 32] = bytes
                .try_into()
                .map_err(|_| std::io::Error::other("signing.key must be 32 bytes hex"))?;
            SigningKey::from_bytes(&seed)
        } else {
            let key = SigningKey::from_bytes(&rand::random::<[u8; 32]>());
            std::fs::write(&key_path, hex::encode(key.to_bytes()))?;
            restrict_permissions(&key_path)?;
            key
        };
        Ok(Self::new(signing))
    }

    pub fn verifying_key(&self) -> VerifyingKey {
        self.signing.verifying_key()
    }

    // ---------- 令牌 ----------

    /// 签发令牌。
    pub fn issue(&self, claims: &TokenClaims) -> String {
        let payload = serde_json::to_vec(claims).expect("claims serialize");
        let sig = self.signing.sign(&payload);
        format!(
            "{TOKEN_PREFIX}.{}.{}",
            B64.encode(&payload),
            B64.encode(sig.to_bytes())
        )
    }

    /// 验签 + 过期检查。任何失败 → `E_CAP_DENIED`（不泄漏细节差异）。
    pub fn verify(&self, token: &str) -> Result<TokenClaims, AbiError> {
        let deny = |why: &str| AbiError::new(ErrorCode::CapDenied, format!("invalid token: {why}"));
        let mut parts = token.splitn(3, '.');
        let (prefix, payload, sig) = (
            parts.next().unwrap_or_default(),
            parts.next().unwrap_or_default(),
            parts.next().unwrap_or_default(),
        );
        if prefix != TOKEN_PREFIX {
            return Err(deny("bad prefix"));
        }
        let payload = B64.decode(payload).map_err(|_| deny("bad payload b64"))?;
        let sig_bytes: [u8; 64] = B64
            .decode(sig)
            .ok()
            .and_then(|v| v.try_into().ok())
            .ok_or_else(|| deny("bad signature b64"))?;
        self.signing
            .verifying_key()
            .verify(&payload, &Signature::from_bytes(&sig_bytes))
            .map_err(|_| deny("signature mismatch"))?;
        let claims: TokenClaims =
            serde_json::from_slice(&payload).map_err(|_| deny("bad claims json"))?;
        if let Some(exp) = claims.constraints.expires_at
            && unix_now() >= exp
        {
            return Err(deny("token expired"));
        }
        Ok(claims)
    }

    // ---------- 动态授权 ----------

    pub fn grant(&self, subject: &str, scope: Scope) {
        self.lock(&self.revoked)
            .entry(subject.to_owned())
            .or_default()
            .remove(&scope);
        self.lock(&self.grants)
            .entry(subject.to_owned())
            .or_default()
            .insert(scope);
    }

    pub fn revoke(&self, subject: &str, scope: Scope) {
        self.lock(&self.grants)
            .entry(subject.to_owned())
            .or_default()
            .remove(&scope);
        self.lock(&self.revoked)
            .entry(subject.to_owned())
            .or_default()
            .insert(scope);
    }

    /// 有效作用域 = token.scopes ∪ grants[subject] − revoked[subject]。
    pub fn effective_scopes(&self, caller: &Caller) -> Vec<Scope> {
        let mut set: HashSet<Scope> = caller.scopes.iter().cloned().collect();
        if let Some(g) = self.lock(&self.grants).get(&caller.subject) {
            set.extend(g.iter().cloned());
        }
        if let Some(r) = self.lock(&self.revoked).get(&caller.subject) {
            for s in r {
                set.remove(s);
            }
        }
        let mut out: Vec<Scope> = set.into_iter().collect();
        out.sort_by_key(std::string::ToString::to_string);
        out
    }

    /// 鉴权入口：作用域覆盖 → 限速 → 审批模式判定。
    ///
    /// 覆盖失败 → `E_CAP_DENIED`；超限 → `E_QUOTA`；
    /// 需人工审批 → `AuthzGate::NeedsApproval`（调用方负责发事件并等待）。
    pub fn check(
        &self,
        caller: &Caller,
        required: &Scope,
        method: &str,
        params_summary: serde_json::Value,
    ) -> Result<AuthzGate, AbiError> {
        let effective = self.effective_scopes(caller);
        if !effective.iter().any(|g| g.covers(required)) {
            return Err(AbiError::new(
                ErrorCode::CapDenied,
                format!("scope {required} not granted to {}", caller.subject),
            ));
        }
        self.check_rate(caller)?;
        match self.approval_mode(caller, required) {
            ApprovalMode::Auto => Ok(AuthzGate::Allowed),
            ApprovalMode::Manual => {
                let (pending, rx) = self.begin_approval(
                    &caller.subject,
                    required.clone(),
                    method,
                    params_summary,
                    None,
                );
                Ok(AuthzGate::NeedsApproval { pending, rx })
            }
        }
    }

    /// 审批模式：约束表中**最具体**（段数最多）的匹配模式生效；
    /// 无匹配时敏感作用域 = manual，其余 auto。
    fn approval_mode(&self, caller: &Caller, required: &Scope) -> ApprovalMode {
        let mut best: Option<(usize, ApprovalMode)> = None;
        for (pattern, mode) in &caller.constraints.approval {
            let Ok(pat) = pattern.parse::<Scope>() else {
                continue;
            };
            if pat.covers(required) {
                let specificity = pat.segments().len();
                if best.is_none_or(|(s, _)| specificity >= s) {
                    best = Some((specificity, *mode));
                }
            }
        }
        match best {
            Some((_, mode)) => mode,
            None if is_sensitive(required) => ApprovalMode::Manual,
            None => ApprovalMode::Auto,
        }
    }

    /// 滑动窗口限速（`N/min` | `N/sec`）。
    fn check_rate(&self, caller: &Caller) -> Result<(), AbiError> {
        let Some(rate) = &caller.constraints.rate else {
            return Ok(());
        };
        let Some((n, window)) = parse_rate(rate) else {
            return Ok(());
        };
        let mut rates = self.lock(&self.rates);
        let q = rates.entry(caller.subject.clone()).or_default();
        let now = Instant::now();
        while let Some(front) = q.front()
            && now.duration_since(*front) > window
        {
            q.pop_front();
        }
        if q.len() >= n {
            return Err(AbiError::new(
                ErrorCode::Quota,
                format!("rate limit {rate} exceeded for {}", caller.subject),
            ));
        }
        q.push_back(now);
        Ok(())
    }

    // ---------- 审批流 ----------

    /// 创建挂起审批。返回 (信息, 决定接收端)。
    pub fn begin_approval(
        &self,
        subject: &str,
        scope: Scope,
        method: &str,
        params_summary: serde_json::Value,
        reason: Option<String>,
    ) -> (PendingApproval, oneshot::Receiver<(ApprovalDecision, bool)>) {
        let n = self.approval_counter.fetch_add(1, Ordering::SeqCst) + 1;
        let info = PendingApproval {
            id: format!("apr-{n}"),
            subject: subject.to_owned(),
            scope,
            method: method.to_owned(),
            params_summary,
            reason,
            created_at_ms: unix_now_ms(),
        };
        let (tx, rx) = oneshot::channel();
        self.lock(&self.pending).push(PendingEntry {
            info: info.clone(),
            tx: Some(tx),
        });
        (info, rx)
    }

    /// 审批决定。`remember && allow` → 转为动态 grant。
    ///
    /// 等待者可能已超时离开（send 失败忽略）；审批条目移除。
    pub fn approve(
        &self,
        approval_id: &str,
        decision: ApprovalDecision,
        remember: bool,
    ) -> Result<PendingApproval, AbiError> {
        let entry = {
            let mut pending = self.lock(&self.pending);
            let idx = pending
                .iter()
                .position(|e| e.info.id == approval_id)
                .ok_or_else(|| {
                    AbiError::new(
                        ErrorCode::InvalidArg,
                        format!("no pending approval {approval_id}"),
                    )
                })?;
            pending.remove(idx)
        };
        if remember && decision == ApprovalDecision::Allow {
            self.grant(&entry.info.subject, entry.info.scope.clone());
        }
        if let Some(tx) = entry.tx {
            let _ = tx.send((decision, remember));
        }
        Ok(entry.info)
    }

    /// 审批收件箱。
    pub fn pending_list(&self) -> Vec<PendingApproval> {
        self.lock(&self.pending)
            .iter()
            .map(|e| e.info.clone())
            .collect()
    }

    fn lock<'a, T>(&self, m: &'a Mutex<T>) -> std::sync::MutexGuard<'a, T> {
        m.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

fn parse_rate(rate: &str) -> Option<(usize, Duration)> {
    let (n, unit) = rate.split_once('/')?;
    let n: usize = n.trim().parse().ok()?;
    let window = match unit.trim() {
        "sec" | "s" => Duration::from_secs(1),
        "min" | "m" => Duration::from_secs(60),
        _ => return None,
    };
    Some((n, window))
}

fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

pub(crate) fn unix_now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

#[cfg(unix)]
fn restrict_permissions(path: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
}

#[cfg(not(unix))]
fn restrict_permissions(_path: &Path) -> std::io::Result<()> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn manager() -> SecurityManager {
        SecurityManager::load_or_generate(None).expect("manager")
    }

    fn scope(s: &str) -> Scope {
        s.parse().expect("scope")
    }

    #[test]
    fn issue_then_verify_roundtrips() {
        let sm = manager();
        let claims = TokenClaims {
            subject: "agent:x".into(),
            scopes: vec![scope("view"), scope("nav@app.test")],
            constraints: TokenConstraints::default(),
            issued_by: "test".into(),
            issued_at: 0,
        };
        let token = sm.issue(&claims);
        let back = sm.verify(&token).expect("verify");
        assert_eq!(back.subject, "agent:x");
        assert_eq!(back.scopes.len(), 2);
    }

    #[test]
    fn rate_limit_trips_quota() {
        let sm = manager();
        let mut constraints = TokenConstraints {
            rate: Some("2/min".into()),
            ..Default::default()
        };
        constraints
            .approval
            .insert("view".into(), ApprovalMode::Auto);
        let caller = Caller {
            subject: "agent:rl".into(),
            scopes: vec![scope("view")],
            constraints,
        };
        let req = scope("view");
        assert!(
            sm.check(&caller, &req, "view.snapshot", serde_json::json!({}))
                .is_ok()
        );
        assert!(
            sm.check(&caller, &req, "view.snapshot", serde_json::json!({}))
                .is_ok()
        );
        match sm.check(&caller, &req, "view.snapshot", serde_json::json!({})) {
            Err(e) => assert_eq!(e.code, ErrorCode::Quota, "third call over budget"),
            Ok(_) => panic!("third call must be rate-limited"),
        }
    }

    #[test]
    fn grant_and_revoke_adjust_effective_scopes() {
        let sm = manager();
        let caller = Caller {
            subject: "agent:g".into(),
            scopes: vec![scope("view")],
            constraints: TokenConstraints::default(),
        };
        assert!(
            !sm.effective_scopes(&caller)
                .iter()
                .any(|s| *s == scope("nav"))
        );
        sm.grant("agent:g", scope("nav"));
        assert!(
            sm.effective_scopes(&caller)
                .iter()
                .any(|s| *s == scope("nav"))
        );
        sm.revoke("agent:g", scope("view"));
        assert!(
            !sm.effective_scopes(&caller)
                .iter()
                .any(|s| *s == scope("view"))
        );
    }

    #[test]
    fn approve_with_remember_becomes_persistent_grant() {
        let sm = manager();
        let (pending, _rx) = sm.begin_approval(
            "agent:a",
            scope("js:exec"),
            "js.exec",
            serde_json::json!({}),
            None,
        );
        assert_eq!(sm.pending_list().len(), 1);
        sm.approve(&pending.id, ApprovalDecision::Allow, true)
            .expect("approve");
        assert!(sm.pending_list().is_empty(), "approved item leaves inbox");
        let caller = Caller {
            subject: "agent:a".into(),
            scopes: vec![],
            constraints: TokenConstraints::default(),
        };
        assert!(
            sm.effective_scopes(&caller)
                .iter()
                .any(|s| *s == scope("js:exec")),
            "remembered approval grants the scope"
        );
    }

    #[test]
    fn sensitive_scope_defaults_to_manual_approval() {
        let sm = manager();
        // No approval policy + sensitive scope -> NeedsApproval.
        let caller = Caller {
            subject: "agent:s".into(),
            scopes: vec![scope("js:exec@app.test")],
            constraints: TokenConstraints::default(),
        };
        let gate = sm
            .check(
                &caller,
                &scope("js:exec@app.test"),
                "js.exec",
                serde_json::json!({}),
            )
            .expect("covered");
        assert!(matches!(gate, AuthzGate::NeedsApproval { .. }));
    }
}
