//! ABI 错误码表（docs/03-abi-spec.md）。

use serde::{Deserialize, Serialize};
use serde_json::json;

/// ABI 错误码。新增/删除必须走 ADR 并更新契约 golden。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ErrorCode {
    #[serde(rename = "E_CAP_DENIED")]
    CapDenied,
    #[serde(rename = "E_APPROVAL_PENDING")]
    ApprovalPending,
    #[serde(rename = "E_PROC_NOT_FOUND")]
    ProcNotFound,
    #[serde(rename = "E_REF_STALE")]
    RefStale,
    #[serde(rename = "E_TIMEOUT")]
    Timeout,
    #[serde(rename = "E_NET_BLOCKED")]
    NetBlocked,
    #[serde(rename = "E_ENGINE_CRASH")]
    EngineCrash,
    #[serde(rename = "E_UNSUPPORTED")]
    Unsupported,
    #[serde(rename = "E_INVALID_ARG")]
    InvalidArg,
    #[serde(rename = "E_QUOTA")]
    Quota,
    #[serde(rename = "E_INTERNAL")]
    Internal,
}

impl ErrorCode {
    /// 全表（顺序即文档顺序，进入契约 golden）。
    pub const ALL: &[ErrorCode] = &[
        ErrorCode::CapDenied,
        ErrorCode::ApprovalPending,
        ErrorCode::ProcNotFound,
        ErrorCode::RefStale,
        ErrorCode::Timeout,
        ErrorCode::NetBlocked,
        ErrorCode::EngineCrash,
        ErrorCode::Unsupported,
        ErrorCode::InvalidArg,
        ErrorCode::Quota,
        ErrorCode::Internal,
    ];

    pub fn as_str(&self) -> &'static str {
        match self {
            ErrorCode::CapDenied => "E_CAP_DENIED",
            ErrorCode::ApprovalPending => "E_APPROVAL_PENDING",
            ErrorCode::ProcNotFound => "E_PROC_NOT_FOUND",
            ErrorCode::RefStale => "E_REF_STALE",
            ErrorCode::Timeout => "E_TIMEOUT",
            ErrorCode::NetBlocked => "E_NET_BLOCKED",
            ErrorCode::EngineCrash => "E_ENGINE_CRASH",
            ErrorCode::Unsupported => "E_UNSUPPORTED",
            ErrorCode::InvalidArg => "E_INVALID_ARG",
            ErrorCode::Quota => "E_QUOTA",
            ErrorCode::Internal => "E_INTERNAL",
        }
    }

    /// JSON-RPC 数值错误码映射。`E_INVALID_ARG` 复用标准 -32602。
    pub fn json_rpc_code(&self) -> i64 {
        match self {
            ErrorCode::InvalidArg => -32602,
            ErrorCode::CapDenied => -32001,
            ErrorCode::ApprovalPending => -32002,
            ErrorCode::ProcNotFound => -32003,
            ErrorCode::RefStale => -32004,
            ErrorCode::Timeout => -32005,
            ErrorCode::NetBlocked => -32006,
            ErrorCode::EngineCrash => -32007,
            ErrorCode::Unsupported => -32008,
            ErrorCode::Quota => -32009,
            ErrorCode::Internal => -32010,
        }
    }
}

impl std::fmt::Display for ErrorCode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// ABI 层错误：错误码 + 人类可读消息。
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error, Serialize, Deserialize)]
#[error("{code}: {message}")]
pub struct AbiError {
    pub code: ErrorCode,
    pub message: String,
}

impl AbiError {
    pub fn new(code: ErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }

    /// 转为 JSON-RPC error 对象：数值码 + 消息，`data.code` 携带字符串码供机器解析。
    pub fn to_rpc_error(&self) -> RpcError {
        RpcError {
            code: self.code.json_rpc_code(),
            message: self.message.clone(),
            data: json!({ "code": self.code.as_str() }),
        }
    }
}

/// JSON-RPC 2.0 error 对象。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RpcError {
    pub code: i64,
    pub message: String,
    pub data: serde_json::Value,
}
