//! rmcp [`ServerHandler`]：工具清单 = ABI 投影，调用 = 转发。

use rmcp::handler::server::ServerHandler;
use rmcp::model::{
    CallToolRequestParams, CallToolResult, ContentBlock, ErrorData, ListToolsResult,
    PaginatedRequestParams, ServerCapabilities, ServerInfo, Tool,
};
use rmcp::service::{RequestContext, RoleServer};
use serde_json::{Value, json};

use crate::abi_client::{AbiClient, CallError};
use crate::projection;

/// ScootLens MCP server（ABI 投影层）。
#[derive(Clone)]
pub struct ScootLensMcp {
    client: AbiClient,
}

impl ScootLensMcp {
    pub fn new(client: AbiClient) -> Self {
        Self { client }
    }

    /// 投影工具清单 → rmcp Tool 列表。
    pub fn tools() -> Vec<Tool> {
        projection::tool_defs()
            .into_iter()
            .map(|d| Tool::new(d.name, d.description, projection::input_schema(d.method)))
            .collect()
    }
}

impl ServerHandler for ScootLensMcp {
    fn get_info(&self) -> ServerInfo {
        let mut info = ServerInfo::default();
        info.capabilities = ServerCapabilities::builder().enable_tools().build();
        info.server_info.name = "scootlens-mcp".into();
        info.server_info.version = env!("CARGO_PKG_VERSION").into();
        info.instructions = Some(
            "ScootLens ABI projection. Tools mirror the kernel syscall table \
             (scootlens_<domain>_<verb>); authorization and approvals are enforced \
             by the kernel capability model — E_CAP_DENIED means the token lacks \
             scope, E_APPROVAL_PENDING means a human approval is still pending."
                .into(),
        );
        info
    }

    async fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, ErrorData> {
        Ok(ListToolsResult::with_all_items(Self::tools()))
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        let Some(method) = projection::method_for_tool(&request.name) else {
            return Err(ErrorData::invalid_params(
                format!("unknown tool: {}", request.name),
                None,
            ));
        };
        let params = request
            .arguments
            .map(Value::Object)
            .unwrap_or_else(|| json!({}));
        match self.client.call(method, params).await {
            Ok(result) => Ok(CallToolResult::success(vec![ContentBlock::text(
                serde_json::to_string_pretty(&result).unwrap_or_else(|_| "null".into()),
            )])),
            // 内核拒绝/失败 = 工具级错误（对模型可见，含 ABI 错误码），非协议错误
            Err(e @ CallError::Rpc { .. }) => {
                let payload = json!({
                    "error": {
                        "abi_code": e.abi_code(),
                        "message": e.to_string(),
                    }
                });
                Ok(CallToolResult::error(vec![ContentBlock::text(
                    serde_json::to_string_pretty(&payload).unwrap_or_else(|_| e.to_string()),
                )]))
            }
            Err(CallError::Transport(msg)) => Err(ErrorData::internal_error(
                format!("gateway transport: {msg}"),
                None,
            )),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tool_list_matches_projection() {
        let tools = ScootLensMcp::tools();
        let defs = projection::tool_defs();
        assert_eq!(tools.len(), defs.len());
        for (t, d) in tools.iter().zip(defs.iter()) {
            assert_eq!(t.name, d.name);
            assert_eq!(t.input_schema["type"], "object");
            assert!(t.description.as_deref().is_some_and(|s| !s.is_empty()));
        }
    }
}
