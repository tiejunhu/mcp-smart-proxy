use std::collections::BTreeMap;
use std::error::Error;
use std::path::Path;

use serde_json::{Map as JsonMap, Value as JsonValue};

use crate::paths::format_path_for_display;

use super::super::local::parse_json_string_object;
use super::super::provider::crush_config_path;
use super::super::self_server::{crush_server_raw_command, inspect_crush_self_server};
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

pub fn load_crush_servers_for_import() -> Result<(std::path::PathBuf, ImportPlan), Box<dyn Error>> {
    load_provider_import_plan(crush_config_path, load_crush_servers_for_import_from_path)
}

pub fn install_crush_mcp_server() -> Result<InstallMcpServerResult, Box<dyn Error>> {
    install_json_mcp_server(
        crush_config_path()?,
        super::super::CRUSH_PROVIDER_NAME,
        JsonInstallAdapter {
            load_config: load_crush_config,
            save_config: save_crush_config,
            root_error: "Crush config root must be a JSON object",
            servers_key: "mcp",
            servers_error: "`mcp` in Crush config must be an object",
            inspect_self_server: inspect_crush_self_server,
            build_server_value: crush_server_value,
        },
    )
}

pub fn replace_crush_mcp_servers() -> Result<ReplaceMcpServersResult, Box<dyn Error>> {
    let config_path = crush_config_path()?;
    replace_crush_mcp_servers_from_path(&config_path)
}

pub fn restore_crush_mcp_servers() -> Result<RestoreMcpServersResult, Box<dyn Error>> {
    let config_path = crush_config_path()?;
    restore_crush_mcp_servers_from_path(&config_path)
}

pub(crate) fn replace_crush_mcp_servers_from_path(
    config_path: &Path,
) -> Result<ReplaceMcpServersResult, Box<dyn Error>> {
    replace_json_mcp_servers_from_path(
        config_path,
        JsonReplaceAdapter {
            load_config: load_crush_config,
            save_config: save_crush_config,
            root_error: "Crush config root must be a JSON object",
            servers_key: "mcp",
            servers_error: "`mcp` in Crush config must be an object",
            filter_backup_servers: crush_backup_servers,
            preserve_server: crush_preserve_server,
            merge_into_backup: merge_crush_servers_into_backup,
        },
    )
}

pub(crate) fn restore_crush_mcp_servers_from_path(
    config_path: &Path,
) -> Result<RestoreMcpServersResult, Box<dyn Error>> {
    restore_json_mcp_servers_from_path(
        config_path,
        JsonRestoreAdapter {
            load_config: load_crush_config,
            save_config: save_crush_config,
            load_backup: load_required_crush_backup,
            backup_servers_key: "mcp",
            missing_backup_servers: missing_crush_backup_servers_error,
            remove_self_servers: remove_crush_self_servers,
            merge_into_target: merge_crush_servers_into_target,
            filter_backup_servers: crush_backup_servers,
        },
    )
}

pub(crate) fn load_crush_servers_for_import_from_path(
    path: &Path,
) -> Result<ImportPlan, Box<dyn Error>> {
    load_json_import_plan_from_path(
        path,
        JsonImportAdapter {
            config_label: "Crush",
            servers_key: "mcp",
            missing_servers: missing_crush_servers_error,
            empty_servers: empty_crush_servers_error,
            server_type_label: "Crush MCP server",
            validate_server: validate_importable_crush_server,
            parse_enabled: parse_crush_import_server_enabled,
            parse_imported_server: crush_imported_server_command,
        },
    )
}

fn missing_crush_servers_error(path: &Path) -> String {
    format!(
        "no `mcp` object found in Crush config {}",
        format_path_for_display(path)
    )
}

fn empty_crush_servers_error(path: &Path) -> String {
    format!(
        "no MCP servers found in Crush config {}",
        format_path_for_display(path)
    )
}

fn missing_crush_backup_servers_error(path: &Path) -> String {
    format!(
        "no `mcp` object found in Crush backup {}",
        format_path_for_display(path)
    )
}

fn parse_crush_import_server_enabled(
    server: &JsonMap<String, JsonValue>,
    name: &str,
) -> Result<bool, Box<dyn Error>> {
    match server.get("disabled") {
        Some(JsonValue::Bool(disabled)) => Ok(!disabled),
        Some(_) => {
            Err(format!("Crush MCP server `{name}` has a non-boolean `disabled` field").into())
        }
        None => Ok(true),
    }
}

fn crush_imported_server_command(
    server: &JsonMap<String, JsonValue>,
    name: &str,
) -> Result<ImportedServerDefinition, Box<dyn Error>> {
    match crush_server_type(server)? {
        "stdio" => {
            let command = server
                .get("command")
                .and_then(JsonValue::as_str)
                .ok_or_else(|| format!("Crush MCP server `{name}` is missing `command`"))?;
            let args = match server.get("args") {
                None => Vec::new(),
                Some(JsonValue::Array(items)) => items
                    .iter()
                    .map(|value| {
                        value.as_str().map(ToOwned::to_owned).ok_or_else(|| {
                            format!("Crush MCP server `{name}` contains a non-string arg")
                        })
                    })
                    .collect::<Result<Vec<_>, _>>()?,
                Some(_) => {
                    return Err(
                        format!("Crush MCP server `{name}` has a non-array `args` field").into(),
                    );
                }
            };
            let env =
                parse_json_string_object(server.get("env"), "env", "Crush MCP server", name)?;
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
                .ok_or_else(|| format!("Crush MCP server `{name}` is missing `url`"))?;
            let headers = parse_json_string_object(
                server.get("headers"),
                "headers",
                "Crush MCP server",
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
            "Crush MCP server `{name}` uses unsupported type `{other}`, only `stdio`, `http`, and `sse` can be imported"
        )
        .into()),
    }
}

fn crush_server_value(server: &StdioServer) -> JsonValue {
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

pub(crate) fn load_crush_config(path: &Path) -> Result<JsonValue, Box<dyn Error>> {
    load_json_object_config(path)
}

fn save_crush_config(path: &Path, config: &JsonValue) -> Result<(), Box<dyn Error>> {
    save_json_object_config(path, config)
}

fn merge_crush_servers_into_backup(
    backup_path: &Path,
    servers: &JsonMap<String, JsonValue>,
) -> Result<(), Box<dyn Error>> {
    merge_json_servers_into_file(
        backup_path,
        load_crush_config,
        save_crush_config,
        "Crush backup root must be a JSON object",
        "mcp",
        "`mcp` in Crush backup must be an object",
        servers,
    )
}

fn crush_backup_servers(servers: &JsonMap<String, JsonValue>) -> JsonMap<String, JsonValue> {
    servers
        .iter()
        .filter(|(_, value)| value.as_object().is_some_and(crush_should_backup_server))
        .map(|(name, value)| (name.clone(), value.clone()))
        .collect()
}

fn crush_should_backup_server(server: &JsonMap<String, JsonValue>) -> bool {
    !server
        .get("url")
        .and_then(JsonValue::as_str)
        .is_some_and(super::super::local::is_unsupported_remote_server_url)
        && crush_server_raw_command(server)
            .is_none_or(|raw_command| !super::super::is_self_server_command(&raw_command))
}

fn crush_preserve_server(server: &JsonMap<String, JsonValue>) -> bool {
    server
        .get("url")
        .and_then(JsonValue::as_str)
        .is_some_and(super::super::local::is_unsupported_remote_server_url)
}

fn merge_crush_servers_into_target(
    config: &mut JsonValue,
    servers: &JsonMap<String, JsonValue>,
) -> Result<(), Box<dyn Error>> {
    merge_json_servers_into_config(
        config,
        "Crush config root must be a JSON object",
        "mcp",
        "`mcp` in Crush config must be an object",
        servers,
    )
}

fn load_required_crush_backup(path: &Path) -> Result<JsonValue, Box<dyn Error>> {
    load_required_json_object_config(path, "Crush backup")
}

fn remove_crush_self_servers(config: &mut JsonValue) -> Result<usize, Box<dyn Error>> {
    remove_json_self_servers(
        config,
        "Crush config root must be a JSON object",
        "mcp",
        "`mcp` in Crush config must be an object",
        crush_server_raw_command,
    )
}

fn validate_importable_crush_server(
    name: &str,
    server: &serde_json::Map<String, serde_json::Value>,
) -> Result<(), Box<dyn Error>> {
    let server_type = crush_server_type(server)?;

    let supported_keys = match server_type {
        "stdio" => ["command", "args", "env", "type", "disabled"].as_slice(),
        "http" | "sse" => ["url", "headers", "type", "disabled"].as_slice(),
        other => {
            return Err(format!(
                "Crush MCP server `{name}` uses unsupported type `{other}`, only `stdio`, `http`, and `sse` can be imported"
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
            "Crush MCP server `{name}` uses unsupported settings {}; only {} can be imported",
            unsupported_keys.join(", "),
            match server_type {
                "stdio" => {
                    "`command`, optional `args`, optional `env`, optional `disabled`, and optional `type`"
                }
                "http" | "sse" => {
                    "`url`, optional `headers`, optional `disabled`, and optional `type`"
                }
                _ => unreachable!(),
            }
        )
        .into())
    }
}

fn crush_server_type(server: &JsonMap<String, JsonValue>) -> Result<&str, Box<dyn Error>> {
    match server.get("type") {
        Some(JsonValue::String(value)) => Ok(value.as_str()),
        Some(_) => Err("Crush MCP server has a non-string `type` field".into()),
        None if server.get("command").is_some() => Ok("stdio"),
        None if server.get("url").is_some() => Ok("http"),
        None => Ok("stdio"),
    }
}
