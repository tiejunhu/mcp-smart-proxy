use std::collections::BTreeMap;
use std::error::Error;
use std::path::Path;

use serde_json::{Map as JsonMap, Value as JsonValue};

use crate::paths::format_path_for_display;

use super::super::local::parse_json_string_object;
use super::super::provider::copilot_config_path;
use super::super::self_server::{copilot_server_raw_command, inspect_copilot_self_server};
use super::super::{
    ImportPlan, ImportedServerDefinition, InstallMcpServerResult, ReplaceMcpServersResult,
    RestoreMcpServersResult, StdioServer,
};
use super::common::{collect_remote_header_env_vars, load_provider_import_plan};
use super::json_support::{
    JsonImportAdapter, JsonInstallAdapter, JsonReplaceAdapter, JsonRestoreAdapter,
    install_json_mcp_server, load_json_import_plan_from_path, load_json_object_config,
    load_required_json_object_config, merge_json_servers_into_config, merge_json_servers_into_file,
    remove_json_self_servers, replace_json_mcp_servers_from_path,
    restore_json_mcp_servers_from_path, save_json_object_config,
};

pub fn load_copilot_servers_for_import() -> Result<(std::path::PathBuf, ImportPlan), Box<dyn Error>>
{
    load_provider_import_plan(
        copilot_config_path,
        load_copilot_servers_for_import_from_path,
    )
}

pub fn install_copilot_mcp_server() -> Result<InstallMcpServerResult, Box<dyn Error>> {
    install_json_mcp_server(
        copilot_config_path()?,
        super::super::COPILOT_PROVIDER_NAME,
        JsonInstallAdapter {
            load_config: load_copilot_config,
            save_config: save_copilot_config,
            root_error: "Copilot CLI config root must be a JSON object",
            servers_key: "mcpServers",
            servers_error: "`mcpServers` in Copilot CLI config must be an object",
            inspect_self_server: inspect_copilot_self_server,
            build_server_value: copilot_server_value,
        },
    )
}

pub fn replace_copilot_mcp_servers() -> Result<ReplaceMcpServersResult, Box<dyn Error>> {
    let config_path = copilot_config_path()?;
    replace_copilot_mcp_servers_from_path(&config_path)
}

pub fn restore_copilot_mcp_servers() -> Result<RestoreMcpServersResult, Box<dyn Error>> {
    let config_path = copilot_config_path()?;
    restore_copilot_mcp_servers_from_path(&config_path)
}

pub(crate) fn replace_copilot_mcp_servers_from_path(
    config_path: &Path,
) -> Result<ReplaceMcpServersResult, Box<dyn Error>> {
    replace_json_mcp_servers_from_path(
        config_path,
        JsonReplaceAdapter {
            load_config: load_copilot_config,
            save_config: save_copilot_config,
            root_error: "Copilot CLI config root must be a JSON object",
            servers_key: "mcpServers",
            servers_error: "`mcpServers` in Copilot CLI config must be an object",
            filter_backup_servers: copilot_backup_servers,
            preserve_server: copilot_preserve_server,
            merge_into_backup: merge_copilot_servers_into_backup,
        },
    )
}

pub(crate) fn restore_copilot_mcp_servers_from_path(
    config_path: &Path,
) -> Result<RestoreMcpServersResult, Box<dyn Error>> {
    restore_json_mcp_servers_from_path(
        config_path,
        JsonRestoreAdapter {
            load_config: load_copilot_config,
            save_config: save_copilot_config,
            load_backup: load_required_copilot_backup,
            backup_servers_key: "mcpServers",
            missing_backup_servers: missing_copilot_backup_servers_error,
            remove_self_servers: remove_copilot_self_servers,
            merge_into_target: merge_copilot_servers_into_target,
            filter_backup_servers: copilot_backup_servers,
        },
    )
}

pub(crate) fn load_copilot_servers_for_import_from_path(
    path: &Path,
) -> Result<ImportPlan, Box<dyn Error>> {
    load_json_import_plan_from_path(
        path,
        JsonImportAdapter {
            config_label: "Copilot CLI",
            servers_key: "mcpServers",
            missing_servers: missing_copilot_servers_error,
            empty_servers: empty_copilot_servers_error,
            server_type_label: "Copilot CLI MCP server",
            validate_server: validate_importable_copilot_server,
            parse_enabled: parse_copilot_import_server_enabled,
            parse_imported_server: copilot_imported_server_command,
        },
    )
}

fn missing_copilot_servers_error(path: &Path) -> String {
    format!(
        "no `mcpServers` object found in Copilot CLI config {}",
        format_path_for_display(path)
    )
}

fn empty_copilot_servers_error(path: &Path) -> String {
    format!(
        "no MCP servers found in Copilot CLI config {}",
        format_path_for_display(path)
    )
}

fn missing_copilot_backup_servers_error(path: &Path) -> String {
    format!(
        "no `mcpServers` object found in Copilot CLI backup {}",
        format_path_for_display(path)
    )
}

fn parse_copilot_import_server_enabled(
    server: &JsonMap<String, JsonValue>,
    name: &str,
) -> Result<bool, Box<dyn Error>> {
    match (server.get("enabled"), server.get("disabled")) {
        (Some(JsonValue::Bool(enabled)), _) => Ok(*enabled),
        (Some(_), _) => {
            Err(format!("Copilot CLI MCP server `{name}` has a non-boolean `enabled` field").into())
        }
        (None, Some(JsonValue::Bool(disabled))) => Ok(!disabled),
        (None, Some(_)) => Err(format!(
            "Copilot CLI MCP server `{name}` has a non-boolean `disabled` field"
        )
        .into()),
        (None, None) => Ok(true),
    }
}

fn copilot_imported_server_command(
    server: &JsonMap<String, JsonValue>,
    name: &str,
) -> Result<ImportedServerDefinition, Box<dyn Error>> {
    match copilot_server_type(server)? {
        "local" | "stdio" => {
            let command = server
                .get("command")
                .and_then(JsonValue::as_str)
                .ok_or_else(|| format!("Copilot CLI MCP server `{name}` is missing `command`"))?;
            let args = match server.get("args") {
                None => Vec::new(),
                Some(JsonValue::Array(items)) => items
                    .iter()
                    .map(|value| {
                        value.as_str().map(ToOwned::to_owned).ok_or_else(|| {
                            format!("Copilot CLI MCP server `{name}` contains a non-string arg")
                        })
                    })
                    .collect::<Result<Vec<_>, _>>()?,
                Some(_) => {
                    return Err(
                        format!("Copilot CLI MCP server `{name}` has a non-array `args` field")
                            .into(),
                    );
                }
            };
            let env = parse_json_string_object(
                server.get("env"),
                "env",
                "Copilot CLI MCP server",
                name,
            )?;
            let mut raw_command = vec![command.to_string()];
            raw_command.extend(args);
            Ok(ImportedServerDefinition {
                command: raw_command,
                url: None,
                headers: BTreeMap::new(),
                env,
                env_vars: Vec::new(),
            })
        }
        "http" | "sse" => {
            let url = server
                .get("url")
                .and_then(JsonValue::as_str)
                .ok_or_else(|| format!("Copilot CLI MCP server `{name}` is missing `url`"))?;
            let headers = parse_json_string_object(
                server.get("headers"),
                "headers",
                "Copilot CLI MCP server",
                name,
            )?;
            let env_vars = collect_remote_header_env_vars(&headers);
            Ok(ImportedServerDefinition {
                command: Vec::new(),
                url: Some(url.to_string()),
                headers,
                env: BTreeMap::new(),
                env_vars,
            })
        }
        other => Err(format!(
            "Copilot CLI MCP server `{name}` uses unsupported type `{other}`, only `local`, `stdio`, `http`, and `sse` can be imported"
        )
        .into()),
    }
}

fn copilot_server_value(server: &StdioServer) -> JsonValue {
    JsonValue::Object(JsonMap::from_iter([
        ("type".to_string(), JsonValue::String("stdio".to_string())),
        (
            "command".to_string(),
            JsonValue::String(server.command.clone()),
        ),
        (
            "args".to_string(),
            JsonValue::Array(server.args.iter().cloned().map(JsonValue::String).collect()),
        ),
        (
            "tools".to_string(),
            JsonValue::Array(vec![JsonValue::String("*".to_string())]),
        ),
    ]))
}

pub(crate) fn load_copilot_config(path: &Path) -> Result<JsonValue, Box<dyn Error>> {
    load_json_object_config(path)
}

fn save_copilot_config(path: &Path, config: &JsonValue) -> Result<(), Box<dyn Error>> {
    save_json_object_config(path, config)
}

fn merge_copilot_servers_into_backup(
    backup_path: &Path,
    servers: &JsonMap<String, JsonValue>,
) -> Result<(), Box<dyn Error>> {
    merge_json_servers_into_file(
        backup_path,
        load_copilot_config,
        save_copilot_config,
        "Copilot CLI backup root must be a JSON object",
        "mcpServers",
        "`mcpServers` in Copilot CLI backup must be an object",
        servers,
    )
}

fn copilot_backup_servers(servers: &JsonMap<String, JsonValue>) -> JsonMap<String, JsonValue> {
    servers
        .iter()
        .filter(|(_, value)| value.as_object().is_some_and(copilot_should_backup_server))
        .map(|(name, value)| (name.clone(), value.clone()))
        .collect()
}

fn copilot_should_backup_server(server: &JsonMap<String, JsonValue>) -> bool {
    !server
        .get("url")
        .and_then(JsonValue::as_str)
        .is_some_and(super::super::local::is_unsupported_remote_server_url)
        && copilot_server_raw_command(server)
            .is_none_or(|raw_command| !super::super::is_self_server_command(&raw_command))
}

fn copilot_preserve_server(server: &JsonMap<String, JsonValue>) -> bool {
    server
        .get("url")
        .and_then(JsonValue::as_str)
        .is_some_and(super::super::local::is_unsupported_remote_server_url)
}

fn merge_copilot_servers_into_target(
    config: &mut JsonValue,
    servers: &JsonMap<String, JsonValue>,
) -> Result<(), Box<dyn Error>> {
    merge_json_servers_into_config(
        config,
        "Copilot CLI config root must be a JSON object",
        "mcpServers",
        "`mcpServers` in Copilot CLI config must be an object",
        servers,
    )
}

fn load_required_copilot_backup(path: &Path) -> Result<JsonValue, Box<dyn Error>> {
    load_required_json_object_config(path, "Copilot CLI backup")
}

fn remove_copilot_self_servers(config: &mut JsonValue) -> Result<usize, Box<dyn Error>> {
    remove_json_self_servers(
        config,
        "Copilot CLI config root must be a JSON object",
        "mcpServers",
        "`mcpServers` in Copilot CLI config must be an object",
        copilot_server_raw_command,
    )
}

fn validate_importable_copilot_server(
    name: &str,
    server: &serde_json::Map<String, serde_json::Value>,
) -> Result<(), Box<dyn Error>> {
    let server_type = copilot_server_type(server)?;

    let supported_keys = match server_type {
        "local" | "stdio" => [
            "command", "args", "env", "tools", "type", "enabled", "disabled",
        ]
        .as_slice(),
        "http" | "sse" => ["url", "headers", "tools", "type", "enabled", "disabled"].as_slice(),
        other => {
            return Err(format!(
                "Copilot CLI MCP server `{name}` uses unsupported type `{other}`, only `local`, `stdio`, `http`, and `sse` can be imported"
            )
            .into());
        }
    };

    let unsupported_keys = server
        .keys()
        .filter(|key| !supported_keys.contains(&key.as_str()))
        .map(|key| format!("`{key}`"))
        .collect::<Vec<_>>();

    if unsupported_keys.is_empty() {
        Ok(())
    } else {
        Err(format!(
            "Copilot CLI MCP server `{name}` uses unsupported settings {}; only {} can be imported",
            unsupported_keys.join(", "),
            match server_type {
                "local" | "stdio" => {
                    "`command`, optional `args`, optional `env`, optional `tools`, optional `enabled` or `disabled`, and optional `type`"
                }
                "http" | "sse" => {
                    "`url`, optional `headers`, optional `tools`, optional `enabled` or `disabled`, and optional `type`"
                }
                _ => unreachable!(),
            }
        )
        .into())
    }
}

fn copilot_server_type(server: &JsonMap<String, JsonValue>) -> Result<&str, Box<dyn Error>> {
    match server.get("type") {
        Some(JsonValue::String(value)) => Ok(value.as_str()),
        Some(_) => Err("Copilot CLI MCP server has a non-string `type` field".into()),
        None if server.get("command").is_some() => Ok("local"),
        None if server.get("url").is_some() => Ok("http"),
        None => Ok("local"),
    }
}
