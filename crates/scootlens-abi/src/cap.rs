//! Capability 令牌 claims 与审批类型（docs/06-security-model.md）。
//!
//! 令牌 wire 格式（ADR-0007）：`slt1.<base64url(claims json)>.<base64url(ed25519 sig)>`。
//! 本模块只定义纯类型；签发/验签在内核 Security Manager。

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::scope::Scope;

/// 令牌前缀（版本 1）。
pub const TOKEN_PREFIX: &str = "slt1";

/// 令牌 claims（签名载荷）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TokenClaims {
    /// 主体，如 `agent:ops-bot-1`、`user:admin`。
    pub subject: String,
    /// 授予的作用域。
    pub scopes: Vec<Scope>,
    /// 约束（过期/限速/审批策略）。
    #[serde(default, skip_serializing_if = "TokenConstraints::is_empty")]
    pub constraints: TokenConstraints,
    /// 签发者。
    pub issued_by: String,
    /// 签发时间（unix 秒）。
    pub issued_at: u64,
}

/// 令牌约束。
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct TokenConstraints {
    /// 过期时间（unix 秒）；缺省 = 不过期。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<u64>,
    /// 限速，如 `60/min`、`10/sec`；缺省 = 不限速。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rate: Option<String>,
    /// 审批策略：作用域模式 → 模式。缺省时敏感作用域 = manual，其余 = auto。
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub approval: BTreeMap<String, ApprovalMode>,
}

impl TokenConstraints {
    pub fn is_empty(&self) -> bool {
        self.expires_at.is_none() && self.rate.is_none() && self.approval.is_empty()
    }
}

/// 审批模式。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalMode {
    /// 直接放行。
    Auto,
    /// 挂起等待人工审批。
    Manual,
}

/// 审批决定（`cap.approve` 参数）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalDecision {
    Allow,
    Deny,
}

/// 挂起中的审批请求（`cap.pending` 返回项 + `cap.request` 事件载荷）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PendingApproval {
    /// 审批请求 ID（`apr-` 前缀）。
    pub id: String,
    /// 请求主体。
    pub subject: String,
    /// 触发审批的作用域要求。
    pub scope: Scope,
    /// 关联的 syscall 方法（`cap.request` 主动申请时为 `cap.request`）。
    pub method: String,
    /// 参数摘要（已脱敏）。
    pub params_summary: serde_json::Value,
    /// 申请理由（cap.request 提供）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    /// 创建时间（unix 毫秒）。
    pub created_at_ms: u64,
}

/// 默认需要人工审批的敏感作用域前缀（docs/06-security-model.md）。
pub const SENSITIVE_SCOPES: &[&str] = &[
    "js:exec",
    "state:read",
    "state:write",
    "state:export",
    "state:import",
    "state:delete",
    "act:upload",
    "act:takeover",
    "net:rules",
    "vault:use",
    "obs:replay",
    "cap:admin",
];

/// `scope` 是否落在敏感集合内（按段前缀判断，忽略 origin）。
pub fn is_sensitive(scope: &Scope) -> bool {
    SENSITIVE_SCOPES.iter().any(|s| {
        let sens: Vec<&str> = s.split(':').collect();
        scope.segments().len() >= sens.len()
            && sens
                .iter()
                .zip(scope.segments().iter())
                .all(|(a, b)| a == b)
    })
}
