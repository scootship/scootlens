//! 契约测试：锁定 ABI 线上格式（docs/03-abi-spec.md）。
//!
//! TDD 红线：任何 ABI 变更必须先改这里的断言/golden，并附 ADR。

use scootlens_abi::{
    ABI_VERSION, AbiError, ElementRef, ErrorCode, Pid, RpcId, RpcNotification, RpcRequest,
    RpcResponse, method,
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

#[test]
fn abi_version_is_semver_like() {
    assert_eq!(ABI_VERSION.split('.').count(), 3);
}
