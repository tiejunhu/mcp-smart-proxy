use rmcp::{
    ErrorData as McpError,
    model::{CallToolResult, Content, Tool, ToolAnnotations, object},
};
use serde::{Deserialize, de::DeserializeOwned};
use serde_json::{Map as JsonMap, Value as JsonValue, json};

use crate::input_popup::popup_input_schema;
use crate::paths::sanitize_name;
use crate::types::{CachedToolsetRecord, ToolSnapshot};

pub(super) const ACTIVATE_ADDITIONAL_MCP_NAME: &str = "activate_additional_mcp";
pub(super) const ACTIVATE_TOOL_IN_ADDITIONAL_MCP_NAME: &str = "activate_tool_in_additional_mcp";
pub(super) const CALL_TOOL_IN_ADDITIONAL_MCP_NAME: &str = "call_tool_in_additional_mcp";
pub(super) const REQUEST_USER_INPUT_IN_POPUP_NAME: &str = "request_user_input_in_popup";
pub(super) const STDIO_HOST_REQUIRED_MESSAGE: &str = "`msp mcp` is a stdio MCP server and must be started by an MCP client such as Codex, OpenCode, or Claude Code instead of running directly in a terminal";

#[derive(Debug, Deserialize)]
pub(super) struct ActivateAdditionalMcpRequest {
    pub(super) external_mcp_name: String,
}

#[derive(Debug, Deserialize)]
pub(super) struct ActivateToolInAdditionalMcpRequest {
    pub(super) external_mcp_name: String,
    pub(super) tool_name: String,
}

#[derive(Debug, Deserialize)]
pub(super) struct CallToolInAdditionalMcpRequest {
    pub(super) external_mcp_name: String,
    pub(super) tool_name: String,
    pub(super) args_in_json: String,
}

#[derive(Clone)]
pub(super) struct ToolCatalog {
    activate_tool: Tool,
    activate_tool_detail: Tool,
    call_tool_in_additional_mcp: Tool,
    request_user_input_in_popup: Option<Tool>,
}

impl ToolCatalog {
    pub(super) fn new(toolsets: &[CachedToolsetRecord], enable_input: bool) -> Self {
        Self {
            activate_tool: activate_tool_definition(toolsets),
            activate_tool_detail: activate_tool_in_additional_mcp_definition(),
            call_tool_in_additional_mcp: call_tool_in_additional_mcp_definition(
                CALL_TOOL_IN_ADDITIONAL_MCP_NAME,
            ),
            request_user_input_in_popup: enable_input.then(request_user_input_in_popup_definition),
        }
    }

    pub(super) fn list(&self) -> Vec<Tool> {
        let mut tools = vec![
            self.activate_tool.clone(),
            self.activate_tool_detail.clone(),
            self.call_tool_in_additional_mcp.clone(),
        ];
        if let Some(tool) = &self.request_user_input_in_popup {
            tools.push(tool.clone());
        }
        tools
    }

    pub(super) fn get(&self, name: &str) -> Option<Tool> {
        match name {
            ACTIVATE_ADDITIONAL_MCP_NAME => Some(self.activate_tool.clone()),
            ACTIVATE_TOOL_IN_ADDITIONAL_MCP_NAME => Some(self.activate_tool_detail.clone()),
            CALL_TOOL_IN_ADDITIONAL_MCP_NAME => Some(self.call_tool_in_additional_mcp.clone()),
            REQUEST_USER_INPUT_IN_POPUP_NAME => self.request_user_input_in_popup.clone(),
            _ => None,
        }
    }
}

pub(super) fn build_activate_tool_description(toolsets: &[CachedToolsetRecord]) -> String {
    let mut lines = vec![
        "Use this tool to activate additional MCP servers. The following additional MCP servers are available to be activated when you need some tools to complete the user request:"
            .to_string(),
        String::new(),
    ];
    if toolsets.is_empty() {
        lines.push(
            "- none: no cached external MCP servers available yet; run `mcp-smart-proxy reload <name>` first."
                .to_string(),
        );
        return lines.join("\n");
    }

    for toolset in toolsets {
        lines.push(format!("- {}: {}", toolset.name, toolset.summary));
    }

    lines.join("\n")
}

pub(super) fn call_tool_in_additional_mcp_definition(name: &'static str) -> Tool {
    Tool::new(
        name,
        "Call a specific tool exposed by an additional MCP server",
        object(json!({
            "type": "object",
            "properties": {
                "external_mcp_name": {
                    "type": "string",
                    "description": "The external MCP server name."
                },
                "tool_name": {
                    "type": "string",
                    "description": "The tool name exposed by that external MCP server."
                },
                "args_in_json": {
                    "type": "string",
                    "description": "A JSON object string containing the external MCP tool arguments, follow the JSON schema, for example {}."
                }
            },
            "required": ["external_mcp_name", "tool_name", "args_in_json"],
            "additionalProperties": false
        })),
    )
}

pub(super) fn request_user_input_in_popup_definition() -> Tool {
    Tool::new(
        REQUEST_USER_INPUT_IN_POPUP_NAME,
        "Request user input through a popup. When you need to ask the user for input on some question and don't have other tools, use this one.",
        object(popup_input_schema()),
    )
}

pub(super) fn resolve_toolset_name<'a>(
    toolsets: &'a [CachedToolsetRecord],
    requested_name: &str,
) -> Option<&'a CachedToolsetRecord> {
    if let Some(toolset) = toolsets
        .iter()
        .find(|toolset| toolset.name == requested_name)
    {
        return Some(toolset);
    }

    let sanitized = sanitize_name(requested_name);
    toolsets.iter().find(|toolset| toolset.name == sanitized)
}

pub(super) fn parse_tool_request<T: DeserializeOwned>(
    tool_name: &str,
    arguments: JsonMap<String, JsonValue>,
) -> Result<T, McpError> {
    serde_json::from_value(JsonValue::Object(arguments)).map_err(|error| {
        McpError::invalid_params(format!("invalid {tool_name} arguments: {error}"), None)
    })
}

pub(super) fn build_activate_tool_result(toolset: &CachedToolsetRecord) -> CallToolResult {
    let content = toolset
        .tools
        .iter()
        .map(format_activate_tool_line)
        .collect::<Vec<_>>()
        .join("\n");
    CallToolResult::success(vec![Content::text(content)])
}

pub(super) fn build_activate_tool_detail_result(tool: &ToolSnapshot) -> CallToolResult {
    CallToolResult::structured(json!({
        "tool": tool,
    }))
}

pub(super) fn parse_tool_arguments_json(
    args_in_json: &str,
) -> Result<Option<JsonMap<String, JsonValue>>, McpError> {
    let parsed = serde_json::from_str::<JsonValue>(args_in_json).map_err(|error| {
        McpError::invalid_params(
            format!("`args_in_json` must be valid JSON: {error}"),
            Some(json!({ "args_in_json": args_in_json })),
        )
    })?;

    match parsed {
        JsonValue::Null => Ok(None),
        JsonValue::Object(map) => Ok(Some(map)),
        _ => Err(McpError::invalid_params(
            "`args_in_json` must decode to a JSON object or null",
            Some(json!({ "args_in_json": args_in_json })),
        )),
    }
}

pub(super) fn resolve_toolset_or_error<'a>(
    toolsets: &'a [CachedToolsetRecord],
    requested_name: &str,
) -> Result<&'a CachedToolsetRecord, McpError> {
    resolve_toolset_name(toolsets, requested_name).ok_or_else(|| {
        McpError::invalid_params(
            format!("unknown external MCP server `{requested_name}`"),
            Some(json!({
                "available_external_mcps": available_toolset_names(toolsets)
            })),
        )
    })
}

pub(super) fn resolve_tool_snapshot_or_error<'a>(
    toolset: &'a CachedToolsetRecord,
    tool_name: &str,
) -> Result<&'a ToolSnapshot, McpError> {
    resolve_tool_snapshot(toolset, tool_name).ok_or_else(|| {
        McpError::invalid_params(
            format!(
                "unknown tool `{tool_name}` in external MCP server `{}`",
                toolset.name
            ),
            Some(json!({
                "available_tools": available_tool_names(toolset)
            })),
        )
    })
}

fn activate_tool_definition(toolsets: &[CachedToolsetRecord]) -> Tool {
    Tool::new(
        ACTIVATE_ADDITIONAL_MCP_NAME,
        build_activate_tool_description(toolsets),
        object(json!({
            "type": "object",
            "properties": {
                "external_mcp_name": {
                    "type": "string",
                    "description": "The MCP server name to activate."
                }
            },
            "required": ["external_mcp_name"],
            "additionalProperties": false
        })),
    )
    .with_annotations(read_only_annotations())
}

fn activate_tool_in_additional_mcp_definition() -> Tool {
    Tool::new(
        ACTIVATE_TOOL_IN_ADDITIONAL_MCP_NAME,
        "Return the full definition of one tool exposed by an additional MCP server, use this tool before calling call_tool_in_additional_mcp",
        object(json!({
            "type": "object",
            "properties": {
                "external_mcp_name": {
                    "type": "string",
                    "description": "The external MCP server name."
                },
                "tool_name": {
                    "type": "string",
                    "description": "The tool name exposed by that external MCP server."
                }
            },
            "required": ["external_mcp_name", "tool_name"],
            "additionalProperties": false
        })),
    )
    .with_annotations(read_only_annotations())
}

fn read_only_annotations() -> ToolAnnotations {
    ToolAnnotations::new().read_only(true)
}

fn resolve_tool_snapshot<'a>(
    toolset: &'a CachedToolsetRecord,
    tool_name: &str,
) -> Option<&'a ToolSnapshot> {
    toolset.tools.iter().find(|tool| tool.name == tool_name)
}

fn available_toolset_names(toolsets: &[CachedToolsetRecord]) -> Vec<String> {
    toolsets
        .iter()
        .map(|toolset| toolset.name.clone())
        .collect()
}

fn available_tool_names(toolset: &CachedToolsetRecord) -> Vec<String> {
    toolset.tools.iter().map(|tool| tool.name.clone()).collect()
}

fn tool_description_preview(tool: &ToolSnapshot) -> String {
    const MAX_DESCRIPTION_CHARS: usize = 80;
    const ELLIPSIS: &str = "...";

    let description = tool.description.as_deref().unwrap_or_default();
    let char_count = description.chars().count();
    if char_count <= MAX_DESCRIPTION_CHARS {
        return description.to_string();
    }

    let preview_len = MAX_DESCRIPTION_CHARS.saturating_sub(ELLIPSIS.chars().count());
    let mut preview = description.chars().take(preview_len).collect::<String>();
    preview.push_str(ELLIPSIS);
    preview
}

fn format_activate_tool_line(tool: &ToolSnapshot) -> String {
    let description = tool_description_preview(tool);
    if description.is_empty() {
        tool.name.clone()
    } else {
        format!("{}: {}", tool.name, description)
    }
}
