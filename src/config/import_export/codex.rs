use std::collections::BTreeMap;
use std::error::Error;
use std::path::Path;

use toml::{Table, Value};

use crate::paths::format_path_for_display;

use super::super::local::{merge_env_vars, parse_toml_string_array, parse_toml_string_table};
use super::super::provider::codex_config_path;
use super::super::self_server::{codex_server_raw_command, inspect_codex_self_server};
use super::super::{
    ImportPlan, ImportedServerDefinition, InstallMcpServerResult, ReplaceMcpServersResult,
    RestoreMcpServersResult, StdioServer,
};
use super::common::{collect_remote_header_env_vars, load_provider_import_plan};
use super::toml_support::{
    TomlImportAdapter, TomlRestoreAdapter, install_toml_mcp_server, load_required_toml_config,
    load_toml_import_plan_from_path, merge_toml_servers_into_config, merge_toml_servers_into_file,
    remove_toml_self_servers, replace_toml_mcp_servers_from_path,
    restore_toml_mcp_servers_from_path,
};

pub fn load_codex_servers_for_import() -> Result<(std::path::PathBuf, ImportPlan), Box<dyn Error>> {
    load_provider_import_plan(codex_config_path, load_codex_servers_for_import_from_path)
}

pub fn install_codex_mcp_server() -> Result<InstallMcpServerResult, Box<dyn Error>> {
    install_toml_mcp_server(
        codex_config_path()?,
        "mcp_servers",
        "`mcp_servers` in Codex config must be a table",
        super::super::CODEX_PROVIDER_NAME,
        inspect_codex_self_server,
        codex_server_value,
    )
}

pub fn replace_codex_mcp_servers() -> Result<ReplaceMcpServersResult, Box<dyn Error>> {
    let config_path = codex_config_path()?;
    replace_codex_mcp_servers_from_path(&config_path)
}

pub fn restore_codex_mcp_servers() -> Result<RestoreMcpServersResult, Box<dyn Error>> {
    let config_path = codex_config_path()?;
    restore_codex_mcp_servers_from_path(&config_path)
}

pub(crate) fn replace_codex_mcp_servers_from_path(
    config_path: &Path,
) -> Result<ReplaceMcpServersResult, Box<dyn Error>> {
    replace_toml_mcp_servers_from_path(
        config_path,
        "mcp_servers",
        "`mcp_servers` in Codex config must be a table",
        codex_backup_servers,
        merge_codex_servers_into_backup,
    )
}

pub(crate) fn restore_codex_mcp_servers_from_path(
    config_path: &Path,
) -> Result<RestoreMcpServersResult, Box<dyn Error>> {
    restore_toml_mcp_servers_from_path(
        config_path,
        TomlRestoreAdapter {
            load_backup: load_required_codex_backup,
            backup_servers_key: "mcp_servers",
            missing_backup_servers: missing_codex_backup_servers_error,
            remove_self_servers: remove_codex_self_servers,
            merge_into_target: merge_codex_servers_into_target,
            filter_backup_servers: codex_backup_servers,
        },
    )
}

pub(crate) fn load_codex_servers_for_import_from_path(
    path: &Path,
) -> Result<ImportPlan, Box<dyn Error>> {
    load_toml_import_plan_from_path(
        path,
        TomlImportAdapter {
            config_label: "Codex",
            servers_key: "mcp_servers",
            missing_servers: missing_codex_servers_error,
            empty_servers: empty_codex_servers_error,
            server_type_label: "Codex MCP server",
            validate_server: validate_importable_codex_server,
            parse_enabled: parse_codex_import_server_enabled,
            parse_imported_server: codex_imported_server_command,
        },
    )
}

fn missing_codex_servers_error(path: &Path) -> String {
    format!(
        "no `mcp_servers` table found in Codex config {}",
        format_path_for_display(path)
    )
}

fn empty_codex_servers_error(path: &Path) -> String {
    format!(
        "no MCP servers found in Codex config {}",
        format_path_for_display(path)
    )
}

fn missing_codex_backup_servers_error(path: &Path) -> String {
    format!(
        "no `mcp_servers` table found in Codex backup {}",
        format_path_for_display(path)
    )
}

fn parse_codex_import_server_enabled(server: &Table, name: &str) -> Result<bool, Box<dyn Error>> {
    match server.get("enabled") {
        Some(Value::Boolean(enabled)) => Ok(*enabled),
        Some(_) => {
            Err(format!("Codex MCP server `{name}` has a non-boolean `enabled` field").into())
        }
        None => Ok(true),
    }
}

fn codex_imported_server_command(
    server: &Table,
    name: &str,
) -> Result<ImportedServerDefinition, Box<dyn Error>> {
    let env = parse_toml_string_table(server.get("env"), "env", "Codex MCP server", name)?;
    let mut env_vars =
        parse_toml_string_array(server.get("env_vars"), "env_vars", "Codex MCP server", name)?;

    match (
        server.get("url"),
        server.get("command").and_then(Value::as_str),
    ) {
        (Some(_), Some(_)) => {
            Err(format!("Codex MCP server `{name}` cannot define both `url` and `command`").into())
        }
        (Some(Value::String(url)), None) => {
            let mut headers = parse_toml_string_table(
                server.get("http_headers"),
                "http_headers",
                "Codex MCP server",
                name,
            )?;
            let env_http_headers = parse_toml_string_table(
                server.get("env_http_headers"),
                "env_http_headers",
                "Codex MCP server",
                name,
            )?;
            for (header_name, env_var_name) in env_http_headers {
                headers.insert(header_name, format!("{{env:{env_var_name}}}"));
            }
            if let Some(Value::String(bearer_token_env_var)) = server.get("bearer_token_env_var") {
                headers.insert(
                    "Authorization".to_string(),
                    format!("Bearer {{env:{bearer_token_env_var}}}"),
                );
            } else if server.get("bearer_token_env_var").is_some() {
                return Err(format!(
                    "Codex MCP server `{name}` has a non-string `bearer_token_env_var` field"
                )
                .into());
            }
            merge_env_vars(&mut env_vars, collect_remote_header_env_vars(&headers));
            Ok(ImportedServerDefinition {
                command: Vec::new(),
                url: Some(url.to_string()),
                headers,
                env,
                env_vars,
            })
        }
        (Some(_), None) => {
            Err(format!("Codex MCP server `{name}` has a non-string `url` field").into())
        }
        (None, Some(command)) => {
            let args = match server.get("args") {
                None => Vec::new(),
                Some(Value::Array(items)) => items
                    .iter()
                    .map(|value| {
                        value.as_str().map(ToOwned::to_owned).ok_or_else(|| {
                            format!("Codex MCP server `{name}` contains a non-string arg")
                        })
                    })
                    .collect::<Result<Vec<_>, _>>()?,
                Some(_) => {
                    return Err(
                        format!("Codex MCP server `{name}` has a non-array `args` field").into(),
                    );
                }
            };
            let mut raw_command = vec![command.to_string()];
            raw_command.extend(args);
            Ok(ImportedServerDefinition {
                command: raw_command,
                url: None,
                headers: BTreeMap::new(),
                env,
                env_vars,
            })
        }
        (None, None) => {
            Err(format!("Codex MCP server `{name}` is missing `command` or `url`").into())
        }
    }
}

fn codex_server_value(server: &StdioServer) -> Value {
    let mut server_table = Table::new();
    server_table.insert("command".to_string(), Value::String(server.command.clone()));
    server_table.insert(
        "args".to_string(),
        Value::Array(server.args.iter().cloned().map(Value::String).collect()),
    );
    Value::Table(server_table)
}

fn merge_codex_servers_into_backup(
    backup_path: &Path,
    servers: &Table,
) -> Result<(), Box<dyn Error>> {
    merge_toml_servers_into_file(
        backup_path,
        "mcp_servers",
        "`mcp_servers` in Codex backup must be a table",
        servers,
    )
}

fn codex_backup_servers(servers: &Table) -> Table {
    servers
        .iter()
        .filter(|(_, value)| {
            value
                .as_table()
                .and_then(codex_server_raw_command)
                .is_none_or(|raw_command| !super::super::is_self_server_command(&raw_command))
        })
        .map(|(name, value)| (name.clone(), value.clone()))
        .collect()
}

fn merge_codex_servers_into_target(
    config: &mut Table,
    servers: &Table,
) -> Result<(), Box<dyn Error>> {
    merge_toml_servers_into_config(
        config,
        "mcp_servers",
        "`mcp_servers` in Codex config must be a table",
        servers,
    )
}

fn load_required_codex_backup(path: &Path) -> Result<Table, Box<dyn Error>> {
    load_required_toml_config(path, "Codex backup")
}

fn remove_codex_self_servers(config: &mut Table) -> Result<usize, Box<dyn Error>> {
    remove_toml_self_servers(
        config,
        "mcp_servers",
        "`mcp_servers` in Codex config must be a table",
        codex_server_raw_command,
    )
}

fn validate_importable_codex_server(name: &str, server: &Table) -> Result<(), Box<dyn Error>> {
    let unsupported_keys = server
        .keys()
        .filter(|key| {
            !matches!(
                key.as_str(),
                "command"
                    | "args"
                    | "enabled"
                    | "env"
                    | "env_vars"
                    | "url"
                    | "http_headers"
                    | "bearer_token_env_var"
                    | "env_http_headers"
            )
        })
        .map(|key| format!("`{key}`"))
        .collect::<Vec<_>>();

    if unsupported_keys.is_empty() {
        return Ok(());
    }

    Err(format!(
        "Codex MCP server `{name}` uses unsupported settings {}; only `command`, `args`, optional `enabled`, `env`, `env_vars`, or remote `url` with optional `http_headers`, `bearer_token_env_var`, and `env_http_headers` can be imported",
        unsupported_keys.join(", ")
    )
    .into())
}
