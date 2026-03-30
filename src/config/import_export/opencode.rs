use std::collections::BTreeMap;
use std::error::Error;
use std::fs;
use std::path::Path;

use serde_json::{Map as JsonMap, Value as JsonValue};

use crate::fs_util::write_file_atomically;
use crate::paths::format_path_for_display;

use super::super::local::parse_json_string_object;
use super::super::provider::opencode_config_path;
use super::super::self_server::{inspect_opencode_self_server, opencode_server_raw_command};
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

pub fn load_opencode_servers_for_import() -> Result<(std::path::PathBuf, ImportPlan), Box<dyn Error>>
{
    load_provider_import_plan(
        opencode_config_path,
        load_opencode_servers_for_import_from_path,
    )
}

pub fn install_opencode_mcp_server() -> Result<InstallMcpServerResult, Box<dyn Error>> {
    install_json_mcp_server(
        opencode_config_path()?,
        super::super::OPENCODE_PROVIDER_NAME,
        JsonInstallAdapter {
            load_config: load_opencode_config,
            save_config: save_opencode_config,
            root_error: "OpenCode config root must be a JSON object",
            servers_key: "mcp",
            servers_error: "`mcp` in OpenCode config must be an object",
            inspect_self_server: inspect_opencode_self_server,
            build_server_value: opencode_server_value,
        },
    )
}

pub fn replace_opencode_mcp_servers() -> Result<ReplaceMcpServersResult, Box<dyn Error>> {
    let config_path = opencode_config_path()?;
    replace_opencode_mcp_servers_from_path(&config_path)
}

pub fn restore_opencode_mcp_servers() -> Result<RestoreMcpServersResult, Box<dyn Error>> {
    let config_path = opencode_config_path()?;
    restore_opencode_mcp_servers_from_path(&config_path)
}

pub(crate) fn replace_opencode_mcp_servers_from_path(
    config_path: &Path,
) -> Result<ReplaceMcpServersResult, Box<dyn Error>> {
    replace_json_mcp_servers_from_path(
        config_path,
        JsonReplaceAdapter {
            load_config: load_opencode_config,
            save_config: save_opencode_config,
            root_error: "OpenCode config root must be a JSON object",
            servers_key: "mcp",
            servers_error: "`mcp` in OpenCode config must be an object",
            filter_backup_servers: opencode_backup_servers,
            merge_into_backup: merge_opencode_servers_into_backup,
        },
    )
}

pub(crate) fn restore_opencode_mcp_servers_from_path(
    config_path: &Path,
) -> Result<RestoreMcpServersResult, Box<dyn Error>> {
    restore_json_mcp_servers_from_path(
        config_path,
        JsonRestoreAdapter {
            load_config: load_opencode_config,
            save_config: save_opencode_config,
            load_backup: load_required_opencode_backup,
            backup_servers_key: "mcp",
            missing_backup_servers: missing_opencode_backup_servers_error,
            remove_self_servers: remove_opencode_self_servers,
            merge_into_target: merge_opencode_servers_into_target,
            filter_backup_servers: opencode_backup_servers,
        },
    )
}

pub(crate) fn load_opencode_servers_for_import_from_path(
    path: &Path,
) -> Result<ImportPlan, Box<dyn Error>> {
    load_json_import_plan_from_path(
        path,
        JsonImportAdapter {
            config_label: "OpenCode",
            servers_key: "mcp",
            missing_servers: missing_opencode_servers_error,
            empty_servers: empty_opencode_servers_error,
            server_type_label: "OpenCode MCP server",
            validate_server: validate_importable_opencode_server,
            parse_enabled: parse_opencode_import_server_enabled,
            parse_imported_server: opencode_imported_server_command,
        },
    )
}

fn missing_opencode_servers_error(path: &Path) -> String {
    format!(
        "no `mcp` object found in OpenCode config {}",
        format_path_for_display(path)
    )
}

fn empty_opencode_servers_error(path: &Path) -> String {
    format!(
        "no MCP servers found in OpenCode config {}",
        format_path_for_display(path)
    )
}

fn missing_opencode_backup_servers_error(path: &Path) -> String {
    format!(
        "no `mcp` object found in OpenCode backup {}",
        format_path_for_display(path)
    )
}

fn parse_opencode_import_server_enabled(
    server: &JsonMap<String, JsonValue>,
    name: &str,
) -> Result<bool, Box<dyn Error>> {
    match server.get("enabled") {
        Some(JsonValue::Bool(enabled)) => Ok(*enabled),
        Some(_) => {
            Err(format!("OpenCode MCP server `{name}` has a non-boolean `enabled` field").into())
        }
        None => Ok(true),
    }
}

fn opencode_imported_server_command(
    server: &JsonMap<String, JsonValue>,
    name: &str,
) -> Result<ImportedServerDefinition, Box<dyn Error>> {
    match server.get("type").and_then(JsonValue::as_str).unwrap_or("local") {
        "local" => {
            let command = server
                .get("command")
                .and_then(serde_json::Value::as_array)
                .ok_or_else(|| format!("OpenCode MCP server `{name}` is missing `command`"))?;
            if command.is_empty() {
                return Err(
                    format!("OpenCode MCP server `{name}` has an empty `command` array").into(),
                );
            }

            let raw_command = command
                .iter()
                .map(|value| {
                    value.as_str().map(ToOwned::to_owned).ok_or_else(|| {
                        format!("OpenCode MCP server `{name}` contains a non-string command part")
                    })
                })
                .collect::<Result<Vec<_>, _>>()?;
            let env = parse_json_string_object(
                server.get("environment"),
                "environment",
                "OpenCode MCP server",
                name,
            )?;
            Ok(ImportedServerDefinition {
                command: raw_command,
                url: None,
                headers: BTreeMap::new(),
                env,
                env_vars: Vec::new(),
            })
        }
        "remote" => {
            let url = server
                .get("url")
                .and_then(JsonValue::as_str)
                .ok_or_else(|| format!("OpenCode MCP server `{name}` is missing `url`"))?;
            let headers = parse_json_string_object(
                server.get("headers"),
                "headers",
                "OpenCode MCP server",
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
            "OpenCode MCP server `{name}` uses unsupported type `{other}`, only `local` and `remote` can be imported"
        )
        .into()),
    }
}

fn opencode_server_value(server: &StdioServer) -> JsonValue {
    JsonValue::Object(JsonMap::from_iter([
        ("type".to_string(), JsonValue::String("local".to_string())),
        (
            "command".to_string(),
            JsonValue::Array(
                server
                    .raw_command()
                    .into_iter()
                    .map(JsonValue::String)
                    .collect(),
            ),
        ),
    ]))
}

pub(crate) fn load_opencode_config(path: &Path) -> Result<JsonValue, Box<dyn Error>> {
    if !path.exists() {
        return Ok(JsonValue::Object(JsonMap::new()));
    }

    let contents = fs::read_to_string(path)?;
    let value = serde_json::from_str(&contents)?;
    Ok(value)
}

fn save_opencode_config(path: &Path, config: &JsonValue) -> Result<(), Box<dyn Error>> {
    let contents = serde_json::to_string_pretty(config)?;
    write_file_atomically(path, contents.as_bytes())?;
    Ok(())
}

fn merge_opencode_servers_into_backup(
    backup_path: &Path,
    servers: &JsonMap<String, JsonValue>,
) -> Result<(), Box<dyn Error>> {
    let mut backup = load_opencode_config(backup_path)?;
    let root = backup
        .as_object_mut()
        .ok_or_else(|| "OpenCode backup root must be a JSON object".to_string())?;
    let backup_servers_value = root
        .entry("mcp".to_string())
        .or_insert_with(|| JsonValue::Object(JsonMap::new()));
    let backup_servers = backup_servers_value
        .as_object_mut()
        .ok_or_else(|| "`mcp` in OpenCode backup must be an object".to_string())?;

    for (name, server) in servers {
        backup_servers.insert(name.clone(), server.clone());
    }

    save_opencode_config(backup_path, &backup)?;
    Ok(())
}

fn opencode_backup_servers(servers: &JsonMap<String, JsonValue>) -> JsonMap<String, JsonValue> {
    servers
        .iter()
        .filter(|(_, value)| {
            value
                .as_object()
                .and_then(opencode_server_raw_command)
                .is_none_or(|raw_command| !super::super::is_self_server_command(&raw_command))
        })
        .map(|(name, value)| (name.clone(), value.clone()))
        .collect()
}

fn merge_opencode_servers_into_target(
    config: &mut JsonValue,
    servers: &JsonMap<String, JsonValue>,
) -> Result<(), Box<dyn Error>> {
    let root = config
        .as_object_mut()
        .ok_or_else(|| "OpenCode config root must be a JSON object".to_string())?;
    let target_servers_value = root
        .entry("mcp".to_string())
        .or_insert_with(|| JsonValue::Object(JsonMap::new()));
    let target_servers = target_servers_value
        .as_object_mut()
        .ok_or_else(|| "`mcp` in OpenCode config must be an object".to_string())?;

    for (name, server) in servers {
        target_servers.insert(name.clone(), server.clone());
    }

    Ok(())
}

fn load_required_opencode_backup(path: &Path) -> Result<JsonValue, Box<dyn Error>> {
    if !path.exists() {
        return Err(format!(
            "OpenCode backup not found at {}",
            format_path_for_display(path)
        )
        .into());
    }

    load_opencode_config(path)
}

fn remove_opencode_self_servers(config: &mut JsonValue) -> Result<usize, Box<dyn Error>> {
    let root = config
        .as_object_mut()
        .ok_or_else(|| "OpenCode config root must be a JSON object".to_string())?;
    let Some(servers_value) = root.get_mut("mcp") else {
        return Ok(0);
    };
    let servers = servers_value
        .as_object_mut()
        .ok_or_else(|| "`mcp` in OpenCode config must be an object".to_string())?;

    let names = servers
        .iter()
        .filter_map(|(name, value)| {
            let server = value.as_object()?;
            let raw_command = opencode_server_raw_command(server)?;
            super::super::is_self_server_command(&raw_command).then_some(name.clone())
        })
        .collect::<Vec<_>>();

    for name in &names {
        servers.remove(name);
    }

    if servers.is_empty() {
        root.remove("mcp");
    }

    Ok(names.len())
}

fn validate_importable_opencode_server(
    name: &str,
    server: &serde_json::Map<String, serde_json::Value>,
) -> Result<(), Box<dyn Error>> {
    let server_type = match server.get("type") {
        Some(JsonValue::String(value)) => value.as_str(),
        Some(_) => {
            return Err(
                format!("OpenCode MCP server `{name}` has a non-string `type` field").into(),
            );
        }
        None => "local",
    };

    let supported_keys = match server_type {
        "local" => ["command", "type", "enabled", "environment"].as_slice(),
        "remote" => ["url", "type", "enabled", "headers"].as_slice(),
        other => {
            return Err(format!(
                "OpenCode MCP server `{name}` uses unsupported type `{other}`, only `local` and `remote` can be imported"
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
            "OpenCode MCP server `{name}` uses unsupported settings {}; only {} can be imported",
            unsupported_keys.join(", "),
            match server_type {
                "local" => "`command` and optional `type`, `enabled`, and `environment`",
                "remote" => "`url` and optional `type`, `enabled`, and `headers`",
                _ => unreachable!(),
            }
        )
        .into())
    }
}
