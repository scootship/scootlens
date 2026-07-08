//! Capability 作用域：解析与匹配（docs/06-security-model.md）。
//!
//! 语法：`<domain>[:<action>[:<qualifier>…]][@<origin-pattern>]`
//!
//! - `act@*.github.com` —— github.com 任意子域的输入动作
//! - `state:read:cookies@github.com` —— 读取指定站点 cookie
//! - `js:exec@localhost:*` —— 仅本地任意端口执行 JS
//! - `proc:spawn`、`cap:admin` —— 无 origin 维度的系统级作用域
//! - `*` —— 超级作用域（管理员令牌），覆盖一切
//!
//! **匹配语义**（`grant.covers(required)`）：
//! - grant 段序列是 required 段序列的前缀（更泛的授权覆盖更细的要求）
//! - grant 无 origin ⇒ 覆盖任意 origin；有 origin ⇒ 按模式匹配
//! - origin 模式：`*`（任意）、精确 host、`*.suffix`（严格子域）；
//!   端口：缺省 = 任意，`:*` = 任意，`:1234` = 精确

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

/// 一条 capability 作用域（授予态与要求态共用同一类型）。
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct Scope {
    segments: Vec<String>,
    origin: Option<String>,
}

impl Scope {
    /// 构造要求态作用域：`segments` 如 `["act"]`，origin 如 `Some("app.example.com")`。
    pub fn required(segments: &[&str], origin: Option<&str>) -> Self {
        Self {
            segments: segments.iter().map(|s| (*s).to_owned()).collect(),
            origin: origin.map(str::to_owned),
        }
    }

    /// 段序列（冒号分隔部分）。
    pub fn segments(&self) -> &[String] {
        &self.segments
    }

    /// origin 模式部分。
    pub fn origin(&self) -> Option<&str> {
        self.origin.as_deref()
    }

    /// 超级作用域 `*`。
    pub fn is_superuser(&self) -> bool {
        self.segments.len() == 1 && self.segments[0] == "*" && self.origin.is_none()
    }

    /// 本授权是否覆盖 `required` 要求。
    pub fn covers(&self, required: &Scope) -> bool {
        if self.is_superuser() {
            return true;
        }
        // 段前缀匹配：grant 更泛（更短）才能覆盖
        if self.segments.len() > required.segments.len() {
            return false;
        }
        if !self
            .segments
            .iter()
            .zip(required.segments.iter())
            .all(|(g, r)| g == r)
        {
            return false;
        }
        match (&self.origin, &required.origin) {
            (None, _) => true,
            (Some(pat), Some(origin)) => origin_matches(pat, origin),
            (Some(_), None) => false,
        }
    }
}

/// origin 模式匹配：`pattern` 形如 `*`、`example.com`、`*.example.com`、`localhost:*`、`app.test:8080`。
///
/// `origin` 为规范化的 `host` 或 `host:port`（仅当 URL 显式带端口时含端口）。
pub fn origin_matches(pattern: &str, origin: &str) -> bool {
    if pattern == "*" {
        return true;
    }
    let (pat_host, pat_port) = split_host_port(pattern);
    let (org_host, org_port) = split_host_port(origin);

    let host_ok = if let Some(suffix) = pat_host.strip_prefix("*.") {
        // 严格子域：*.example.com 匹配 a.example.com，不匹配 example.com
        org_host.len() > suffix.len() + 1
            && org_host.to_ascii_lowercase().ends_with(&{
                let mut s = String::with_capacity(suffix.len() + 1);
                s.push('.');
                s.push_str(&suffix.to_ascii_lowercase());
                s
            })
    } else {
        pat_host.eq_ignore_ascii_case(org_host)
    };
    if !host_ok {
        return false;
    }
    match pat_port {
        None | Some("*") => true,
        Some(p) => org_port == Some(p),
    }
}

/// 把 `host[:port]` 拆开；仅当冒号后全为数字或 `*` 时视为端口。
fn split_host_port(s: &str) -> (&str, Option<&str>) {
    if let Some((host, port)) = s.rsplit_once(':')
        && !port.is_empty()
        && (port == "*" || port.bytes().all(|b| b.is_ascii_digit()))
    {
        return (host, Some(port));
    }
    (s, None)
}

/// 作用域解析错误。
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("invalid scope: {0}")]
pub struct ParseScopeError(String);

impl FromStr for Scope {
    type Err = ParseScopeError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if s.is_empty() {
            return Err(ParseScopeError("empty".into()));
        }
        let (body, origin) = match s.rsplit_once('@') {
            Some((b, o)) => (b, Some(o)),
            None => (s, None),
        };
        if body.is_empty() {
            return Err(ParseScopeError(format!("{s}: empty body")));
        }
        if let Some(o) = origin
            && (o.is_empty() || o.contains(char::is_whitespace) || o.contains('@'))
        {
            return Err(ParseScopeError(format!("{s}: bad origin pattern")));
        }
        let segments: Vec<String> = body.split(':').map(str::to_owned).collect();
        for seg in &segments {
            let valid = !seg.is_empty()
                && seg.bytes().all(|b| {
                    b.is_ascii_lowercase()
                        || b.is_ascii_digit()
                        || b == b'_'
                        || b == b'*'
                        || b == b'-'
                        || b == b'.'
                });
            if !valid {
                return Err(ParseScopeError(format!("{s}: bad segment {seg:?}")));
            }
        }
        // `*` 只允许作为唯一段（超级作用域），拒绝 `act:*` 这类歧义写法
        if segments.iter().any(|seg| seg == "*") && (segments.len() != 1 || origin.is_some()) {
            return Err(ParseScopeError(format!(
                "{s}: `*` is only valid as the whole scope"
            )));
        }
        Ok(Scope {
            segments,
            origin: origin.map(str::to_owned),
        })
    }
}

impl TryFrom<String> for Scope {
    type Error = ParseScopeError;

    fn try_from(s: String) -> Result<Self, Self::Error> {
        s.parse()
    }
}

impl From<Scope> for String {
    fn from(s: Scope) -> String {
        s.to_string()
    }
}

impl fmt::Display for Scope {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.segments.join(":"))?;
        if let Some(o) = &self.origin {
            write!(f, "@{o}")?;
        }
        Ok(())
    }
}
