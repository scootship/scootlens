//! 契约测试：锁定 ABI 线上格式（docs/03-abi-spec.md）。
//!
//! TDD 红线：任何 ABI 变更必须先改这里的断言/golden，并附 ADR。

use scootlens_abi::{
    ABI_VERSION, AbiError, ApprovalMode, ElementRef, ErrorCode, NetAction, NetDecision, NetRule,
    NetRuleSet, Pid, QuotaPolicy, QuotaSpec, RpcId, RpcNotification, RpcRequest, RpcResponse,
    Scope, SnapId, TokenClaims, TokenConstraints, WfRetry, WfSpec, WfStep, WfTrigger, is_sensitive,
    method, origin_matches,
};
use serde_json::json;

// ---------- ID 类型 ----------

#[test]
fn pid_roundtrip_and_validation() {
    let pid: Pid = "p-a1b2c3".parse().expect("valid pid");
    assert_eq!(pid.to_string(), "p-a1b2c3");
    let js = serde_json::to_string(&pid).expect("ser");
    assert_eq!(js, "\"p-a1b2c3\"");
    let back: Pid = serde_json::from_str(&js).expect("de");
    assert_eq!(back, pid);

    for bad in ["", "p-", "x-abc", "p-白", "p-a b", "abc"] {
        assert!(bad.parse::<Pid>().is_err(), "should reject {bad:?}");
        assert!(
            serde_json::from_value::<Pid>(json!(bad)).is_err(),
            "serde should reject {bad:?}"
        );
    }
}

#[test]
fn element_ref_roundtrip_and_staleness() {
    let r: ElementRef = "s3e17".parse().expect("valid ref");
    assert_eq!((r.generation(), r.index()), (3, 17));
    assert_eq!(r.to_string(), "s3e17");
    assert_eq!(
        serde_json::to_value(r.clone()).expect("ser"),
        json!("s3e17")
    );

    assert!(!r.is_stale(3));
    assert!(r.is_stale(4));

    for bad in ["", "s3", "e17", "s3e", "sxey", "3e17", "s-1e2", "s3e17z"] {
        assert!(bad.parse::<ElementRef>().is_err(), "should reject {bad:?}");
    }
}

#[test]
fn snap_id_roundtrip_and_validation() {
    let id: SnapId = "snap-9f2ab04c".parse().expect("valid snap id");
    assert_eq!(id.to_string(), "snap-9f2ab04c");
    let js = serde_json::to_string(&id).expect("ser");
    assert_eq!(js, "\"snap-9f2ab04c\"");
    let back: SnapId = serde_json::from_str(&js).expect("de");
    assert_eq!(back, id);

    for bad in ["", "snap-", "p-abc", "snap-ABC", "snap-a b", "9f2ab04c"] {
        assert!(bad.parse::<SnapId>().is_err(), "should reject {bad:?}");
        assert!(
            serde_json::from_value::<SnapId>(json!(bad)).is_err(),
            "serde should reject {bad:?}"
        );
    }
}

// ---------- 错误码 ----------

#[test]
fn error_code_table_is_locked() {
    let table: Vec<_> = ErrorCode::ALL
        .iter()
        .map(|c| json!({"str": c.as_str(), "rpc": c.json_rpc_code()}))
        .collect();
    insta::assert_json_snapshot!("error_code_table", table);
}

#[test]
fn abi_error_serializes_with_code_str() {
    let err = AbiError::new(ErrorCode::RefStale, "generation 3 expired");
    let rpc = err.to_rpc_error();
    insta::assert_json_snapshot!("abi_error_ref_stale", rpc);
}

// ---------- JSON-RPC 2.0 封装 ----------

#[test]
fn rpc_request_wire_format() {
    let req = RpcRequest::new(
        RpcId::Num(7),
        method::ACT_CLICK,
        json!({"pid": "p-a1b2c3", "ref": "s3e17"}),
    );
    insta::assert_json_snapshot!("rpc_request_act_click", req);

    let wire = serde_json::to_value(&req).expect("ser");
    assert_eq!(wire["jsonrpc"], "2.0");
    let back: RpcRequest = serde_json::from_value(wire).expect("de");
    assert_eq!(back, req);
}

#[test]
fn rpc_request_rejects_wrong_version() {
    let r = serde_json::from_value::<RpcRequest>(
        json!({"jsonrpc": "1.0", "id": 1, "method": "sys.info"}),
    );
    assert!(r.is_err());
}

#[test]
fn rpc_response_success_and_failure() {
    let ok = RpcResponse::success(RpcId::Str("req-1".into()), json!({"pid": "p-x1"}));
    insta::assert_json_snapshot!("rpc_response_success", ok);

    let fail = RpcResponse::failure(
        RpcId::Num(2),
        AbiError::new(ErrorCode::CapDenied, "missing scope act@example.com"),
    );
    insta::assert_json_snapshot!("rpc_response_failure", fail);

    // result 与 error 互斥
    let v = serde_json::to_value(&fail).expect("ser");
    assert!(v.get("result").is_none());
    assert!(v.get("error").is_some());
}

#[test]
fn rpc_notification_has_no_id() {
    let n = RpcNotification::new(
        "evt.proc.lifecycle",
        json!({"pid": "p-x1", "state": "crashed"}),
    );
    let v = serde_json::to_value(&n).expect("ser");
    assert!(v.get("id").is_none());
    insta::assert_json_snapshot!("rpc_notification_lifecycle", n);
}

// ---------- 方法表 ----------

#[test]
fn method_table_is_locked() {
    // 系统调用表 v0：新增/改名必须走 ADR，golden 变更即为信号
    insta::assert_json_snapshot!("method_table", method::ALL);
}

#[test]
fn method_lookup() {
    assert!(method::is_known("proc.spawn"));
    assert!(method::is_known("view.snapshot"));
    assert!(!method::is_known("proc.hack"));
}

// ---------- 配额与工作流（P3）----------

#[test]
fn quota_spec_wire_format() {
    let q = QuotaSpec {
        max_memory_bytes: 512 * 1024 * 1024,
        on_exceed: QuotaPolicy::Suspend,
    };
    insta::assert_json_snapshot!("quota_spec", q);

    // on_exceed 缺省为 warn
    let d: QuotaSpec = serde_json::from_value(json!({ "max_memory_bytes": 1 })).expect("de");
    assert_eq!(d.on_exceed, QuotaPolicy::Warn);
}

#[test]
fn wf_spec_wire_format() {
    let spec = WfSpec {
        name: "patrol".into(),
        trigger: WfTrigger::Cron {
            expr: "*/5 * * * *".into(),
        },
        steps: vec![WfStep {
            method: "proc.list".into(),
            params: json!({}),
            retry: WfRetry {
                max_attempts: 2,
                backoff_ms: 100,
            },
        }],
        scopes: vec!["proc:list".into()],
    };
    insta::assert_json_snapshot!("wf_spec", spec);

    // retry / params 可缺省
    let d: WfSpec = serde_json::from_value(json!({
        "name": "n",
        "trigger": { "kind": "manual" },
        "steps": [{ "method": "proc.list" }],
        "scopes": [],
    }))
    .expect("de");
    assert_eq!(d.steps[0].retry.max_attempts, 0);
    assert_eq!(d.trigger, WfTrigger::Manual);
    let ev: WfTrigger =
        serde_json::from_value(json!({ "kind": "event", "topic": "nav" })).expect("de");
    assert_eq!(
        ev,
        WfTrigger::Event {
            topic: "nav".into()
        }
    );
}

#[test]
fn abi_version_is_semver_like() {
    assert_eq!(ABI_VERSION.split('.').count(), 3);
}

// ---------- 作用域（P2）----------

#[test]
fn scope_parse_roundtrip() {
    for s in [
        "*",
        "proc:spawn",
        "act@*.github.com",
        "state:read:cookies@github.com",
        "js:exec@localhost:*",
        "nav@app.test:8080",
        "cap:admin",
    ] {
        let scope: Scope = s.parse().expect(s);
        assert_eq!(scope.to_string(), s);
        let js = serde_json::to_value(&scope).expect("ser");
        assert_eq!(js, json!(s));
        let back: Scope = serde_json::from_value(js).expect("de");
        assert_eq!(back, scope);
    }
}

#[test]
fn scope_rejects_malformed() {
    for bad in [
        "",
        "@example.com",
        "act@",
        "a b",
        "act:@x",
        "act:*",         // `*` 只能是整个作用域
        "*@example.com", // 超级作用域不带 origin
        "act@a@b",
        "ACT@example.com", // 段必须小写
    ] {
        assert!(bad.parse::<Scope>().is_err(), "should reject {bad:?}");
    }
}

#[test]
fn scope_covers_matrix() {
    let cases: &[(&str, &str, Option<&str>, bool)] = &[
        // (grant, required-segments 以 : 连接, required-origin, 期望)
        ("*", "js:exec", Some("any.test"), true),
        ("act", "act", Some("a.test"), true), // 无 origin 授权 = 任意 origin
        ("act@*.example.com", "act", Some("app.example.com"), true),
        ("act@*.example.com", "act", Some("example.com"), false), // 严格子域
        ("act@example.com", "act", Some("example.com"), true),
        ("act@example.com", "act", Some("unrelated.test"), false),
        ("act@app.test", "nav", Some("app.test"), false), // 域不同
        ("state:read", "state:read:cookies", Some("github.com"), true), // 前缀覆盖
        ("state:read:cookies", "state:read", None, false), // 更细不能覆盖更泛
        (
            "js:exec@localhost:*",
            "js:exec",
            Some("localhost:3000"),
            true,
        ),
        ("js:exec@localhost:*", "js:exec", Some("localhost"), true),
        (
            "js:exec@localhost:3000",
            "js:exec",
            Some("localhost:8080"),
            false,
        ),
        ("act@app.test", "act", None, false), // 有 origin 的授权不覆盖无 origin 要求
        ("proc:spawn", "proc:spawn", None, true),
    ];
    for (grant, req_body, req_origin, want) in cases {
        let grant: Scope = grant.parse().expect("grant");
        let segs: Vec<&str> = req_body.split(':').collect();
        let required = Scope::required(&segs, *req_origin);
        assert_eq!(
            grant.covers(&required),
            *want,
            "grant={grant} required={required}"
        );
    }
}

#[test]
fn origin_pattern_matching() {
    assert!(origin_matches("*", "anything.test"));
    assert!(origin_matches("*.github.com", "gist.github.com"));
    assert!(!origin_matches("*.github.com", "github.com"));
    assert!(!origin_matches("*.github.com", "notgithub.com"));
    assert!(origin_matches("GitHub.com", "github.com")); // host 大小写不敏感
    assert!(origin_matches("localhost:*", "localhost:9910"));
    assert!(!origin_matches("localhost:9910", "localhost"));
}

#[test]
fn sensitive_scope_set() {
    for s in [
        "js:exec@x.test",
        "state:read:cookies@a.b",
        "cap:admin",
        "vault:use",
    ] {
        let scope: Scope = s.parse().expect(s);
        assert!(is_sensitive(&scope), "{s} should be sensitive");
    }
    for s in ["nav@a.test", "view@a.test", "proc:spawn", "state:list:proc"] {
        let scope: Scope = s.parse().expect(s);
        assert!(!is_sensitive(&scope), "{s} should not be sensitive");
    }
}

// ---------- 令牌 claims（P2）----------

#[test]
fn token_claims_wire_format() {
    let claims = TokenClaims {
        subject: "agent:ops-bot-1".into(),
        scopes: vec![
            "nav@*.example.com".parse().expect("scope"),
            "vault:use".parse().expect("scope"),
        ],
        constraints: TokenConstraints {
            expires_at: Some(1_900_000_000),
            rate: Some("60/min".into()),
            approval: [("js:exec".to_owned(), ApprovalMode::Manual)].into(),
        },
        issued_by: "user:admin".into(),
        issued_at: 1_800_000_000,
    };
    insta::assert_json_snapshot!("token_claims", claims);
    let v = serde_json::to_value(&claims).expect("ser");
    let back: TokenClaims = serde_json::from_value(v).expect("de");
    assert_eq!(back, claims);
}

// ---------- 网络规则类型（P2）----------

#[test]
fn net_rule_wire_format() {
    let rules = NetRuleSet {
        default: scootlens_abi::NetDefault::Allow,
        rules: vec![NetRule {
            action: NetAction::Deny,
            host: "*.denied.test".into(),
            methods: vec!["POST".into()],
            resource_types: vec!["xhr".into()],
            set_headers: vec![],
        }],
    };
    insta::assert_json_snapshot!("net_rule_set", rules);
    let v = serde_json::to_value(&rules).expect("ser");
    let back: NetRuleSet = serde_json::from_value(v).expect("de");
    assert_eq!(back, rules);

    let d = NetDecision::Allow {
        set_headers: vec![],
    };
    assert!(d.allowed());
    assert!(!NetDecision::Deny.allowed());
}
