use std::collections::HashMap;
use std::error::Error;
use std::fs;
use std::future::Future;
use std::path::Path;
use std::process::Stdio;
use std::sync::Arc;

use rmcp::{
    ErrorData as McpError, RoleClient, ServerHandler, ServiceExt,
    model::{
        CallToolRequestMethod, CallToolRequestParams, CallToolResult, ListToolsResult,
        PaginatedRequestParams, ServerCapabilities, ServerInfo, Tool, ToolAnnotations, object,
    },
    service::{ClientInitializeError, RequestContext, RunningService, ServiceError},
    transport::{ConfigureCommandExt, TokioChildProcess, stdio},
};
use serde::Deserialize;
use serde_json::{Map as JsonMap, Value as JsonValue, json};
use tokio::sync::Mutex;
use toml::{Table, Value};

use crate::config::{configured_server, list_servers, load_config_table};
use crate::console::{
    ExternalOutputRouter, describe_command, operation_error, print_external_command_failure,
    print_external_output_if_present, spawn_stderr_collector,
};
use crate::paths::{cache_file_path_from_home, home_dir, sanitize_name};
use crate::reload::reload_server;
use crate::types::{CachedTools, ConfiguredServer, ToolSnapshot};

const ACTIVATE_EXTERNAL_MCP_NAME: &str = "activate_external_mcp";
const CALL_TOOL_IN_EXTERNAL_MCP_NAME: &str = "call_tool_in_external_mcp";

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
struct CallToolInExternalMcpRequest {
    external_mcp_name: String,
    tool_name: String,
    args_in_json: String,
}

pub async fn serve_cached_toolsets(config_path: &Path) -> Result<(), Box<dyn Error>> {
    reload_all_toolsets(config_path).await.map_err(|error| {
        operation_error(
            "mcp.reload_all_toolsets",
            format!(
                "failed to reload configured MCP servers before starting proxy with config {}",
                config_path.display()
            ),
            error,
        )
    })?;

    let config = load_config_table(config_path).map_err(|error| {
        operation_error(
            "mcp.load_config",
            format!("failed to load config from {}", config_path.display()),
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
        .map_err(|error| {
            operation_error(
                "mcp.serve",
                "failed to start the proxy stdio MCP server",
                Box::new(error),
            )
        })?;
    service.waiting().await.map_err(|error| {
        operation_error(
            "mcp.wait",
            "proxy stdio MCP server exited with an error",
            Box::new(error),
        )
    })?;
    Ok(())
}

async fn reload_all_toolsets(config_path: &Path) -> Result<(), Box<dyn Error>> {
    let servers = list_servers(config_path).map_err(|error| {
        operation_error(
            "mcp.reload_all_toolsets.list_servers",
            format!(
                "failed to list configured MCP servers from {} before startup reload",
                config_path.display()
            ),
            error,
        )
    })?;

    for server in servers {
        let server_name = server.name;
        reload_server(config_path, &server_name)
            .await
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
    .annotate(
        ToolAnnotations::new()
            .read_only(true)
            .destructive(false)
            .idempotent(true)
            .open_world(false),
    )
}

fn call_tool_in_external_mcp_definition() -> Tool {
    Tool::new(
        CALL_TOOL_IN_EXTERNAL_MCP_NAME,
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
    .annotate(
        ToolAnnotations::new()
            .read_only(false)
            .destructive(true)
            .idempotent(false)
            .open_world(true),
    )
}

fn resolve_toolset_name<'a>(
    toolsets: &'a [CachedToolsetRecord],
    requested_name: &str,
) -> Option<&'a CachedToolsetRecord> {
    toolsets
        .iter()
        .find(|toolset| toolset.name == requested_name)
        .or_else(|| {
            let sanitized = sanitize_name(requested_name);
            toolsets.iter().find(|toolset| toolset.name == sanitized)
        })
}

fn build_activate_tool_result(toolset: &CachedToolsetRecord) -> CallToolResult {
    CallToolResult::structured(json!({
        "tools": toolset.tools,
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

fn map_client_initialize_error(error: ClientInitializeError) -> McpError {
    match error {
        ClientInitializeError::JsonRpcError(error) => error,
        other => McpError::internal_error(other.to_string(), None),
    }
}

async fn connect_toolset_client(server: &ConfiguredServer) -> Result<ToolsetClient, McpError> {
    let command_line = describe_command(&server.command, &server.args);
    let stderr_router = ExternalOutputRouter::new();
    let stderr_capture = stderr_router.start_capture().await;
    let (transport, stderr) = TokioChildProcess::builder(
        tokio::process::Command::new(&server.command).configure(|cmd| {
            cmd.args(&server.args);
        }),
    )
    .stderr(Stdio::piped())
    .spawn()
    .map_err(|error| McpError::internal_error(error.to_string(), None))?;
    let label = server.command.clone();

    if let Some(stderr) = stderr {
        spawn_stderr_collector(
            "mcp.connect_toolset_client".to_string(),
            label.clone(),
            command_line.clone(),
            stderr,
            stderr_router.clone(),
        );
    }

    let client = match ().serve(transport).await {
        Ok(client) => client,
        Err(error) => {
            let stderr_content = stderr_capture.finish().await;
            print_external_command_failure(
                "mcp.connect_toolset_client",
                &label,
                &command_line,
                "connect-failed",
            );
            print_external_output_if_present(
                "mcp.connect_toolset_client",
                &label,
                &command_line,
                "stderr",
                &stderr_content,
            )
            .await;
            return Err(map_client_initialize_error(error));
        }
    };
    let _ = stderr_capture.finish().await;

    Ok(ToolsetClient {
        service: Arc::new(client),
        stderr: stderr_router,
        command_line,
        label,
    })
}

struct SmartProxyMcpServer {
    activate_tool: Tool,
    call_tool_in_external_mcp: Tool,
    toolsets: Vec<CachedToolsetRecord>,
    clients: Mutex<HashMap<String, ToolsetClient>>,
}

impl SmartProxyMcpServer {
    fn new(toolsets: Vec<CachedToolsetRecord>) -> Self {
        let activate_tool = activate_tool_definition(&toolsets);
        let call_tool_in_external_mcp = call_tool_in_external_mcp_definition();
        Self {
            activate_tool,
            call_tool_in_external_mcp,
            toolsets,
            clients: Mutex::new(HashMap::new()),
        }
    }

    async fn get_or_connect_client(
        &self,
        toolset: &CachedToolsetRecord,
    ) -> Result<ToolsetClient, McpError> {
        let mut clients = self.clients.lock().await;

        if let Some(client) = clients.get(&toolset.name).cloned() {
            if !client.service.is_closed() {
                return Ok(client);
            }
            clients.remove(&toolset.name);
        }

        let client = connect_toolset_client(&toolset.server).await?;
        clients.insert(toolset.name.clone(), client.clone());
        Ok(client)
    }
}

impl ServerHandler for SmartProxyMcpServer {
    fn get_info(&self) -> ServerInfo {
        ServerInfo {
            instructions: Some(
                "Use `activate_external_mcp` to inspect cached tools, then `call_tool_in_external_mcp` to invoke a specific downstream MCP tool."
                    .into(),
            ),
            capabilities: ServerCapabilities::builder().enable_tools().build(),
            ..Default::default()
        }
    }

    fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<rmcp::RoleServer>,
    ) -> impl Future<Output = Result<ListToolsResult, McpError>> + Send + '_ {
        std::future::ready(Ok(ListToolsResult {
            tools: vec![
                self.activate_tool.clone(),
                self.call_tool_in_external_mcp.clone(),
            ],
            ..Default::default()
        }))
    }

    fn get_tool(&self, name: &str) -> Option<Tool> {
        match name {
            ACTIVATE_EXTERNAL_MCP_NAME => Some(self.activate_tool.clone()),
            CALL_TOOL_IN_EXTERNAL_MCP_NAME => Some(self.call_tool_in_external_mcp.clone()),
            _ => None,
        }
    }

    fn call_tool(
        &self,
        request: CallToolRequestParams,
        _context: RequestContext<rmcp::RoleServer>,
    ) -> impl Future<Output = Result<CallToolResult, McpError>> + Send + '_ {
        async move {
            let arguments = request
                .arguments
                .ok_or_else(|| McpError::invalid_params("tool arguments are required", None))?;

            match request.name.as_ref() {
                ACTIVATE_EXTERNAL_MCP_NAME => {
                    let params: ActivateExternalMcpRequest =
                        serde_json::from_value(JsonValue::Object(arguments)).map_err(|error| {
                            McpError::invalid_params(
                                format!("invalid activate_external_mcp arguments: {error}"),
                                None,
                            )
                        })?;

                    let Some(toolset) =
                        resolve_toolset_name(&self.toolsets, &params.external_mcp_name)
                    else {
                        return Err(McpError::invalid_params(
                            format!("unknown external MCP server `{}`", params.external_mcp_name),
                            Some(json!({
                                "available_external_mcps": self
                                    .toolsets
                                    .iter()
                                    .map(|toolset| toolset.name.clone())
                                    .collect::<Vec<_>>()
                            })),
                        ));
                    };

                    Ok(build_activate_tool_result(toolset))
                }
                CALL_TOOL_IN_EXTERNAL_MCP_NAME => {
                    let params: CallToolInExternalMcpRequest =
                        serde_json::from_value(JsonValue::Object(arguments)).map_err(|error| {
                            McpError::invalid_params(
                                format!("invalid call_tool_in_external_mcp arguments: {error}"),
                                None,
                            )
                        })?;

                    let Some(toolset) =
                        resolve_toolset_name(&self.toolsets, &params.external_mcp_name)
                    else {
                        return Err(McpError::invalid_params(
                            format!("unknown external MCP server `{}`", params.external_mcp_name),
                            Some(json!({
                                "available_external_mcps": self
                                    .toolsets
                                    .iter()
                                    .map(|toolset| toolset.name.clone())
                                    .collect::<Vec<_>>()
                            })),
                        ));
                    };

                    let arguments = parse_tool_arguments_json(&params.args_in_json)?;
                    let client = self.get_or_connect_client(toolset).await?;
                    let stderr_capture = client.stderr.start_capture().await;
                    match client
                        .service
                        .call_tool(CallToolRequestParams {
                            meta: None,
                            name: params.tool_name.into(),
                            arguments,
                            task: None,
                        })
                        .await
                    {
                        Ok(result) => {
                            let _ = stderr_capture.finish().await;
                            Ok(result)
                        }
                        Err(error) => {
                            let stderr_content = stderr_capture.finish().await;
                            print_external_command_failure(
                                "mcp.call_tool_in_external_mcp",
                                &client.label,
                                &client.command_line,
                                "tool-call-failed",
                            );
                            print_external_output_if_present(
                                "mcp.call_tool_in_external_mcp",
                                &client.label,
                                &client.command_line,
                                "stderr",
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
                    command: "uvx".to_string(),
                    args: vec!["alpha".to_string()],
                },
                tools: vec![],
            },
            CachedToolsetRecord {
                name: "beta".to_string(),
                summary: "Use this for Beta tasks.".to_string(),
                server: ConfiguredServer {
                    command: "uvx".to_string(),
                    args: vec!["beta".to_string()],
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
        assert_eq!(toolsets[0].server.command, "uvx");
        assert_eq!(toolsets[0].server.args, vec!["alpha-server".to_string()]);
    }

    #[test]
    fn resolves_toolset_by_sanitized_name() {
        let toolsets = vec![CachedToolsetRecord {
            name: "team-alpha".to_string(),
            summary: "Use Alpha.".to_string(),
            server: ConfiguredServer {
                command: "uvx".to_string(),
                args: vec!["alpha".to_string()],
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
                command: "uvx".to_string(),
                args: vec!["alpha".to_string()],
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

        assert_eq!(
            result.structured_content,
            Some(json!({
                "tools": [{
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
                }]
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
        let tool = call_tool_in_external_mcp_definition();
        let properties = tool
            .input_schema
            .get("properties")
            .and_then(JsonValue::as_object)
            .unwrap();

        assert!(properties.contains_key("external_mcp_name"));
        assert!(properties.contains_key("tool_name"));
        assert!(properties.contains_key("args_in_json"));
    }

    fn unique_test_home(name: &str) -> std::path::PathBuf {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();

        env::temp_dir().join(format!("mcp-smart-proxy-{unique}-{name}"))
    }
}
