use std::collections::HashMap;
use std::error::Error;
use std::fs;
use std::future::Future;
use std::io::IsTerminal;
use std::path::Path;
use std::sync::Arc;

use rmcp::{
    ErrorData as McpError, RoleClient, ServerHandler, ServiceExt,
    model::{
        CallToolRequestMethod, CallToolRequestParams, CallToolResult, Content, ListToolsResult,
        PaginatedRequestParams, ServerCapabilities, ServerInfo, Tool, object,
    },
    service::{RequestContext, RunningService, ServiceError},
    transport::stdio,
};
use serde::{Deserialize, de::DeserializeOwned};
use serde_json::{Map as JsonMap, Value as JsonValue, json};
use tokio::sync::Mutex;
use toml::{Table, Value};

use crate::config::{configured_server, list_servers, load_config_table, server_is_enabled};
use crate::console::{
    ExternalOutputRouter, operation_error, print_external_command_failure_with_captured_stderr,
};
use crate::downstream_client::connect_stdio_client;
use crate::paths::{cache_file_path_from_home, format_path_for_display, home_dir, sanitize_name};
use crate::reload::{reload_server, reload_server_with_provider};
use crate::remote::connect_remote_client;
use crate::types::{
    CachedTools, ConfiguredServer, ConfiguredTransport, ModelProviderConfig, ToolSnapshot,
};

const ACTIVATE_EXTERNAL_MCP_NAME: &str = "activate_external_mcp";
const ACTIVATE_EXTERNAL_MCP_TOOL_NAME: &str = "activate_external_mcp_tool";
const CALL_TOOL_IN_EXTERNAL_MCP_NAME: &str = "call_tool_in_external_mcp";
const STDIO_HOST_REQUIRED_MESSAGE: &str = "`msp mcp` is a stdio MCP server and must be started by an MCP client such as Codex, OpenCode, or Claude Code instead of running directly in a terminal";

#[derive(Debug, Clone)]
struct CachedToolsetRecord {
    name: String,
    summary: String,
    server: ConfiguredServer,
    tools: Vec<ToolSnapshot>,
}

#[derive(Clone)]
struct ToolsetClient {
    service: Arc<RunningService<RoleClient, ()>>,
    stderr: ExternalOutputRouter,
    command_line: String,
    label: String,
}

#[derive(Debug, Deserialize)]
struct ActivateExternalMcpRequest {
    external_mcp_name: String,
}

#[derive(Debug, Deserialize)]
struct ActivateExternalMcpToolRequest {
    external_mcp_name: String,
    tool_name: String,
}

#[derive(Debug, Deserialize)]
struct CallToolInExternalMcpRequest {
    external_mcp_name: String,
    tool_name: String,
    args_in_json: String,
}

pub async fn serve_cached_toolsets(
    config_path: &Path,
    provider: Option<ModelProviderConfig>,
) -> Result<(), Box<dyn Error>> {
    ensure_proxy_stdio_host_connection()?;

    reload_all_toolsets(config_path, provider.as_ref())
        .await
        .map_err(|error| {
            operation_error(
                "mcp.reload_all_toolsets",
                format!(
                    "failed to reload configured MCP servers before starting proxy with config {}",
                    format_path_for_display(config_path)
                ),
                error,
            )
        })?;

    let config = load_config_table(config_path).map_err(|error| {
        operation_error(
            "mcp.load_config",
            format!(
                "failed to load config from {}",
                format_path_for_display(config_path)
            ),
            error,
        )
    })?;
    let toolsets = load_cached_toolsets(&config).map_err(|error| {
        operation_error(
            "mcp.load_toolsets",
            "failed to load cached toolsets from config",
            error,
        )
    })?;
    let service = SmartProxyMcpServer::new(toolsets)
        .serve(stdio())
        .await
        .map_err(map_proxy_serve_error)?;
    service.waiting().await.map_err(|error| {
        operation_error(
            "mcp.wait",
            "proxy stdio MCP server exited with an error",
            Box::new(error),
        )
    })?;
    Ok(())
}

fn ensure_proxy_stdio_host_connection() -> Result<(), Box<dyn Error>> {
    validate_proxy_stdio_launch(
        std::io::stdin().is_terminal(),
        std::io::stdout().is_terminal(),
    )
}

fn validate_proxy_stdio_launch(
    stdin_is_terminal: bool,
    stdout_is_terminal: bool,
) -> Result<(), Box<dyn Error>> {
    if stdin_is_terminal || stdout_is_terminal {
        return Err(operation_error(
            "mcp.serve.stdio_host",
            STDIO_HOST_REQUIRED_MESSAGE,
            "stdio MCP servers require an upstream host connection over stdin/stdout".into(),
        ));
    }

    Ok(())
}

fn map_proxy_serve_error(error: impl Error + 'static) -> Box<dyn Error> {
    if error.to_string() == "connection closed: initialize request" {
        return operation_error(
            "mcp.serve.initialize",
            STDIO_HOST_REQUIRED_MESSAGE,
            Box::new(error),
        );
    }

    operation_error(
        "mcp.serve",
        "failed to start the proxy stdio MCP server",
        Box::new(error),
    )
}

async fn reload_all_toolsets(
    config_path: &Path,
    provider: Option<&ModelProviderConfig>,
) -> Result<(), Box<dyn Error>> {
    let servers = list_servers(config_path).map_err(|error| {
        operation_error(
            "mcp.reload_all_toolsets.list_servers",
            format!(
                "failed to list configured MCP servers from {} before startup reload",
                format_path_for_display(config_path)
            ),
            error,
        )
    })?;

    for server in servers.into_iter().filter(|server| server.enabled) {
        let server_name = server.name;
        match provider {
            Some(provider) => {
                reload_server_with_provider(config_path, &server_name, provider).await
            }
            None => reload_server(config_path, &server_name).await,
        }
        .map_err(|error| {
            operation_error(
                "mcp.reload_all_toolsets.reload_server",
                format!("failed to reload MCP server `{server_name}` before proxy startup"),
                error,
            )
        })?;
    }

    Ok(())
}

fn load_cached_toolsets(config: &Table) -> Result<Vec<CachedToolsetRecord>, Box<dyn Error>> {
    load_cached_toolsets_from_home(config, &home_dir()?)
}

fn load_cached_toolsets_from_home(
    config: &Table,
    home: &Path,
) -> Result<Vec<CachedToolsetRecord>, Box<dyn Error>> {
    let Some(servers) = config.get("servers").and_then(Value::as_table) else {
        return Ok(Vec::new());
    };

    let mut names = servers.keys().cloned().collect::<Vec<_>>();
    names.sort();

    let mut toolsets = Vec::new();
    for name in names {
        if !server_is_enabled(config, &name)? {
            continue;
        }
        let (_, server) = configured_server(config, &name)?;
        let cache_path = cache_file_path_from_home(home, &name)?;
        if !cache_path.exists() {
            continue;
        }

        let cached: CachedTools = serde_json::from_str(&fs::read_to_string(cache_path)?)?;
        toolsets.push(CachedToolsetRecord {
            name,
            summary: cached.summary,
            server,
            tools: cached.tools,
        });
    }

    Ok(toolsets)
}

fn build_activate_tool_description(toolsets: &[CachedToolsetRecord]) -> String {
    let mut lines = vec!["available external MCP servers:".to_string(), String::new()];
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

fn activate_tool_definition(toolsets: &[CachedToolsetRecord]) -> Tool {
    Tool::new(
        ACTIVATE_EXTERNAL_MCP_NAME,
        build_activate_tool_description(toolsets),
        object(json!({
            "type": "object",
            "properties": {
                "external_mcp_name": {
                    "type": "string",
                    "description": "The external MCP server name to activate."
                }
            },
            "required": ["external_mcp_name"],
            "additionalProperties": false
        })),
    )
}

fn call_tool_in_external_mcp_definition(name: &'static str) -> Tool {
    Tool::new(
        name,
        "Call a specific tool exposed by an external MCP server",
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

fn activate_external_mcp_tool_definition() -> Tool {
    Tool::new(
        ACTIVATE_EXTERNAL_MCP_TOOL_NAME,
        "Return the full definition of one tool exposed by an external MCP server, use this tool before calling call_tool_in_external_mcp",
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
}

fn resolve_toolset_name<'a>(
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

fn resolve_toolset_or_error<'a>(
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

fn resolve_tool_snapshot_or_error<'a>(
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

fn parse_tool_request<T: DeserializeOwned>(
    tool_name: &str,
    arguments: JsonMap<String, JsonValue>,
) -> Result<T, McpError> {
    serde_json::from_value(JsonValue::Object(arguments)).map_err(|error| {
        McpError::invalid_params(format!("invalid {tool_name} arguments: {error}"), None)
    })
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

fn build_activate_tool_result(toolset: &CachedToolsetRecord) -> CallToolResult {
    let content = toolset
        .tools
        .iter()
        .map(format_activate_tool_line)
        .collect::<Vec<_>>()
        .join("\n");
    CallToolResult::success(vec![Content::text(content)])
}

fn build_activate_tool_detail_result(tool: &ToolSnapshot) -> CallToolResult {
    CallToolResult::structured(json!({
        "tool": tool,
    }))
}

fn parse_tool_arguments_json(
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

fn map_service_error(error: ServiceError) -> McpError {
    match error {
        ServiceError::McpError(error) => error,
        other => McpError::internal_error(other.to_string(), None),
    }
}

async fn connect_toolset_client(
    server_name: &str,
    server: &ConfiguredServer,
) -> Result<ToolsetClient, McpError> {
    match &server.transport {
        ConfiguredTransport::Stdio { command, args } => {
            let label = command.clone();
            let client = connect_stdio_client(
                "mcp.connect_toolset_client",
                "mcp.connect_toolset_client.spawn",
                "mcp.connect_toolset_client.connect",
                label.clone(),
                command,
                args,
                server.resolved_env(),
            )
            .await
            .map_err(|error| McpError::internal_error(error.to_string(), None))?;

            Ok(ToolsetClient {
                service: Arc::new(client.service),
                stderr: client.stderr,
                command_line: client.command_line,
                label,
            })
        }
        ConfiguredTransport::Remote { url, .. } => {
            let label = format!("remote:{url}");
            let client = connect_remote_client(server_name, server)
                .await
                .map_err(|error| McpError::internal_error(error.to_string(), None))?;
            Ok(ToolsetClient {
                service: Arc::new(client),
                stderr: ExternalOutputRouter::new(),
                command_line: url.clone(),
                label,
            })
        }
    }
}

struct SmartProxyMcpServer {
    activate_tool: Tool,
    activate_tool_detail: Tool,
    call_tool_in_external_mcp: Tool,
    toolsets: Vec<CachedToolsetRecord>,
    client_slots: HashMap<String, Arc<Mutex<Option<ToolsetClient>>>>,
}

impl SmartProxyMcpServer {
    fn new(toolsets: Vec<CachedToolsetRecord>) -> Self {
        let activate_tool = activate_tool_definition(&toolsets);
        let activate_tool_detail = activate_external_mcp_tool_definition();
        let call_tool_in_external_mcp =
            call_tool_in_external_mcp_definition(CALL_TOOL_IN_EXTERNAL_MCP_NAME);
        let client_slots = toolsets
            .iter()
            .map(|toolset| (toolset.name.clone(), Arc::new(Mutex::new(None))))
            .collect();
        Self {
            activate_tool,
            activate_tool_detail,
            call_tool_in_external_mcp,
            toolsets,
            client_slots,
        }
    }

    async fn get_or_connect_client(
        &self,
        toolset: &CachedToolsetRecord,
    ) -> Result<ToolsetClient, McpError> {
        let slot = self
            .client_slots
            .get(&toolset.name)
            .cloned()
            .ok_or_else(|| McpError::internal_error("missing client slot for toolset", None))?;
        let mut client_guard = slot.lock().await;

        if let Some(client) = client_guard.as_ref() {
            if !client.service.is_closed() {
                return Ok(client.clone());
            }
            *client_guard = None;
        }

        let client = connect_toolset_client(&toolset.name, &toolset.server).await?;
        *client_guard = Some(client.clone());
        Ok(client)
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
            tools: vec![
                self.activate_tool.clone(),
                self.activate_tool_detail.clone(),
                self.call_tool_in_external_mcp.clone(),
            ],
            ..Default::default()
        }))
    }

    fn get_tool(&self, name: &str) -> Option<Tool> {
        match name {
            ACTIVATE_EXTERNAL_MCP_NAME => Some(self.activate_tool.clone()),
            ACTIVATE_EXTERNAL_MCP_TOOL_NAME => Some(self.activate_tool_detail.clone()),
            CALL_TOOL_IN_EXTERNAL_MCP_NAME => Some(self.call_tool_in_external_mcp.clone()),
            _ => None,
        }
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
            ACTIVATE_EXTERNAL_MCP_NAME => {
                let params: ActivateExternalMcpRequest =
                    parse_tool_request(ACTIVATE_EXTERNAL_MCP_NAME, arguments)?;
                let toolset = resolve_toolset_or_error(&self.toolsets, &params.external_mcp_name)?;

                Ok(build_activate_tool_result(toolset))
            }
            ACTIVATE_EXTERNAL_MCP_TOOL_NAME => {
                let params: ActivateExternalMcpToolRequest =
                    parse_tool_request(ACTIVATE_EXTERNAL_MCP_TOOL_NAME, arguments)?;
                let toolset = resolve_toolset_or_error(&self.toolsets, &params.external_mcp_name)?;
                let tool = resolve_tool_snapshot_or_error(toolset, &params.tool_name)?;

                Ok(build_activate_tool_detail_result(tool))
            }
            CALL_TOOL_IN_EXTERNAL_MCP_NAME => {
                let params: CallToolInExternalMcpRequest =
                    parse_tool_request(CALL_TOOL_IN_EXTERNAL_MCP_NAME, arguments)?;
                let toolset = resolve_toolset_or_error(&self.toolsets, &params.external_mcp_name)?;

                let arguments = parse_tool_arguments_json(&params.args_in_json)?;
                let client = self.get_or_connect_client(toolset).await?;
                let stderr_capture = client.stderr.start_capture().await;
                let request = match arguments {
                    Some(arguments) => CallToolRequestParams::new(params.tool_name.clone())
                        .with_arguments(arguments),
                    None => CallToolRequestParams::new(params.tool_name.clone()),
                };
                match client.service.call_tool(request).await {
                    Ok(result) => {
                        let _ = stderr_capture.finish().await;
                        Ok(result)
                    }
                    Err(error) => {
                        let stderr_content = stderr_capture.finish().await;
                        print_external_command_failure_with_captured_stderr(
                            "mcp.call_tool_in_external_mcp",
                            &client.label,
                            &client.command_line,
                            "tool-call-failed",
                            &stderr_content,
                        )
                        .await;
                        Err(map_service_error(error))
                    }
                }
            }
            _ => Err(McpError::method_not_found::<CallToolRequestMethod>()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::env;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn builds_tool_description_from_cached_summaries() {
        let toolsets = vec![
            CachedToolsetRecord {
                name: "alpha".to_string(),
                summary: "Use this when you need Alpha workflows.".to_string(),
                server: ConfiguredServer {
                    transport: ConfiguredTransport::Stdio {
                        command: "uvx".to_string(),
                        args: vec!["alpha".to_string()],
                    },
                    ..Default::default()
                },
                tools: vec![],
            },
            CachedToolsetRecord {
                name: "beta".to_string(),
                summary: "Use this for Beta tasks.".to_string(),
                server: ConfiguredServer {
                    transport: ConfiguredTransport::Stdio {
                        command: "uvx".to_string(),
                        args: vec!["beta".to_string()],
                    },
                    ..Default::default()
                },
                tools: vec![],
            },
        ];

        assert_eq!(
            build_activate_tool_description(&toolsets),
            "available external MCP servers:\n\n- alpha: Use this when you need Alpha workflows.\n- beta: Use this for Beta tasks."
        );
    }

    #[test]
    fn loads_only_toolsets_with_cache_files() {
        let home = unique_test_home("load-cached-toolsets");
        let config: Table = toml::from_str(
            r#"
                [servers.alpha]
                transport = "stdio"
                command = "uvx"
                args = ["alpha-server"]

                [servers.beta]
                transport = "stdio"
                command = "uvx"
                args = ["beta-server"]
            "#,
        )
        .unwrap();

        let alpha_cache = cache_file_path_from_home(&home, "alpha").unwrap();
        fs::create_dir_all(alpha_cache.parent().unwrap()).unwrap();
        fs::write(
            &alpha_cache,
            serde_json::to_string(&CachedTools {
                server: "alpha".to_string(),
                summary: "Use Alpha.".to_string(),
                fetched_at_epoch_ms: 42,
                tools: vec![],
            })
            .unwrap(),
        )
        .unwrap();

        let toolsets = load_cached_toolsets_from_home(&config, &home).unwrap();

        assert_eq!(toolsets.len(), 1);
        assert_eq!(toolsets[0].name, "alpha");
        assert_eq!(toolsets[0].summary, "Use Alpha.");
        assert_eq!(
            toolsets[0].server.stdio_transport(),
            Some(("uvx", ["alpha-server".to_string()].as_slice()))
        );
    }

    #[test]
    fn skips_disabled_toolsets() {
        let home = unique_test_home("load-disabled-toolsets");
        let config: Table = toml::from_str(
            r#"
                [servers.alpha]
                transport = "stdio"
                command = "uvx"
                args = ["alpha-server"]
                enabled = false

                [servers.beta]
                transport = "stdio"
                command = "uvx"
                args = ["beta-server"]
            "#,
        )
        .unwrap();

        let alpha_cache = cache_file_path_from_home(&home, "alpha").unwrap();
        let beta_cache = cache_file_path_from_home(&home, "beta").unwrap();
        fs::create_dir_all(alpha_cache.parent().unwrap()).unwrap();
        fs::write(
            &alpha_cache,
            serde_json::to_string(&CachedTools {
                server: "alpha".to_string(),
                summary: "Use Alpha.".to_string(),
                fetched_at_epoch_ms: 42,
                tools: vec![],
            })
            .unwrap(),
        )
        .unwrap();
        fs::write(
            &beta_cache,
            serde_json::to_string(&CachedTools {
                server: "beta".to_string(),
                summary: "Use Beta.".to_string(),
                fetched_at_epoch_ms: 43,
                tools: vec![],
            })
            .unwrap(),
        )
        .unwrap();

        let toolsets = load_cached_toolsets_from_home(&config, &home).unwrap();

        assert_eq!(toolsets.len(), 1);
        assert_eq!(toolsets[0].name, "beta");
    }

    #[test]
    fn resolves_toolset_by_sanitized_name() {
        let toolsets = vec![CachedToolsetRecord {
            name: "team-alpha".to_string(),
            summary: "Use Alpha.".to_string(),
            server: ConfiguredServer {
                transport: ConfiguredTransport::Stdio {
                    command: "uvx".to_string(),
                    args: vec!["alpha".to_string()],
                },
                ..Default::default()
            },
            tools: vec![],
        }];

        let found = resolve_toolset_name(&toolsets, "Team Alpha").unwrap();
        assert_eq!(found.name, "team-alpha");
    }

    #[test]
    fn activate_tool_returns_only_tools() {
        let toolset = CachedToolsetRecord {
            name: "alpha".to_string(),
            summary: "Use Alpha.".to_string(),
            server: ConfiguredServer {
                transport: ConfiguredTransport::Stdio {
                    command: "uvx".to_string(),
                    args: vec!["alpha".to_string()],
                },
                ..Default::default()
            },
            tools: vec![ToolSnapshot {
                name: "search".to_string(),
                title: Some("Search".to_string()),
                description: Some("Search things".to_string()),
                input_schema: json!({
                    "type": "object"
                }),
                output_schema: None,
                annotations: None,
                execution: None,
                icons: None,
                meta: None,
            }],
        };

        let result = build_activate_tool_result(&toolset);

        assert_eq!(result.structured_content, None);
        assert_eq!(result.content.len(), 1);
        assert_eq!(
            result.content[0].as_text().unwrap().text,
            "search: Search things"
        );
    }

    #[test]
    fn activate_tool_truncates_tool_description_to_80_characters_with_ellipsis() {
        let toolset = CachedToolsetRecord {
            name: "alpha".to_string(),
            summary: "Use Alpha.".to_string(),
            server: ConfiguredServer {
                transport: ConfiguredTransport::Stdio {
                    command: "uvx".to_string(),
                    args: vec!["alpha".to_string()],
                },
                ..Default::default()
            },
            tools: vec![ToolSnapshot {
                name: "search".to_string(),
                title: Some("Search".to_string()),
                description: Some(
                    "12345678901234567890123456789012345678901234567890123456789012345678901234567890EXTRA"
                        .to_string(),
                ),
                input_schema: json!({
                    "type": "object"
                }),
                output_schema: None,
                annotations: None,
                execution: None,
                icons: None,
                meta: None,
            }],
        };

        let result = build_activate_tool_result(&toolset);

        assert_eq!(result.structured_content, None);
        assert_eq!(result.content.len(), 1);
        assert_eq!(
            result.content[0].as_text().unwrap().text,
            "search: 12345678901234567890123456789012345678901234567890123456789012345678901234567..."
        );
    }

    #[test]
    fn activate_tool_returns_name_only_when_description_is_missing() {
        let toolset = CachedToolsetRecord {
            name: "alpha".to_string(),
            summary: "Use Alpha.".to_string(),
            server: ConfiguredServer {
                transport: ConfiguredTransport::Stdio {
                    command: "uvx".to_string(),
                    args: vec!["alpha".to_string()],
                },
                ..Default::default()
            },
            tools: vec![ToolSnapshot {
                name: "search".to_string(),
                title: Some("Search".to_string()),
                description: None,
                input_schema: json!({
                    "type": "object"
                }),
                output_schema: None,
                annotations: None,
                execution: None,
                icons: None,
                meta: None,
            }],
        };

        let result = build_activate_tool_result(&toolset);

        assert_eq!(result.structured_content, None);
        assert_eq!(result.content.len(), 1);
        assert_eq!(result.content[0].as_text().unwrap().text, "search");
    }

    #[test]
    fn activate_tool_detail_returns_full_tool_definition() {
        let tool = ToolSnapshot {
            name: "search".to_string(),
            title: Some("Search".to_string()),
            description: Some("Search things".to_string()),
            input_schema: json!({
                "type": "object"
            }),
            output_schema: None,
            annotations: None,
            execution: None,
            icons: None,
            meta: None,
        };

        let result = build_activate_tool_detail_result(&tool);

        assert_eq!(
            result.structured_content,
            Some(json!({
                "tool": {
                    "name": "search",
                    "title": "Search",
                    "description": "Search things",
                    "input_schema": {
                        "type": "object"
                    },
                    "output_schema": null,
                    "annotations": null,
                    "execution": null,
                    "icons": null,
                    "meta": null
                }
            }))
        );
    }

    #[test]
    fn parses_object_arguments_json() {
        let parsed = parse_tool_arguments_json(r#"{"query":"hello"}"#).unwrap();

        assert_eq!(
            parsed,
            Some(json!({ "query": "hello" }).as_object().unwrap().clone())
        );
    }

    #[test]
    fn parses_null_arguments_json() {
        let parsed = parse_tool_arguments_json("null").unwrap();

        assert_eq!(parsed, None);
    }

    #[test]
    fn rejects_non_object_arguments_json() {
        let error = parse_tool_arguments_json(r#"["hello"]"#).unwrap_err();

        assert_eq!(
            error.message,
            "`args_in_json` must decode to a JSON object or null"
        );
    }

    #[test]
    fn call_tool_definition_contains_expected_fields() {
        let tool = call_tool_in_external_mcp_definition(CALL_TOOL_IN_EXTERNAL_MCP_NAME);
        let properties = tool
            .input_schema
            .get("properties")
            .and_then(JsonValue::as_object)
            .unwrap();

        assert!(properties.contains_key("external_mcp_name"));
        assert!(properties.contains_key("tool_name"));
        assert!(properties.contains_key("args_in_json"));
    }

    #[test]
    fn rejects_running_proxy_stdio_server_directly_in_terminal() {
        let error = validate_proxy_stdio_launch(true, true).unwrap_err();

        assert_eq!(
            error.to_string(),
            format!("mcp.serve.stdio_host: {STDIO_HOST_REQUIRED_MESSAGE}")
        );
    }

    #[test]
    fn allows_running_proxy_stdio_server_when_connected_to_a_host() {
        validate_proxy_stdio_launch(false, false).unwrap();
    }

    #[test]
    fn rejects_running_proxy_stdio_server_when_stdin_is_terminal() {
        let error = validate_proxy_stdio_launch(true, false).unwrap_err();

        assert_eq!(
            error.to_string(),
            format!("mcp.serve.stdio_host: {STDIO_HOST_REQUIRED_MESSAGE}")
        );
    }

    #[test]
    fn rejects_running_proxy_stdio_server_when_stdout_is_terminal() {
        let error = validate_proxy_stdio_launch(false, true).unwrap_err();

        assert_eq!(
            error.to_string(),
            format!("mcp.serve.stdio_host: {STDIO_HOST_REQUIRED_MESSAGE}")
        );
    }

    fn unique_test_home(name: &str) -> std::path::PathBuf {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();

        env::temp_dir().join(format!("mcp-smart-proxy-{unique}-{name}"))
    }
}
