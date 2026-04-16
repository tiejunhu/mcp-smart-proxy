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
use crate::toon::rewrite_call_tool_result_to_toon;
use crate::types::CachedToolsetRecord;

use super::lua_eval::{EVAL_LUA_SCRIPT_NAME, EvalLuaScriptRequest, execute_eval_lua_script};
use super::tools::{
    ACTIVATE_ADDITIONAL_MCPS_NAME, ACTIVATE_TOOLS_IN_ADDITIONAL_MCP_NAME,
    ActivateAdditionalMcpsRequest, ActivateToolsInAdditionalMcpRequest,
    CALL_TOOL_IN_ADDITIONAL_MCP_NAME, CallToolInAdditionalMcpRequest, ToolCatalog,
    build_activate_tool_detail_result, build_activate_tool_result, parse_tool_arguments_json,
    parse_tool_request, resolve_tool_snapshot_or_error, resolve_toolset_or_error,
};

pub(super) struct SmartProxyMcpServer {
    config_path: PathBuf,
    output_toon: bool,
    tools: ToolCatalog,
    toolsets: Vec<CachedToolsetRecord>,
}

impl SmartProxyMcpServer {
    pub(super) fn new(
        config_path: PathBuf,
        toolsets: Vec<CachedToolsetRecord>,
        output_toon: bool,
    ) -> Self {
        let tools = ToolCatalog::new(&toolsets);
        Self {
            config_path,
            output_toon,
            tools,
            toolsets,
        }
    }

    async fn call_activate_tool(
        &self,
        arguments: JsonMap<String, JsonValue>,
    ) -> Result<CallToolResult, McpError> {
        let params: ActivateAdditionalMcpsRequest =
            parse_tool_request(ACTIVATE_ADDITIONAL_MCPS_NAME, arguments)?;
        let toolsets = params
            .external_mcp_names
            .iter()
            .map(|name| resolve_toolset_or_error(&self.toolsets, name))
            .collect::<Result<Vec<_>, _>>()?;
        Ok(build_activate_tool_result(&toolsets))
    }

    async fn call_activate_tool_detail(
        &self,
        arguments: JsonMap<String, JsonValue>,
    ) -> Result<CallToolResult, McpError> {
        let params: ActivateToolsInAdditionalMcpRequest =
            parse_tool_request(ACTIVATE_TOOLS_IN_ADDITIONAL_MCP_NAME, arguments)?;
        let toolset = resolve_toolset_or_error(&self.toolsets, &params.external_mcp_name)?;
        let tools = params
            .tool_names
            .iter()
            .map(|tool_name| resolve_tool_snapshot_or_error(toolset, tool_name))
            .collect::<Result<Vec<_>, _>>()?;
        Ok(build_activate_tool_detail_result(&tools))
    }

    async fn call_external_tool(
        &self,
        arguments: JsonMap<String, JsonValue>,
    ) -> Result<CallToolResult, McpError> {
        let params: CallToolInAdditionalMcpRequest =
            parse_tool_request(CALL_TOOL_IN_ADDITIONAL_MCP_NAME, arguments)?;
        let toolset = resolve_toolset_or_error(&self.toolsets, &params.external_mcp_name)?;
        let arguments = parse_tool_arguments_json(&params.args_in_json)?;

        self.call_downstream_tool(toolset, params.tool_name, arguments)
            .await
    }

    async fn call_eval_lua_script(
        &self,
        arguments: JsonMap<String, JsonValue>,
    ) -> Result<CallToolResult, McpError> {
        let params: EvalLuaScriptRequest = parse_tool_request(EVAL_LUA_SCRIPT_NAME, arguments)?;
        Ok(execute_eval_lua_script(&self.config_path, params).await)
    }

    async fn call_downstream_tool(
        &self,
        toolset: &CachedToolsetRecord,
        tool_name: String,
        arguments: Option<JsonMap<String, JsonValue>>,
    ) -> Result<CallToolResult, McpError> {
        let result = daemon::call_tool(
            &self.config_path,
            None,
            &toolset.name,
            &tool_name,
            arguments,
        )
        .await
        .map_err(|error| McpError::internal_error(error.to_string(), None))?;

        if self.output_toon {
            return rewrite_call_tool_result_to_toon(&result)
                .map_err(|error| McpError::internal_error(error.to_string(), None));
        }

        Ok(result)
    }
}

impl ServerHandler for SmartProxyMcpServer {
    fn get_info(&self) -> ServerInfo {
        let mut info = ServerInfo::default();
        let instructions = "Use `activate_additional_mcps` to inspect cached tool names, `activate_tools_in_additional_mcp` to inspect one or more full cached tool definitions, and `call_tool_in_additional_mcp` to invoke a specific downstream MCP tool.";
        info.instructions = Some(instructions.into());
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
            ACTIVATE_ADDITIONAL_MCPS_NAME => self.call_activate_tool(arguments).await,
            ACTIVATE_TOOLS_IN_ADDITIONAL_MCP_NAME => {
                self.call_activate_tool_detail(arguments).await
            }
            CALL_TOOL_IN_ADDITIONAL_MCP_NAME => self.call_external_tool(arguments).await,
            EVAL_LUA_SCRIPT_NAME => self.call_eval_lua_script(arguments).await,
            _ => Err(McpError::method_not_found::<CallToolRequestMethod>()),
        }
    }
}
