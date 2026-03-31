use std::future::Future;
use std::path::PathBuf;

use rmcp::{
    ErrorData as McpError, ServerHandler,
    model::{
        CallToolRequestMethod, CallToolRequestParams, CallToolResult, ListToolsResult,
        PaginatedRequestParams, ServerCapabilities, ServerInfo,
    },
    service::RequestContext,
};
use serde_json::{Map as JsonMap, Value as JsonValue};

use crate::daemon;
use crate::types::CachedToolsetRecord;

use super::tools::{
    ACTIVATE_EXTERNAL_MCP_NAME, ACTIVATE_EXTERNAL_MCP_TOOL_NAME, ActivateExternalMcpRequest,
    ActivateExternalMcpToolRequest, CALL_TOOL_IN_EXTERNAL_MCP_NAME, CallToolInExternalMcpRequest,
    ToolCatalog, build_activate_tool_detail_result, build_activate_tool_result,
    parse_tool_arguments_json, parse_tool_request, resolve_tool_snapshot_or_error,
    resolve_toolset_or_error,
};

pub(super) struct SmartProxyMcpServer {
    config_path: PathBuf,
    tools: ToolCatalog,
    toolsets: Vec<CachedToolsetRecord>,
}

impl SmartProxyMcpServer {
    pub(super) fn new(config_path: PathBuf, toolsets: Vec<CachedToolsetRecord>) -> Self {
        let tools = ToolCatalog::new(&toolsets);
        Self {
            config_path,
            tools,
            toolsets,
        }
    }

    async fn call_activate_tool(
        &self,
        arguments: JsonMap<String, JsonValue>,
    ) -> Result<CallToolResult, McpError> {
        let params: ActivateExternalMcpRequest =
            parse_tool_request(ACTIVATE_EXTERNAL_MCP_NAME, arguments)?;
        let toolset = resolve_toolset_or_error(&self.toolsets, &params.external_mcp_name)?;
        Ok(build_activate_tool_result(toolset))
    }

    async fn call_activate_tool_detail(
        &self,
        arguments: JsonMap<String, JsonValue>,
    ) -> Result<CallToolResult, McpError> {
        let params: ActivateExternalMcpToolRequest =
            parse_tool_request(ACTIVATE_EXTERNAL_MCP_TOOL_NAME, arguments)?;
        let toolset = resolve_toolset_or_error(&self.toolsets, &params.external_mcp_name)?;
        let tool = resolve_tool_snapshot_or_error(toolset, &params.tool_name)?;
        Ok(build_activate_tool_detail_result(tool))
    }

    async fn call_external_tool(
        &self,
        arguments: JsonMap<String, JsonValue>,
    ) -> Result<CallToolResult, McpError> {
        let params: CallToolInExternalMcpRequest =
            parse_tool_request(CALL_TOOL_IN_EXTERNAL_MCP_NAME, arguments)?;
        let toolset = resolve_toolset_or_error(&self.toolsets, &params.external_mcp_name)?;
        let arguments = parse_tool_arguments_json(&params.args_in_json)?;

        self.call_downstream_tool(toolset, params.tool_name, arguments)
            .await
    }

    async fn call_downstream_tool(
        &self,
        toolset: &CachedToolsetRecord,
        tool_name: String,
        arguments: Option<JsonMap<String, JsonValue>>,
    ) -> Result<CallToolResult, McpError> {
        daemon::call_tool(
            &self.config_path,
            None,
            &toolset.name,
            &tool_name,
            arguments,
        )
        .await
        .map_err(|error| McpError::internal_error(error.to_string(), None))
    }
}

impl ServerHandler for SmartProxyMcpServer {
    fn get_info(&self) -> ServerInfo {
        let mut info = ServerInfo::default();
        info.instructions = Some(
            "Use `activate_external_mcp` to inspect cached tool names, `activate_external_mcp_tool` to inspect one full cached tool definition, then `call_tool_in_external_mcp` to invoke a specific downstream MCP tool."
                .into(),
        );
        info.capabilities = ServerCapabilities::builder().enable_tools().build();
        info
    }

    fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<rmcp::RoleServer>,
    ) -> impl Future<Output = Result<ListToolsResult, McpError>> + Send + '_ {
        std::future::ready(Ok(ListToolsResult {
            tools: self.tools.list(),
            ..Default::default()
        }))
    }

    fn get_tool(&self, name: &str) -> Option<rmcp::model::Tool> {
        self.tools.get(name)
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        _context: RequestContext<rmcp::RoleServer>,
    ) -> Result<CallToolResult, McpError> {
        let arguments = request
            .arguments
            .ok_or_else(|| McpError::invalid_params("tool arguments are required", None))?;

        match request.name.as_ref() {
            ACTIVATE_EXTERNAL_MCP_NAME => self.call_activate_tool(arguments).await,
            ACTIVATE_EXTERNAL_MCP_TOOL_NAME => self.call_activate_tool_detail(arguments).await,
            CALL_TOOL_IN_EXTERNAL_MCP_NAME => self.call_external_tool(arguments).await,
            _ => Err(McpError::method_not_found::<CallToolRequestMethod>()),
        }
    }
}
