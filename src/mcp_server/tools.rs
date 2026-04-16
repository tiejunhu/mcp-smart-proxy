use rmcp::{
    ErrorData as McpError,
    model::{CallToolResult, Content, Tool, ToolAnnotations, object},
};
use serde::{Deserialize, de::DeserializeOwned};
use serde_json::{Map as JsonMap, Value as JsonValue, json};

use crate::paths::sanitize_name;
use crate::types::{CachedToolsetRecord, ToolSnapshot};

use super::lua_eval::EVAL_LUA_SCRIPT_NAME;

pub(super) const ACTIVATE_ADDITIONAL_MCPS_NAME: &str = "activate_additional_mcps";
pub(super) const ACTIVATE_TOOLS_IN_ADDITIONAL_MCP_NAME: &str = "activate_tools_in_additional_mcp";
pub(super) const CALL_TOOL_IN_ADDITIONAL_MCP_NAME: &str = "call_tool_in_additional_mcp";
pub(super) const EVAL_LUA_SCRIPT_DESCRIPTION: &str = "Evaluate a Lua 5.5 script. The script can call any activated MCP tools through the async `call_mcp_tool(mcp_name, tool_name, args)` helper, where `args` must be a Lua table that maps to a JSON object or nil.";
pub(super) const STDIO_HOST_REQUIRED_MESSAGE: &str = "`msp mcp` is a stdio MCP server and must be started by an MCP client such as Codex, OpenCode, or Claude Code instead of running directly in a terminal";

#[derive(Debug, Deserialize)]
pub(super) struct ActivateAdditionalMcpsRequest {
    pub(super) external_mcp_names: Vec<String>,
}

#[derive(Debug, Deserialize)]
pub(super) struct ActivateToolsInAdditionalMcpRequest {
    pub(super) external_mcp_name: String,
    pub(super) tool_names: Vec<String>,
}

#[derive(Debug, Deserialize)]
pub(super) struct CallToolInAdditionalMcpRequest {
    pub(super) external_mcp_name: String,
    pub(super) tool_name: String,
    pub(super) args_in_json: String,
}

#[derive(Clone)]
pub(super) struct ToolCatalog {
    activate_mcps: Tool,
    activate_tools: Tool,
    call_tool_in_additional_mcp: Tool,
    eval_lua_script: Tool,
}

impl ToolCatalog {
    pub(super) fn new(toolsets: &[CachedToolsetRecord]) -> Self {
        Self {
            activate_mcps: activate_additional_mcps_definition(toolsets),
            activate_tools: activate_tools_in_additional_mcp_definition(),
            call_tool_in_additional_mcp: call_tool_in_additional_mcp_definition(
                CALL_TOOL_IN_ADDITIONAL_MCP_NAME,
            ),
            eval_lua_script: eval_lua_script_definition(),
        }
    }

    pub(super) fn list(&self) -> Vec<Tool> {
        vec![
            self.activate_mcps.clone(),
            self.activate_tools.clone(),
            self.call_tool_in_additional_mcp.clone(),
            self.eval_lua_script.clone(),
        ]
    }

    pub(super) fn get(&self, name: &str) -> Option<Tool> {
        match name {
            ACTIVATE_ADDITIONAL_MCPS_NAME => Some(self.activate_mcps.clone()),
            ACTIVATE_TOOLS_IN_ADDITIONAL_MCP_NAME => Some(self.activate_tools.clone()),
            CALL_TOOL_IN_ADDITIONAL_MCP_NAME => Some(self.call_tool_in_additional_mcp.clone()),
            EVAL_LUA_SCRIPT_NAME => Some(self.eval_lua_script.clone()),
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
    .with_annotations(proxy_tool_annotations(false))
}

pub(super) fn eval_lua_script_definition() -> Tool {
    Tool::new(
        EVAL_LUA_SCRIPT_NAME,
        EVAL_LUA_SCRIPT_DESCRIPTION,
        object(json!({
            "type": "object",
            "properties": {
                "script": {
                    "type": "string",
                    "description": "The Lua 5.5 source code to execute. Return a value to produce structured output."
                },
                "globals": {
                    "type": "object",
                    "description": "Optional JSON object whose top-level keys are injected into the Lua global environment before execution."
                }
            },
            "required": ["script"],
            "additionalProperties": false
        })),
    )
    .with_annotations(proxy_tool_annotations(false))
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

pub(super) fn build_activate_tool_result(toolsets: &[&CachedToolsetRecord]) -> CallToolResult {
    let content = toolsets
        .iter()
        .map(|toolset| {
            let mut lines = vec![format!("[{}]", toolset.name)];
            lines.extend(toolset.tools.iter().map(format_activate_tool_line));
            lines.join("\n")
        })
        .collect::<Vec<_>>()
        .join("\n\n");
    CallToolResult::success(vec![Content::text(content)])
}

pub(super) fn build_activate_tool_detail_result(tools: &[&ToolSnapshot]) -> CallToolResult {
    CallToolResult::structured(json!({
        "tools": tools,
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

fn activate_additional_mcps_definition(toolsets: &[CachedToolsetRecord]) -> Tool {
    Tool::new(
        ACTIVATE_ADDITIONAL_MCPS_NAME,
        build_activate_tool_description(toolsets),
        object(json!({
            "type": "object",
            "properties": {
                "external_mcp_names": {
                    "type": "array",
                    "description": "The MCP server names to activate.",
                    "items": {
                        "type": "string"
                    },
                    "minItems": 1
                }
            },
            "required": ["external_mcp_names"],
            "additionalProperties": false
        })),
    )
    .with_annotations(proxy_tool_annotations(true))
}

fn activate_tools_in_additional_mcp_definition() -> Tool {
    Tool::new(
        ACTIVATE_TOOLS_IN_ADDITIONAL_MCP_NAME,
        "Return the full definitions of one or more tools exposed by an additional MCP server, use this tool before calling call_tool_in_additional_mcp",
        object(json!({
            "type": "object",
            "properties": {
                "external_mcp_name": {
                    "type": "string",
                    "description": "The external MCP server name."
                },
                "tool_names": {
                    "type": "array",
                    "description": "The tool names exposed by that external MCP server.",
                    "items": {
                        "type": "string"
                    },
                    "minItems": 1
                }
            },
            "required": ["external_mcp_name", "tool_names"],
            "additionalProperties": false
        })),
    )
    .with_annotations(proxy_tool_annotations(true))
}

fn proxy_tool_annotations(read_only: bool) -> ToolAnnotations {
    ToolAnnotations::new()
        .read_only(read_only)
        .destructive(false)
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
