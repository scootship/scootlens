//! # scootlens-net
//!
//! 网络规则求值引擎与请求日志类型（docs/04-kernel-design.md §4.6）。
//!
//! 规则类型是 ABI 协议表面（定义在 `scootlens-abi`）；本 crate 提供纯求值逻辑：
//!
//! - 单规则命中：host 模式 × 方法 × 资源类型（各维度求与）
//! - 规则集求值：首条命中规则决定动作；无命中走默认策略
//! - 层级组合：proc 级规则优先于全局级（`evaluate_layered`）
//!
//! 强制点在驱动（P2 经 CDP Fetch 拦截），驱动通过 HAL `RequestPolicy` 回调
//! 询问内核，内核用本引擎求值——单一强制引擎，驱动零规则逻辑。

use scootlens_abi::{
    NetAction, NetDecision, NetDefault, NetHeader, NetRequestSummary, NetRule, NetRuleSet,
    origin_matches,
};

/// 单条规则是否命中请求。
fn rule_matches(rule: &NetRule, req: &NetRequestSummary, host: &str) -> bool {
    if !origin_matches(&rule.host, host) {
        return false;
    }
    if !rule.methods.is_empty()
        && !rule
            .methods
            .iter()
            .any(|m| m.eq_ignore_ascii_case(&req.method))
    {
        return false;
    }
    if !rule.resource_types.is_empty()
        && !rule
            .resource_types
            .iter()
            .any(|t| t.eq_ignore_ascii_case(&req.resource_type))
    {
        return false;
    }
    true
}

/// 从 URL 提取匹配用 host（含显式端口）。非法 URL 返回 None（调用方应拒绝）。
pub fn host_of(url: &str) -> Option<String> {
    let parsed = url::Url::parse(url).ok()?;
    let host = parsed.host_str()?;
    Some(match parsed.port() {
        Some(p) => format!("{host}:{p}"),
        None => host.to_owned(),
    })
}

/// 对单个规则集求值。首条命中规则决定；无命中走 `default`。
pub fn evaluate(set: &NetRuleSet, req: &NetRequestSummary) -> NetDecision {
    let Some(host) = host_of(&req.url) else {
        // 无 host 的请求（data:/about: 等）不经网络栈放行；恶意 scheme 由导航层拦截
        return NetDecision::Allow {
            set_headers: vec![],
        };
    };
    let mut headers: Vec<NetHeader> = Vec::new();
    for rule in &set.rules {
        if !rule_matches(rule, req, &host) {
            continue;
        }
        match rule.action {
            NetAction::Deny => return NetDecision::Deny,
            NetAction::Allow => {
                headers.extend(rule.set_headers.iter().cloned());
                return NetDecision::Allow {
                    set_headers: headers,
                };
            }
        }
    }
    match set.default {
        NetDefault::Allow => NetDecision::Allow {
            set_headers: vec![],
        },
        NetDefault::Deny => NetDecision::Deny,
    }
}

/// 层级求值：proc 级规则先行；proc 级无命中时落到全局级。
///
/// 注意：proc 级的**默认策略**只在没有全局层时生效——有全局层时，
/// proc 层仅由显式规则决定，未命中继续走全局层（默认策略语义见 docs/04 §4.6）。
pub fn evaluate_layered(
    proc_rules: Option<&NetRuleSet>,
    global: Option<&NetRuleSet>,
    req: &NetRequestSummary,
) -> NetDecision {
    let Some(host) = host_of(&req.url) else {
        return NetDecision::Allow {
            set_headers: vec![],
        };
    };
    let mut headers: Vec<NetHeader> = Vec::new();
    for layer in [proc_rules, global].into_iter().flatten() {
        for rule in &layer.rules {
            if !rule_matches(rule, req, &host) {
                continue;
            }
            match rule.action {
                NetAction::Deny => return NetDecision::Deny,
                NetAction::Allow => {
                    headers.extend(rule.set_headers.iter().cloned());
                    return NetDecision::Allow {
                        set_headers: headers,
                    };
                }
            }
        }
    }
    // 无任何命中：默认策略取"最严"层——任一层要求白名单模式即拒
    let default_deny = proc_rules
        .map(|s| s.default == NetDefault::Deny)
        .unwrap_or(false)
        || global
            .map(|s| s.default == NetDefault::Deny)
            .unwrap_or(false);
    if default_deny {
        NetDecision::Deny
    } else {
        NetDecision::Allow {
            set_headers: vec![],
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn req(url: &str, method: &str, rt: &str) -> NetRequestSummary {
        NetRequestSummary {
            url: url.into(),
            method: method.into(),
            resource_type: rt.into(),
        }
    }

    fn deny(host: &str) -> NetRule {
        NetRule {
            action: NetAction::Deny,
            host: host.into(),
            methods: vec![],
            resource_types: vec![],
            set_headers: vec![],
        }
    }

    fn allow(host: &str) -> NetRule {
        NetRule {
            action: NetAction::Allow,
            host: host.into(),
            methods: vec![],
            resource_types: vec![],
            set_headers: vec![],
        }
    }

    #[test]
    fn default_allow_when_no_rules() {
        let set = NetRuleSet::default();
        assert!(evaluate(&set, &req("http://a.test/", "GET", "document")).allowed());
    }

    #[test]
    fn denylist_blocks_matching_host() {
        let set = NetRuleSet {
            default: NetDefault::Allow,
            rules: vec![deny("*.evil.test")],
        };
        assert_eq!(
            evaluate(&set, &req("http://x.evil.test/p", "GET", "document")),
            NetDecision::Deny
        );
        assert!(evaluate(&set, &req("http://good.test/", "GET", "document")).allowed());
    }

    #[test]
    fn whitelist_mode_denies_unlisted() {
        let set = NetRuleSet {
            default: NetDefault::Deny,
            rules: vec![allow("app.test")],
        };
        assert!(evaluate(&set, &req("http://app.test/", "GET", "document")).allowed());
        assert_eq!(
            evaluate(&set, &req("http://other.test/", "GET", "document")),
            NetDecision::Deny
        );
    }

    #[test]
    fn first_match_wins() {
        let set = NetRuleSet {
            default: NetDefault::Allow,
            rules: vec![deny("a.test"), allow("a.test")],
        };
        assert_eq!(
            evaluate(&set, &req("http://a.test/", "GET", "document")),
            NetDecision::Deny
        );
    }

    #[test]
    fn method_and_resource_type_constrain() {
        let set = NetRuleSet {
            default: NetDefault::Allow,
            rules: vec![NetRule {
                action: NetAction::Deny,
                host: "api.test".into(),
                methods: vec!["POST".into()],
                resource_types: vec!["xhr".into()],
                set_headers: vec![],
            }],
        };
        assert_eq!(
            evaluate(&set, &req("http://api.test/x", "POST", "xhr")),
            NetDecision::Deny
        );
        // 方法不同 → 放行
        assert!(evaluate(&set, &req("http://api.test/x", "GET", "xhr")).allowed());
        // 资源类型不同 → 放行
        assert!(evaluate(&set, &req("http://api.test/x", "POST", "document")).allowed());
        // 方法大小写不敏感
        assert_eq!(
            evaluate(&set, &req("http://api.test/x", "post", "XHR")),
            NetDecision::Deny
        );
    }

    #[test]
    fn allow_rule_injects_headers() {
        let set = NetRuleSet {
            default: NetDefault::Deny,
            rules: vec![NetRule {
                action: NetAction::Allow,
                host: "app.test".into(),
                methods: vec![],
                resource_types: vec![],
                set_headers: vec![NetHeader {
                    name: "X-Scoot".into(),
                    value: "1".into(),
                }],
            }],
        };
        match evaluate(&set, &req("http://app.test/", "GET", "document")) {
            NetDecision::Allow { set_headers } => {
                assert_eq!(set_headers.len(), 1);
                assert_eq!(set_headers[0].name, "X-Scoot");
            }
            NetDecision::Deny => panic!("expected allow"),
        }
    }

    #[test]
    fn host_with_port_matching() {
        let set = NetRuleSet {
            default: NetDefault::Deny,
            rules: vec![allow("localhost:*")],
        };
        assert!(evaluate(&set, &req("http://localhost:9910/ws", "GET", "xhr")).allowed());
        assert!(evaluate(&set, &req("http://localhost/", "GET", "xhr")).allowed());
        assert_eq!(
            evaluate(&set, &req("http://remote.test/", "GET", "xhr")),
            NetDecision::Deny
        );
    }

    #[test]
    fn layered_proc_overrides_global() {
        let global = NetRuleSet {
            default: NetDefault::Allow,
            rules: vec![deny("blocked.test")],
        };
        let proc = NetRuleSet {
            default: NetDefault::Allow,
            rules: vec![allow("blocked.test"), deny("proc-only.test")],
        };
        // proc allow 先命中 → 放行（覆盖全局 deny）
        assert!(
            evaluate_layered(
                Some(&proc),
                Some(&global),
                &req("http://blocked.test/", "GET", "xhr")
            )
            .allowed()
        );
        // proc deny 命中
        assert_eq!(
            evaluate_layered(
                Some(&proc),
                Some(&global),
                &req("http://proc-only.test/", "GET", "xhr")
            ),
            NetDecision::Deny
        );
        // proc 无命中 → 落到全局
        assert_eq!(
            evaluate_layered(
                Some(&NetRuleSet::default()),
                Some(&global),
                &req("http://blocked.test/", "GET", "xhr")
            ),
            NetDecision::Deny
        );
        // 双层均无命中 → 默认放行
        assert!(
            evaluate_layered(
                Some(&proc),
                Some(&global),
                &req("http://free.test/", "GET", "xhr")
            )
            .allowed()
        );
    }

    #[test]
    fn layered_default_deny_is_strictest() {
        let global = NetRuleSet {
            default: NetDefault::Deny,
            rules: vec![allow("app.test")],
        };
        assert!(
            evaluate_layered(None, Some(&global), &req("http://app.test/", "GET", "xhr")).allowed()
        );
        assert_eq!(
            evaluate_layered(None, Some(&global), &req("http://x.test/", "GET", "xhr")),
            NetDecision::Deny
        );
    }

    #[test]
    fn schemeless_or_hostless_urls_bypass() {
        let set = NetRuleSet {
            default: NetDefault::Deny,
            rules: vec![],
        };
        assert!(evaluate(&set, &req("about:blank", "GET", "document")).allowed());
        assert!(evaluate(&set, &req("data:text/html,hi", "GET", "document")).allowed());
    }

    #[test]
    fn host_of_extracts_explicit_port_only() {
        assert_eq!(host_of("https://a.test/p"), Some("a.test".into()));
        assert_eq!(host_of("http://a.test:8080/"), Some("a.test:8080".into()));
        assert_eq!(host_of("about:blank"), None);
        assert_eq!(host_of("not a url"), None);
    }
}
