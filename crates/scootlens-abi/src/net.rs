//! 网络规则类型（`net.rules.set` 协议表面；求值引擎在 scootlens-net）。

use serde::{Deserialize, Serialize};

/// 规则动作。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NetAction {
    Allow,
    Deny,
}

/// 一条网络规则。命中条件为各限定维度的**与**；缺省维度不限定。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NetRule {
    pub action: NetAction,
    /// host 模式：`*`、精确 host、`*.suffix`（同作用域 origin 语义）。
    pub host: String,
    /// 限定 HTTP 方法（大写），空 = 全部。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub methods: Vec<String>,
    /// 限定资源类型（`document`/`script`/`image`/`xhr`/`fetch`…），空 = 全部。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub resource_types: Vec<String>,
    /// 命中且放行时注入/覆盖的请求头。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub set_headers: Vec<NetHeader>,
}

/// 注入的请求头。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NetHeader {
    pub name: String,
    pub value: String,
}

/// 默认策略（无规则命中时）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NetDefault {
    /// 默认放行 + 显式 denylist。
    #[default]
    Allow,
    /// 白名单模式：未命中 allow 规则即拒。
    Deny,
}

/// 一组规则（proc 级或全局级）。
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct NetRuleSet {
    #[serde(default)]
    pub default: NetDefault,
    #[serde(default)]
    pub rules: Vec<NetRule>,
}

/// 一次请求的描述（策略输入 + net.log 条目主体）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NetRequestSummary {
    pub url: String,
    /// HTTP 方法（大写）。
    pub method: String,
    /// 资源类型（小写；未知为 `other`）。
    pub resource_type: String,
}

/// 策略判定结果。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "decision", rename_all = "snake_case")]
pub enum NetDecision {
    /// 放行，附带需注入的请求头（可为空）。
    Allow {
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        set_headers: Vec<NetHeader>,
    },
    /// 拦截（客户端观察到 `E_NET_BLOCKED` 或请求失败）。
    Deny,
}

impl NetDecision {
    pub fn allowed(&self) -> bool {
        matches!(self, NetDecision::Allow { .. })
    }
}
