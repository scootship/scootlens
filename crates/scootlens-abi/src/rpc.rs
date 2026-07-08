//! JSON-RPC 2.0 封装（传输层格式，docs/03-abi-spec.md）。

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::error::{AbiError, RpcError};

/// `"jsonrpc": "2.0"` 版本标记：反序列化时强校验。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct V2;

impl Serialize for V2 {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str("2.0")
    }
}

impl<'de> Deserialize<'de> for V2 {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let v = String::deserialize(d)?;
        if v == "2.0" {
            Ok(V2)
        } else {
            Err(serde::de::Error::custom(format!(
                "unsupported jsonrpc version: {v}"
            )))
        }
    }
}

/// 请求/响应 ID。
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(untagged)]
pub enum RpcId {
    Num(i64),
    Str(String),
}

/// JSON-RPC 请求。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RpcRequest {
    pub jsonrpc: V2,
    pub id: RpcId,
    pub method: String,
    #[serde(default, skip_serializing_if = "Value::is_null")]
    pub params: Value,
}

impl RpcRequest {
    pub fn new(id: RpcId, method: impl Into<String>, params: Value) -> Self {
        Self {
            jsonrpc: V2,
            id,
            method: method.into(),
            params,
        }
    }
}

/// JSON-RPC 响应：`result` 与 `error` 互斥。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RpcResponse {
    pub jsonrpc: V2,
    pub id: RpcId,
    #[serde(flatten)]
    pub outcome: RpcOutcome,
}

/// 响应结果体。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum RpcOutcome {
    Success { result: Value },
    Failure { error: RpcError },
}

impl RpcResponse {
    pub fn success(id: RpcId, result: Value) -> Self {
        Self {
            jsonrpc: V2,
            id,
            outcome: RpcOutcome::Success { result },
        }
    }

    pub fn failure(id: RpcId, err: AbiError) -> Self {
        Self {
            jsonrpc: V2,
            id,
            outcome: RpcOutcome::Failure {
                error: err.to_rpc_error(),
            },
        }
    }
}

/// JSON-RPC 通知（服务端事件推送，无 id）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RpcNotification {
    pub jsonrpc: V2,
    pub method: String,
    #[serde(default, skip_serializing_if = "Value::is_null")]
    pub params: Value,
}

impl RpcNotification {
    pub fn new(method: impl Into<String>, params: Value) -> Self {
        Self {
            jsonrpc: V2,
            method: method.into(),
            params,
        }
    }
}
