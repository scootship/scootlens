//! # scootlens-abi
//!
//! ScootLens ABI 协议真源：ID 类型、错误码、JSON-RPC 2.0 封装、系统调用方法表。
//! 规范见 `docs/03-abi-spec.md`；任何变更需先改契约测试并附 ADR。
//!
//! 本 crate 零内部依赖（依赖规则见 `docs/02-architecture.md`）。

mod cap;
mod error;
mod id;
pub mod method;
mod net;
mod quota;
mod replay;
mod rpc;
mod scope;
mod wf;

pub use cap::{
    ApprovalDecision, ApprovalMode, PendingApproval, SENSITIVE_SCOPES, TOKEN_PREFIX, TokenClaims,
    TokenConstraints, is_sensitive,
};
pub use error::{AbiError, ErrorCode, RpcError};
pub use id::{ElementRef, Pid, SnapId};
pub use net::{
    NetAction, NetDecision, NetDefault, NetHeader, NetRequestSummary, NetRule, NetRuleSet,
};
pub use quota::{QuotaPolicy, QuotaSpec};
pub use replay::{REPLAY_FORMAT_VERSION, ReplayBundle, ReplayFrame, ReplayLine};
pub use rpc::{RpcId, RpcNotification, RpcOutcome, RpcRequest, RpcResponse, V2};
pub use scope::{ParseScopeError, Scope, origin_matches};
pub use wf::{WfRetry, WfSpec, WfStep, WfTrigger};

/// ABI 版本。v0 期间允许破坏性变更（需 ADR）；v1 起向后兼容。
pub const ABI_VERSION: &str = "0.2.0";
