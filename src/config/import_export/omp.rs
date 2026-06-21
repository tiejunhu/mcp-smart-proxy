use std::collections::BTreeMap;
use std::error::Error;
use std::path::Path;

use serde_json::{Map as JsonMap, Value as JsonValue};

use crate::paths::format_path_for_display;

use super::super::local::parse_json_string_object;
use super::super::provider::omp_config_path;
use super::super::self_server::{inspect_omp_self_server, omp_server_raw_command};
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

pub fn load_omp_servers_for_import() -> Result<(std::path::PathBuf, ImportPlan), Box<dyn Error>> {
    load_provider_import_plan(omp_config_path, load_omp_servers_for_import_from_path)
}

pub fn install_omp_mcp_server() -> Result<InstallMcpServerResult, Box<dyn Error>> {
    install_json_mcp_server(
        omp_config_path()?,
        super::super::OMP_PROVIDER_NAME,
        JsonInstallAdapter {
            load_config: load_omp_config,
            save_config: save_omp_config,
            root_error: "Oh My Pi config root must be a JSON object",
            servers_key: "mcpServers",
            servers_error: "`mcpServers` in Oh My Pi config must be an object",
            inspect_self_server: inspect_omp_self_server,
            build_server_value: omp_server_value,
        },
    )
}

pub fn replace_omp_mcp_servers() -> Result<ReplaceMcpServersResult, Box<dyn Error>> {
    let config_path = omp_config_path()?;
    replace_omp_mcp_servers_from_path(&config_path)
}

pub fn restore_omp_mcp_servers() -> Result<RestoreMcpServersResult, Box<dyn Error>> {
    let config_path = omp_config_path()?;
    restore_omp_mcp_servers_from_path(&config_path)
}

pub(crate) fn replace_omp_mcp_servers_from_path(
    config_path: &Path,
) -> Result<ReplaceMcpServersResult, Box<dyn Error>> {
    replace_json_mcp_servers_from_path(
        config_path,
        JsonReplaceAdapter {
            load_config: load_omp_config,
            save_config: save_omp_config,
            root_error: "Oh My Pi config root must be a JSON object",
            servers_key: "mcpServers",
            servers_error: "`mcpServers` in Oh My Pi config must be an object",
            filter_backup_servers: omp_backup_servers,
            preserve_server: omp_preserve_server,
            merge_into_backup: merge_omp_servers_into_backup,
        },
    )
}

pub(crate) fn restore_omp_mcp_servers_from_path(
    config_path: &Path,
) -> Result<RestoreMcpServersResult, Box<dyn Error>> {
    restore_json_mcp_servers_from_path(
        config_path,
        JsonRestoreAdapter {
            load_config: load_omp_config,
            save_config: save_omp_config,
            load_backup: load_required_omp_backup,
            backup_servers_key: "mcpServers",
            missing_backup_servers: missing_omp_backup_servers_error,
            remove_self_servers: remove_omp_self_servers,
            merge_into_target: merge_omp_servers_into_target,
            filter_backup_servers: omp_backup_servers,
        },
    )
}

pub(crate) fn load_omp_servers_for_import_from_path(
    path: &Path,
) -> Result<ImportPlan, Box<dyn Error>> {
    load_json_import_plan_from_path(
        path,
        JsonImportAdapter {
            config_label: "Oh My Pi",
            servers_key: "mcpServers",
            missing_servers: missing_omp_servers_error,
            empty_servers: empty_omp_servers_error,
            server_type_label: "Oh My Pi MCP server",
            validate_server: validate_importable_omp_server,
            parse_enabled: parse_omp_import_server_enabled,
            parse_imported_server: omp_imported_server_command,
        },
    )
}

fn missing_omp_servers_error(path: &Path) -> String {
    format!(
        "no `mcpServers` object found in Oh My Pi config {}",
        format_path_for_display(path)
    )
}

fn empty_omp_servers_error(path: &Path) -> String {
    format!(
        "no MCP servers found in Oh My Pi config {}",
        format_path_for_display(path)
    )
}

fn missing_omp_backup_servers_error(path: &Path) -> String {
    format!(
        "no `mcpServers` object found in Oh My Pi backup {}",
        format_path_for_display(path)
    )
}

fn parse_omp_import_server_enabled(
    server: &JsonMap<String, JsonValue>,
    name: &str,
) -> Result<bool, Box<dyn Error>> {
    match server.get("enabled") {
        Some(JsonValue::Bool(enabled)) => Ok(*enabled),
        Some(_) => {
            Err(format!("Oh My Pi MCP server `{name}` has a non-boolean `enabled` field").into())
        }
        None => Ok(true),
    }
}

fn omp_imported_server_command(
    server: &JsonMap<String, JsonValue>,
    name: &str,
) -> Result<ImportedServerDefinition, Box<dyn Error>> {
    match omp_server_type(server)? {
        "stdio" => {
            let command = server
                .get("command")
                .and_then(JsonValue::as_str)
                .ok_or_else(|| format!("Oh My Pi MCP server `{name}` is missing `command`"))?;
            let args = match server.get("args") {
                None => Vec::new(),
                Some(JsonValue::Array(items)) => items
                    .iter()
                    .map(|value| {
                        value.as_str().map(ToOwned::to_owned).ok_or_else(|| {
                            format!("Oh My Pi MCP server `{name}` contains a non-string arg")
                        })
                    })
                    .collect::<Result<Vec<_>, _>>()?,
                Some(_) => {
                    return Err(
                        format!("Oh My Pi MCP server `{name}` has a non-array `args` field").into(),
                    );
                }
            };
            let env =
                parse_json_string_object(server.get("env"), "env", "Oh My Pi MCP server", name)?;
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
                .ok_or_else(|| format!("Oh My Pi MCP server `{name}` is missing `url`"))?;
            let headers = parse_json_string_object(
                server.get("headers"),
                "headers",
                "Oh My Pi MCP server",
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
            "Oh My Pi MCP server `{name}` uses unsupported type `{other}`, only `stdio`, `http`, and `sse` can be imported"
        )
        .into()),
    }
}

fn omp_server_value(server: &StdioServer) -> JsonValue {
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

pub(crate) fn load_omp_config(path: &Path) -> Result<JsonValue, Box<dyn Error>> {
    load_json_object_config(path)
}

fn save_omp_config(path: &Path, config: &JsonValue) -> Result<(), Box<dyn Error>> {
    save_json_object_config(path, config)
}

fn merge_omp_servers_into_backup(
    backup_path: &Path,
    servers: &JsonMap<String, JsonValue>,
) -> Result<(), Box<dyn Error>> {
    merge_json_servers_into_file(
        backup_path,
        load_omp_config,
        save_omp_config,
        "Oh My Pi backup root must be a JSON object",
        "mcpServers",
        "`mcpServers` in Oh My Pi backup must be an object",
        servers,
    )
}

fn omp_backup_servers(servers: &JsonMap<String, JsonValue>) -> JsonMap<String, JsonValue> {
    servers
        .iter()
        .filter(|(_, value)| value.as_object().is_some_and(omp_should_backup_server))
        .map(|(name, value)| (name.clone(), value.clone()))
        .collect()
}

fn omp_should_backup_server(server: &JsonMap<String, JsonValue>) -> bool {
    !server
        .get("url")
        .and_then(JsonValue::as_str)
        .is_some_and(super::super::local::is_unsupported_remote_server_url)
        && omp_server_raw_command(server)
            .is_none_or(|raw_command| !super::super::is_self_server_command(&raw_command))
}

fn omp_preserve_server(server: &JsonMap<String, JsonValue>) -> bool {
    server
        .get("url")
        .and_then(JsonValue::as_str)
        .is_some_and(super::super::local::is_unsupported_remote_server_url)
}

fn merge_omp_servers_into_target(
    config: &mut JsonValue,
    servers: &JsonMap<String, JsonValue>,
) -> Result<(), Box<dyn Error>> {
    merge_json_servers_into_config(
        config,
        "Oh My Pi config root must be a JSON object",
        "mcpServers",
        "`mcpServers` in Oh My Pi config must be an object",
        servers,
    )
}

fn load_required_omp_backup(path: &Path) -> Result<JsonValue, Box<dyn Error>> {
    load_required_json_object_config(path, "Oh My Pi backup")
}

fn remove_omp_self_servers(config: &mut JsonValue) -> Result<usize, Box<dyn Error>> {
    remove_json_self_servers(
        config,
        "Oh My Pi config root must be a JSON object",
        "mcpServers",
        "`mcpServers` in Oh My Pi config must be an object",
        omp_server_raw_command,
    )
}

fn validate_importable_omp_server(
    name: &str,
    server: &serde_json::Map<String, serde_json::Value>,
) -> Result<(), Box<dyn Error>> {
    let server_type = omp_server_type(server)?;

    let supported_keys = match server_type {
        "stdio" => ["command", "args", "env", "type", "enabled"].as_slice(),
        "http" | "sse" => ["url", "headers", "type", "enabled"].as_slice(),
        other => {
            return Err(format!(
                "Oh My Pi MCP server `{name}` uses unsupported type `{other}`, only `stdio`, `http`, and `sse` can be imported"
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
            "Oh My Pi MCP server `{name}` uses unsupported settings {}; only {} can be imported",
            unsupported_keys.join(", "),
            match server_type {
                "stdio" => {
                    "`command`, optional `args`, optional `env`, optional `enabled`, and optional `type`"
                }
                "http" | "sse" => {
                    "`url`, optional `headers`, optional `enabled`, and optional `type`"
                }
                _ => unreachable!(),
            }
        )
        .into())
    }
}

fn omp_server_type(server: &JsonMap<String, JsonValue>) -> Result<&str, Box<dyn Error>> {
    match server.get("type") {
        Some(JsonValue::String(value)) => Ok(value.as_str()),
        Some(_) => Err("Oh My Pi MCP server has a non-string `type` field".into()),
        None if server.get("command").is_some() => Ok("stdio"),
        None if server.get("url").is_some() => Ok("http"),
        None => Ok("stdio"),
    }
}
