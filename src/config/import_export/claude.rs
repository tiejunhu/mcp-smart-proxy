use std::collections::BTreeMap;
use std::error::Error;
use std::fs;
use std::path::Path;

use serde_json::{Map as JsonMap, Value as JsonValue};

use crate::fs_util::write_file_atomically;
use crate::paths::format_path_for_display;

use super::super::local::parse_json_string_object;
use super::super::provider::claude_config_path;
use super::super::self_server::{claude_server_raw_command, inspect_claude_self_server};
use super::super::{
    ImportPlan, ImportedServerDefinition, InstallMcpServerResult, ReplaceMcpServersResult,
    RestoreMcpServersResult, StdioServer,
};
use super::{
    JsonImportAdapter, JsonInstallAdapter, JsonReplaceAdapter, JsonRestoreAdapter,
    collect_remote_header_env_vars, install_json_mcp_server, load_json_import_plan_from_path,
    load_provider_import_plan, replace_json_mcp_servers_from_path,
    restore_json_mcp_servers_from_path,
};

pub fn load_claude_servers_for_import() -> Result<(std::path::PathBuf, ImportPlan), Box<dyn Error>>
{
    load_provider_import_plan(claude_config_path, load_claude_servers_for_import_from_path)
}

pub fn install_claude_mcp_server() -> Result<InstallMcpServerResult, Box<dyn Error>> {
    install_json_mcp_server(
        claude_config_path()?,
        super::super::CLAUDE_PROVIDER_NAME,
        JsonInstallAdapter {
            load_config: load_claude_config,
            save_config: save_claude_config,
            root_error: "Claude Code config root must be a JSON object",
            servers_key: "mcpServers",
            servers_error: "`mcpServers` in Claude Code config must be an object",
            inspect_self_server: inspect_claude_self_server,
            build_server_value: claude_server_value,
        },
    )
}

pub fn replace_claude_mcp_servers() -> Result<ReplaceMcpServersResult, Box<dyn Error>> {
    let config_path = claude_config_path()?;
    replace_claude_mcp_servers_from_path(&config_path)
}

pub fn restore_claude_mcp_servers() -> Result<RestoreMcpServersResult, Box<dyn Error>> {
    let config_path = claude_config_path()?;
    restore_claude_mcp_servers_from_path(&config_path)
}

pub(crate) fn replace_claude_mcp_servers_from_path(
    config_path: &Path,
) -> Result<ReplaceMcpServersResult, Box<dyn Error>> {
    replace_json_mcp_servers_from_path(
        config_path,
        JsonReplaceAdapter {
            load_config: load_claude_config,
            save_config: save_claude_config,
            root_error: "Claude Code config root must be a JSON object",
            servers_key: "mcpServers",
            servers_error: "`mcpServers` in Claude Code config must be an object",
            filter_backup_servers: claude_backup_servers,
            merge_into_backup: merge_claude_servers_into_backup,
        },
    )
}

pub(crate) fn restore_claude_mcp_servers_from_path(
    config_path: &Path,
) -> Result<RestoreMcpServersResult, Box<dyn Error>> {
    restore_json_mcp_servers_from_path(
        config_path,
        JsonRestoreAdapter {
            load_config: load_claude_config,
            save_config: save_claude_config,
            load_backup: load_required_claude_backup,
            backup_servers_key: "mcpServers",
            missing_backup_servers: missing_claude_backup_servers_error,
            remove_self_servers: remove_claude_self_servers,
            merge_into_target: merge_claude_servers_into_target,
            filter_backup_servers: claude_backup_servers,
        },
    )
}

pub(crate) fn load_claude_servers_for_import_from_path(
    path: &Path,
) -> Result<ImportPlan, Box<dyn Error>> {
    load_json_import_plan_from_path(
        path,
        JsonImportAdapter {
            config_label: "Claude Code",
            servers_key: "mcpServers",
            missing_servers: missing_claude_servers_error,
            empty_servers: empty_claude_servers_error,
            server_type_label: "Claude Code MCP server",
            validate_server: validate_importable_claude_server,
            parse_enabled: always_enabled_json_import_server,
            parse_imported_server: claude_imported_server_command,
        },
    )
}

fn missing_claude_servers_error(path: &Path) -> String {
    format!(
        "no `mcpServers` object found in Claude Code config {}",
        format_path_for_display(path)
    )
}

fn empty_claude_servers_error(path: &Path) -> String {
    format!(
        "no MCP servers found in Claude Code config {}",
        format_path_for_display(path)
    )
}

fn missing_claude_backup_servers_error(path: &Path) -> String {
    format!(
        "no `mcpServers` object found in Claude Code backup {}",
        format_path_for_display(path)
    )
}

fn always_enabled_json_import_server(
    _server: &JsonMap<String, JsonValue>,
    _name: &str,
) -> Result<bool, Box<dyn Error>> {
    Ok(true)
}

fn claude_imported_server_command(
    server: &JsonMap<String, JsonValue>,
    name: &str,
) -> Result<ImportedServerDefinition, Box<dyn Error>> {
    match server.get("type").and_then(JsonValue::as_str).unwrap_or("stdio") {
        "stdio" => {
            let command = server
                .get("command")
                .and_then(JsonValue::as_str)
                .ok_or_else(|| format!("Claude Code MCP server `{name}` is missing `command`"))?;
            let args = match server.get("args") {
                None => Vec::new(),
                Some(JsonValue::Array(items)) => items
                    .iter()
                    .map(|value| {
                        value.as_str().map(ToOwned::to_owned).ok_or_else(|| {
                            format!("Claude Code MCP server `{name}` contains a non-string arg")
                        })
                    })
                    .collect::<Result<Vec<_>, _>>()?,
                Some(_) => {
                    return Err(format!(
                        "Claude Code MCP server `{name}` has a non-array `args` field"
                    )
                    .into());
                }
            };
            let env =
                parse_json_string_object(server.get("env"), "env", "Claude Code MCP server", name)?;
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
                .ok_or_else(|| format!("Claude Code MCP server `{name}` is missing `url`"))?;
            let headers = parse_json_string_object(
                server.get("headers"),
                "headers",
                "Claude Code MCP server",
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
            "Claude Code MCP server `{name}` uses unsupported type `{other}`, only `stdio`, `http`, and `sse` can be imported"
        )
        .into()),
    }
}

fn claude_server_value(server: &StdioServer) -> JsonValue {
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
    ]))
}

pub(crate) fn load_claude_config(path: &Path) -> Result<JsonValue, Box<dyn Error>> {
    if !path.exists() {
        return Ok(JsonValue::Object(JsonMap::new()));
    }

    let contents = fs::read_to_string(path)?;
    let value = serde_json::from_str(&contents)?;
    Ok(value)
}

fn save_claude_config(path: &Path, config: &JsonValue) -> Result<(), Box<dyn Error>> {
    let contents = serde_json::to_string_pretty(config)?;
    write_file_atomically(path, contents.as_bytes())?;
    Ok(())
}

fn merge_claude_servers_into_backup(
    backup_path: &Path,
    servers: &JsonMap<String, JsonValue>,
) -> Result<(), Box<dyn Error>> {
    let mut backup = load_claude_config(backup_path)?;
    let root = backup
        .as_object_mut()
        .ok_or_else(|| "Claude Code backup root must be a JSON object".to_string())?;
    let backup_servers_value = root
        .entry("mcpServers".to_string())
        .or_insert_with(|| JsonValue::Object(JsonMap::new()));
    let backup_servers = backup_servers_value
        .as_object_mut()
        .ok_or_else(|| "`mcpServers` in Claude Code backup must be an object".to_string())?;

    for (name, server) in servers {
        backup_servers.insert(name.clone(), server.clone());
    }

    save_claude_config(backup_path, &backup)?;
    Ok(())
}

fn claude_backup_servers(servers: &JsonMap<String, JsonValue>) -> JsonMap<String, JsonValue> {
    servers
        .iter()
        .filter(|(_, value)| {
            value
                .as_object()
                .and_then(claude_server_raw_command)
                .is_none_or(|raw_command| !super::super::is_self_server_command(&raw_command))
        })
        .map(|(name, value)| (name.clone(), value.clone()))
        .collect()
}

fn merge_claude_servers_into_target(
    config: &mut JsonValue,
    servers: &JsonMap<String, JsonValue>,
) -> Result<(), Box<dyn Error>> {
    let root = config
        .as_object_mut()
        .ok_or_else(|| "Claude Code config root must be a JSON object".to_string())?;
    let target_servers_value = root
        .entry("mcpServers".to_string())
        .or_insert_with(|| JsonValue::Object(JsonMap::new()));
    let target_servers = target_servers_value
        .as_object_mut()
        .ok_or_else(|| "`mcpServers` in Claude Code config must be an object".to_string())?;

    for (name, server) in servers {
        target_servers.insert(name.clone(), server.clone());
    }

    Ok(())
}

fn load_required_claude_backup(path: &Path) -> Result<JsonValue, Box<dyn Error>> {
    if !path.exists() {
        return Err(format!(
            "Claude Code backup not found at {}",
            format_path_for_display(path)
        )
        .into());
    }

    load_claude_config(path)
}

fn remove_claude_self_servers(config: &mut JsonValue) -> Result<usize, Box<dyn Error>> {
    let Some(root) = config.as_object_mut() else {
        return Err("Claude Code config root must be a JSON object".into());
    };
    let Some(servers_value) = root.get_mut("mcpServers") else {
        return Ok(0);
    };
    let servers = servers_value
        .as_object_mut()
        .ok_or_else(|| "`mcpServers` in Claude Code config must be an object".to_string())?;

    let names = servers
        .iter()
        .filter_map(|(name, value)| {
            let server = value.as_object()?;
            let raw_command = claude_server_raw_command(server)?;
            super::super::is_self_server_command(&raw_command).then_some(name.clone())
        })
        .collect::<Vec<_>>();

    for name in &names {
        servers.remove(name);
    }

    if servers.is_empty() {
        root.remove("mcpServers");
    }

    Ok(names.len())
}

fn validate_importable_claude_server(
    name: &str,
    server: &serde_json::Map<String, serde_json::Value>,
) -> Result<(), Box<dyn Error>> {
    let server_type = match server.get("type") {
        Some(JsonValue::String(value)) => value.as_str(),
        Some(_) => {
            return Err(
                format!("Claude Code MCP server `{name}` has a non-string `type` field").into(),
            );
        }
        None => "stdio",
    };

    let supported_keys = match server_type {
        "stdio" => ["command", "args", "env", "type"].as_slice(),
        "http" | "sse" => ["url", "headers", "type"].as_slice(),
        other => {
            return Err(format!(
                "Claude Code MCP server `{name}` uses unsupported type `{other}`, only `stdio`, `http`, and `sse` can be imported"
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
            "Claude Code MCP server `{name}` uses unsupported settings {}; only {} can be imported",
            unsupported_keys.join(", "),
            match server_type {
                "stdio" => "`command`, optional `args`, optional `env`, and optional `type`",
                "http" | "sse" => "`url`, optional `headers`, and optional `type`",
                _ => unreachable!(),
            }
        )
        .into())
    }
}
