//! # scootlens-mcp
//!
//! MCP server：ABI 投影层，零业务逻辑（ADR-0005、docs/03-abi-spec.md §MCP 投影）。
//!
//! - **工具清单**从 [`scootlens_abi::method::ALL`] 生成：`scootlens_<domain>_<verb>`
//! - **运行拓扑**：MCP 客户端 spawn `scootlens-mcp`（stdio）→ 本进程以 capability
//!   令牌连接 scootlensd gateway（WS JSON-RPC）→ 工具调用逐一转发为 ABI 调用
//! - **零授权能力**：本层不做任何权限判断；作用域校验、限速、人工审批全部由
//!   内核 Security Manager 强制，客户端无法经由 MCP 层获得额外权限

mod abi_client;
mod projection;
mod server;

pub use abi_client::{AbiClient, CallError};
pub use projection::{
    EXCLUDED_METHODS, ToolDef, input_schema, method_for_tool, tool_defs, tool_name,
};
pub use server::ScootLensMcp;
