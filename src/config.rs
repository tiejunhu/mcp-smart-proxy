use std::collections::BTreeMap;
use std::env;
use std::error::Error;
use std::fs;
use std::path::{Path, PathBuf};

use serde_json::{Map as JsonMap, Value as JsonValue};
use toml::{Table, Value};

mod self_server;

use crate::env_template::collect_env_var_names;
use crate::fs_util::write_file_atomically;
use crate::paths::{
    cache_file_path, expand_tilde, format_path_for_display, sanitize_name, sibling_backup_path,
};
use crate::types::{
    CachedTools, ClaudeRuntimeConfig, CodexRuntimeConfig, ConfiguredServer, ConfiguredTransport,
    ModelProviderConfig, OpencodeRuntimeConfig,
};

pub(crate) use self_server::is_self_server_command;

use self_server::{
    claude_server_raw_command, codex_server_raw_command, inspect_claude_self_server,
    inspect_codex_self_server, inspect_opencode_self_server, next_available_server_name,
    opencode_server_raw_command, proxy_stdio_server,
};

const DEFAULT_MODEL: &str = "gpt-5.2";
const DEFAULT_OPENCODE_MODEL: &str = "openai/gpt-5.2";
const DEFAULT_CLAUDE_MODEL: &str = "sonnet";
const DEFAULT_CODEX_CONFIG_PATH: &str = "~/.codex/config.toml";
const DEFAULT_OPENCODE_CONFIG_PATH: &str = "~/.config/opencode/opencode.json";
const DEFAULT_CLAUDE_CONFIG_PATH: &str = "~/.claude.json";
const CODEX_HOME_ENV: &str = "CODEX_HOME";
const CODEX_PROVIDER_NAME: &str = "codex";
const OPENCODE_PROVIDER_NAME: &str = "opencode";
const CLAUDE_PROVIDER_NAME: &str = "claude";

#[derive(Debug, Clone, PartialEq, Eq)]
struct StdioServer {
    command: String,
    args: Vec<String>,
}

impl StdioServer {
    fn from_command(command: Vec<String>) -> Result<Self, Box<dyn Error>> {
        let mut parts = command.into_iter();
        let executable = parts
            .next()
            .ok_or_else(|| "missing stdio server command".to_string())?;

        Ok(Self {
            command: executable,
            args: parts.collect(),
        })
    }

    fn raw_command(&self) -> Vec<String> {
        let mut command = vec![self.command.clone()];
        command.extend(self.args.iter().cloned());
        command
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ImportedServerDefinition {
    command: Vec<String>,
    url: Option<String>,
    headers: BTreeMap<String, String>,
    env: BTreeMap<String, String>,
    env_vars: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImportableServer {
    pub name: String,
    pub command: Vec<String>,
    pub url: Option<String>,
    pub headers: BTreeMap<String, String>,
    pub enabled: bool,
    pub env: BTreeMap<String, String>,
    pub env_vars: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ListedServer {
    pub name: String,
    pub command: String,
    pub args: Vec<String>,
    pub enabled: bool,
    pub last_updated_at: Option<u128>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImportPlan {
    pub servers: Vec<ImportableServer>,
    pub skipped_self_servers: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemovedServer {
    pub name: String,
    pub cache_path: PathBuf,
    pub cache_deleted: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SetServerEnabledResult {
    pub name: String,
    pub enabled: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServerConfigSnapshot {
    pub name: String,
    pub transport: String,
    pub enabled: bool,
    pub command: Option<String>,
    pub args: Vec<String>,
    pub url: Option<String>,
    pub headers: BTreeMap<String, String>,
    pub env: BTreeMap<String, String>,
    pub env_vars: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct UpdateServerConfig {
    pub transport: Option<String>,
    pub command: Option<String>,
    pub clear_args: bool,
    pub add_args: Vec<String>,
    pub url: Option<String>,
    pub enabled: Option<bool>,
    pub clear_headers: bool,
    pub set_headers: BTreeMap<String, String>,
    pub unset_headers: Vec<String>,
    pub clear_env: bool,
    pub set_env: BTreeMap<String, String>,
    pub unset_env: Vec<String>,
    pub clear_env_vars: bool,
    pub add_env_vars: Vec<String>,
    pub unset_env_vars: Vec<String>,
}

impl UpdateServerConfig {
    pub fn has_changes(&self) -> bool {
        self.transport.is_some()
            || self.command.is_some()
            || self.clear_args
            || !self.add_args.is_empty()
            || self.url.is_some()
            || self.enabled.is_some()
            || self.clear_headers
            || !self.set_headers.is_empty()
            || !self.unset_headers.is_empty()
            || self.clear_env
            || !self.set_env.is_empty()
            || !self.unset_env.is_empty()
            || self.clear_env_vars
            || !self.add_env_vars.is_empty()
            || !self.unset_env_vars.is_empty()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InstallMcpServerStatus {
    AlreadyInstalled,
    Updated,
    Installed,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstallMcpServerResult {
    pub name: String,
    pub config_path: PathBuf,
    pub status: InstallMcpServerStatus,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReplaceMcpServersResult {
    pub config_path: PathBuf,
    pub backup_path: PathBuf,
    pub backed_up_server_count: usize,
    pub removed_server_count: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RestoreMcpServersResult {
    pub config_path: PathBuf,
    pub backup_path: PathBuf,
    pub removed_self_server_count: usize,
    pub restored_server_count: usize,
}

pub fn add_server(
    config_path: &Path,
    name: &str,
    raw_command: Vec<String>,
) -> Result<String, Box<dyn Error>> {
    if raw_command.len() == 1 && looks_like_url(&raw_command[0]) {
        return save_remote_server(
            config_path,
            name,
            raw_command[0].clone(),
            BTreeMap::new(),
            true,
            BTreeMap::new(),
            Vec::new(),
        );
    }

    save_stdio_server(
        config_path,
        name,
        raw_command,
        true,
        BTreeMap::new(),
        Vec::new(),
    )
}

pub fn import_server(
    config_path: &Path,
    server: &ImportableServer,
) -> Result<String, Box<dyn Error>> {
    if let Some(url) = &server.url {
        return save_remote_server(
            config_path,
            &server.name,
            url.clone(),
            server.headers.clone(),
            server.enabled,
            server.env.clone(),
            server.env_vars.clone(),
        );
    }

    save_stdio_server(
        config_path,
        &server.name,
        server.command.clone(),
        server.enabled,
        server.env.clone(),
        server.env_vars.clone(),
    )
}

fn save_stdio_server(
    config_path: &Path,
    name: &str,
    raw_command: Vec<String>,
    enabled: bool,
    env: BTreeMap<String, String>,
    env_vars: Vec<String>,
) -> Result<String, Box<dyn Error>> {
    if is_self_server_command(&raw_command) {
        return Err("cannot add `msp mcp` as a managed server".into());
    }
    let server = StdioServer::from_command(raw_command)?;

    let mut config = load_config_table(config_path)?;
    let name = sanitize_name(name);
    if name.is_empty() {
        return Err("server name must contain at least one ASCII letter or digit".into());
    }
    if has_server_name(&config, &name) {
        return Err(format!("server `{name}` already exists").into());
    }

    insert_server(&mut config, &name, &server, enabled, env, env_vars)?;
    save_config_table(config_path, &config)?;

    Ok(name)
}

fn save_remote_server(
    config_path: &Path,
    name: &str,
    url: String,
    headers: BTreeMap<String, String>,
    enabled: bool,
    env: BTreeMap<String, String>,
    env_vars: Vec<String>,
) -> Result<String, Box<dyn Error>> {
    let mut config = load_config_table(config_path)?;
    let name = sanitize_name(name);
    if name.is_empty() {
        return Err("server name must contain at least one ASCII letter or digit".into());
    }
    if has_server_name(&config, &name) {
        return Err(format!("server `{name}` already exists").into());
    }

    insert_remote_server(&mut config, &name, &url, headers, enabled, env, env_vars)?;
    save_config_table(config_path, &config)?;

    Ok(name)
}

pub fn list_servers(config_path: &Path) -> Result<Vec<ListedServer>, Box<dyn Error>> {
    let config = load_config_table(config_path)?;
    let Some(servers) = config.get("servers").and_then(Value::as_table) else {
        return Ok(Vec::new());
    };

    let mut names = servers.keys().cloned().collect::<Vec<_>>();
    names.sort();

    names.into_iter()
        .map(|name| {
            let server = servers[&name]
                .as_table()
                .ok_or_else(|| format!("server `{name}` must be a table"))?;

            let transport = resolved_server_transport(server, &name)?;
            let (command, args) = match transport {
                "stdio" => {
                    let command = server
                        .get("command")
                        .and_then(Value::as_str)
                        .ok_or_else(|| format!("server `{name}` is missing `command`"))?
                        .to_string();

                    let args = server
                        .get("args")
                        .and_then(Value::as_array)
                        .map(|items| {
                            items
                                .iter()
                                .map(|value| {
                                    value.as_str().map(ToOwned::to_owned).ok_or_else(|| {
                                        format!("server `{name}` contains a non-string arg")
                                    })
                                })
                                .collect::<Result<Vec<_>, _>>()
                        })
                        .transpose()?
                        .unwrap_or_default();

                    (command, args)
                }
                "remote" => (
                    parse_remote_server_url(server, &name)?.to_string(),
                    Vec::new(),
                ),
                other => {
                    return Err(format!(
                        "server `{name}` uses unsupported transport `{other}`, only `stdio` and `remote` are supported"
                    )
                    .into())
                }
            };

            let enabled = parse_server_enabled(server, &name)?;
            let last_updated_at = read_cached_tools_timestamp(&name);

            Ok(ListedServer {
                name,
                command,
                args,
                enabled,
                last_updated_at,
            })
        })
        .collect()
}

fn read_cached_tools_timestamp(server_name: &str) -> Option<u128> {
    let cache_path = cache_file_path(server_name).ok()?;
    let contents = fs::read_to_string(cache_path).ok()?;
    let cached: CachedTools = serde_json::from_str(&contents).ok()?;
    Some(cached.fetched_at_epoch_ms)
}

pub fn remove_server(
    config_path: &Path,
    requested_name: &str,
) -> Result<RemovedServer, Box<dyn Error>> {
    let mut config = load_config_table(config_path)?;
    let remove_servers_table = {
        let servers = config
            .get_mut("servers")
            .and_then(Value::as_table_mut)
            .ok_or_else(|| "no `servers` table found in config".to_string())?;

        let resolved_name = resolve_server_name(servers, requested_name)
            .ok_or_else(|| format!("server `{requested_name}` not found"))?;
        servers.remove(&resolved_name);

        (resolved_name, servers.is_empty())
    };
    let (resolved_name, remove_servers_table) = remove_servers_table;

    if remove_servers_table {
        config.remove("servers");
    }
    save_config_table(config_path, &config)?;

    let cache_path = cache_file_path(&resolved_name)?;
    let cache_deleted = if cache_path.exists() {
        fs::remove_file(&cache_path)?;
        true
    } else {
        false
    };

    Ok(RemovedServer {
        name: resolved_name,
        cache_path,
        cache_deleted,
    })
}

pub fn set_server_enabled(
    config_path: &Path,
    requested_name: &str,
    enabled: bool,
) -> Result<SetServerEnabledResult, Box<dyn Error>> {
    let mut config = load_config_table(config_path)?;
    let servers = config
        .get_mut("servers")
        .and_then(Value::as_table_mut)
        .ok_or_else(|| "no `servers` table found in config".to_string())?;

    let resolved_name = resolve_server_name(servers, requested_name)
        .ok_or_else(|| format!("server `{requested_name}` not found"))?;
    let server = servers
        .get_mut(&resolved_name)
        .and_then(Value::as_table_mut)
        .ok_or_else(|| format!("server `{resolved_name}` must be a table"))?;

    server.insert("enabled".to_string(), Value::Boolean(enabled));
    save_config_table(config_path, &config)?;

    Ok(SetServerEnabledResult {
        name: resolved_name,
        enabled,
    })
}

pub fn load_config_table(path: &Path) -> Result<Table, Box<dyn Error>> {
    if !path.exists() {
        return Ok(Table::new());
    }

    let contents = fs::read_to_string(path)?;
    let table = toml::from_str(&contents)?;
    Ok(table)
}

pub fn save_config_table(path: &Path, config: &Table) -> Result<(), Box<dyn Error>> {
    let contents = toml::to_string_pretty(config)?;
    write_file_atomically(path, contents.as_bytes())?;
    Ok(())
}

pub fn configured_server(
    config: &Table,
    requested_name: &str,
) -> Result<(String, ConfiguredServer), Box<dyn Error>> {
    let servers = config
        .get("servers")
        .and_then(Value::as_table)
        .ok_or_else(|| "no `servers` table found in config".to_string())?;

    let resolved_name = if servers.contains_key(requested_name) {
        requested_name.to_string()
    } else {
        let normalized = sanitize_name(requested_name);
        if servers.contains_key(&normalized) {
            normalized
        } else {
            return Err(format!("server `{requested_name}` not found").into());
        }
    };

    let server = servers[&resolved_name]
        .as_table()
        .ok_or_else(|| format!("server `{resolved_name}` must be a table"))?;

    let transport = resolved_server_transport(server, &resolved_name)?;
    let configured_transport = match transport {
        "stdio" => {
            let command = server
                .get("command")
                .and_then(Value::as_str)
                .ok_or_else(|| format!("server `{resolved_name}` is missing `command`"))?
                .to_string();

            let args = server
                .get("args")
                .and_then(Value::as_array)
                .map(|items| {
                    items
                        .iter()
                        .map(|value| {
                            value.as_str().map(ToOwned::to_owned).ok_or_else(|| {
                                format!("server `{resolved_name}` contains a non-string arg")
                            })
                        })
                        .collect::<Result<Vec<_>, _>>()
                })
                .transpose()?
                .unwrap_or_default();

            ConfiguredTransport::Stdio { command, args }
        }
        "remote" => {
            let url = parse_remote_server_url(server, &resolved_name)?;
            let headers =
                parse_toml_string_table(server.get("headers"), "headers", "server", &resolved_name)?;
            ConfiguredTransport::Remote {
                url: url.to_string(),
                headers,
            }
        }
        other => {
            return Err(format!(
                "server `{resolved_name}` uses unsupported transport `{other}`, only `stdio` and `remote` are supported"
            )
            .into())
        }
    };

    let env = parse_toml_string_table(server.get("env"), "env", "server", &resolved_name)?;
    let env_vars =
        parse_toml_string_array(server.get("env_vars"), "env_vars", "server", &resolved_name)?;

    Ok((
        resolved_name,
        ConfiguredServer {
            transport: configured_transport,
            env,
            env_vars,
        },
    ))
}

pub fn server_is_enabled(config: &Table, requested_name: &str) -> Result<bool, Box<dyn Error>> {
    let servers = config
        .get("servers")
        .and_then(Value::as_table)
        .ok_or_else(|| "no `servers` table found in config".to_string())?;
    let resolved_name = resolve_server_name(servers, requested_name)
        .ok_or_else(|| format!("server `{requested_name}` not found"))?;
    let server = servers[&resolved_name]
        .as_table()
        .ok_or_else(|| format!("server `{resolved_name}` must be a table"))?;

    parse_server_enabled(server, &resolved_name)
}

pub fn load_server_config(
    config_path: &Path,
    requested_name: &str,
) -> Result<ServerConfigSnapshot, Box<dyn Error>> {
    let config = load_config_table(config_path)?;
    let (resolved_name, server) = resolved_server_table(&config, requested_name)?;
    server_config_snapshot(&resolved_name, server)
}

pub fn update_server_config(
    config_path: &Path,
    requested_name: &str,
    update: &UpdateServerConfig,
) -> Result<ServerConfigSnapshot, Box<dyn Error>> {
    let mut config = load_config_table(config_path)?;
    let resolved_name = {
        let (resolved_name, server) = resolved_server_table_mut(&mut config, requested_name)?;
        let current_transport = resolved_server_transport(server, &resolved_name)?.to_string();
        let next_transport = if let Some(url) = &update.url {
            if !looks_like_url(url) {
                return Err(format!(
                    "server `{resolved_name}` has an invalid remote `url` value `{url}`"
                )
                .into());
            }
            "remote".to_string()
        } else if current_transport == "remote" && update.command.is_some() {
            "stdio".to_string()
        } else if let Some(transport) = &update.transport {
            transport.clone()
        } else {
            current_transport.clone()
        };

        match next_transport.as_str() {
            "stdio" | "remote" => {}
            other => {
                return Err(format!(
                    "server `{resolved_name}` uses unsupported transport `{other}`, only `stdio` and `remote` are supported"
                )
                .into())
            }
        }

        if next_transport == "remote"
            && (update.command.is_some() || update.clear_args || !update.add_args.is_empty())
        {
            return Err(format!(
                "server `{resolved_name}` uses remote transport; update it with `--url` and header flags instead of `--cmd` or `--arg`"
            )
            .into());
        }

        if next_transport == "stdio"
            && current_transport == "remote"
            && update.command.is_none()
            && update.transport.as_deref() == Some("stdio")
        {
            return Err(format!(
                "server `{resolved_name}` uses remote transport; pass `--cmd` when converting it to stdio"
            )
            .into());
        }

        match update.transport.as_deref() {
            Some("stdio") | Some("remote") => {
                server.insert(
                    "transport".to_string(),
                    Value::String(next_transport.clone()),
                );
            }
            Some(_) => unreachable!("unsupported transport already rejected"),
            None if next_transport != current_transport => {
                server.remove("transport");
            }
            None => {}
        }

        if next_transport == "stdio" {
            server.remove("url");
            server.remove("headers");

            if let Some(command) = &update.command {
                server.insert("command".to_string(), Value::String(command.clone()));
            } else if current_transport == "remote" {
                return Err(format!(
                    "server `{resolved_name}` uses remote transport; pass `--cmd` when converting it to stdio"
                )
                .into());
            }

            if current_transport == "remote" || update.clear_args || !update.add_args.is_empty() {
                let mut args = if update.clear_args || current_transport == "remote" {
                    Vec::new()
                } else {
                    parse_toml_string_array(server.get("args"), "args", "server", &resolved_name)?
                };
                args.extend(update.add_args.iter().cloned());
                server.insert(
                    "args".to_string(),
                    Value::Array(args.into_iter().map(Value::String).collect()),
                );
            }
        } else {
            server.remove("command");
            server.remove("args");

            let url = match &update.url {
                Some(url) => url.clone(),
                None => parse_remote_server_url(server, &resolved_name)?.to_string(),
            };
            server.insert("url".to_string(), Value::String(url));

            if update.clear_headers
                || !update.set_headers.is_empty()
                || !update.unset_headers.is_empty()
            {
                let mut headers = if update.clear_headers || current_transport != "remote" {
                    BTreeMap::new()
                } else {
                    parse_toml_string_table(
                        server.get("headers"),
                        "headers",
                        "server",
                        &resolved_name,
                    )?
                };
                for key in &update.unset_headers {
                    headers.remove(key);
                }
                for (key, value) in &update.set_headers {
                    headers.insert(key.clone(), value.clone());
                }
                if headers.is_empty() {
                    server.remove("headers");
                } else {
                    server.insert(
                        "headers".to_string(),
                        Value::Table(
                            headers
                                .into_iter()
                                .map(|(key, value)| (key, Value::String(value)))
                                .collect(),
                        ),
                    );
                }
            } else if current_transport != "remote" {
                server.remove("headers");
            }
        }

        if let Some(enabled) = update.enabled {
            server.insert("enabled".to_string(), Value::Boolean(enabled));
        }

        if update.clear_env || !update.set_env.is_empty() || !update.unset_env.is_empty() {
            let mut env = if update.clear_env {
                BTreeMap::new()
            } else {
                parse_toml_string_table(server.get("env"), "env", "server", &resolved_name)?
            };
            for key in &update.unset_env {
                env.remove(key);
            }
            for (key, value) in &update.set_env {
                env.insert(key.clone(), value.clone());
            }
            if env.is_empty() {
                server.remove("env");
            } else {
                server.insert(
                    "env".to_string(),
                    Value::Table(
                        env.into_iter()
                            .map(|(key, value)| (key, Value::String(value)))
                            .collect(),
                    ),
                );
            }
        }

        if update.clear_env_vars
            || !update.add_env_vars.is_empty()
            || !update.unset_env_vars.is_empty()
        {
            let mut env_vars = if update.clear_env_vars {
                Vec::new()
            } else {
                parse_toml_string_array(
                    server.get("env_vars"),
                    "env_vars",
                    "server",
                    &resolved_name,
                )?
            };
            env_vars.retain(|name| !update.unset_env_vars.contains(name));
            merge_env_vars(&mut env_vars, update.add_env_vars.clone());
            if env_vars.is_empty() {
                server.remove("env_vars");
            } else {
                server.insert(
                    "env_vars".to_string(),
                    Value::Array(env_vars.into_iter().map(Value::String).collect()),
                );
            }
        }

        resolved_name
    };

    save_config_table(config_path, &config)?;
    load_server_config(config_path, &resolved_name)
}

pub fn load_codex_servers_for_import() -> Result<(PathBuf, ImportPlan), Box<dyn Error>> {
    let path = codex_config_path()?;
    let plan = load_codex_servers_for_import_from_path(&path)?;
    Ok((path, plan))
}

pub fn load_opencode_servers_for_import() -> Result<(PathBuf, ImportPlan), Box<dyn Error>> {
    let path = opencode_config_path()?;
    let plan = load_opencode_servers_for_import_from_path(&path)?;
    Ok((path, plan))
}

pub fn load_claude_servers_for_import() -> Result<(PathBuf, ImportPlan), Box<dyn Error>> {
    let path = claude_config_path()?;
    let plan = load_claude_servers_for_import_from_path(&path)?;
    Ok((path, plan))
}

pub fn install_codex_mcp_server() -> Result<InstallMcpServerResult, Box<dyn Error>> {
    let config_path = codex_config_path()?;
    let mut config = load_config_table(&config_path)?;
    let desired_server = proxy_stdio_server(CODEX_PROVIDER_NAME);

    let (name, status) = {
        let servers_value = config
            .entry("mcp_servers")
            .or_insert_with(|| Value::Table(Table::new()));
        let servers = servers_value
            .as_table_mut()
            .ok_or_else(|| "`mcp_servers` in Codex config must be a table".to_string())?;

        match inspect_codex_self_server(servers, CODEX_PROVIDER_NAME) {
            Some((name, true)) => {
                return Ok(InstallMcpServerResult {
                    name,
                    config_path,
                    status: InstallMcpServerStatus::AlreadyInstalled,
                });
            }
            Some((name, false)) => {
                servers.insert(name.clone(), codex_server_value(&desired_server));
                (name, InstallMcpServerStatus::Updated)
            }
            None => {
                let name = next_available_server_name(servers.keys().map(String::as_str));
                servers.insert(name.clone(), codex_server_value(&desired_server));
                (name, InstallMcpServerStatus::Installed)
            }
        }
    };

    save_config_table(&config_path, &config)?;

    Ok(InstallMcpServerResult {
        name,
        config_path,
        status,
    })
}

pub fn install_opencode_mcp_server() -> Result<InstallMcpServerResult, Box<dyn Error>> {
    let config_path = opencode_config_path()?;
    let mut config = load_opencode_config(&config_path)?;
    let desired_server = proxy_stdio_server(OPENCODE_PROVIDER_NAME);

    let (name, status) = {
        let root = config
            .as_object_mut()
            .ok_or_else(|| "OpenCode config root must be a JSON object".to_string())?;
        let servers_value = root
            .entry("mcp".to_string())
            .or_insert_with(|| JsonValue::Object(JsonMap::new()));
        let servers = servers_value
            .as_object_mut()
            .ok_or_else(|| "`mcp` in OpenCode config must be an object".to_string())?;

        match inspect_opencode_self_server(servers, OPENCODE_PROVIDER_NAME) {
            Some((name, true)) => {
                return Ok(InstallMcpServerResult {
                    name,
                    config_path,
                    status: InstallMcpServerStatus::AlreadyInstalled,
                });
            }
            Some((name, false)) => {
                servers.insert(name.clone(), opencode_server_value(&desired_server));
                (name, InstallMcpServerStatus::Updated)
            }
            None => {
                let name = next_available_server_name(servers.keys().map(String::as_str));
                servers.insert(name.clone(), opencode_server_value(&desired_server));
                (name, InstallMcpServerStatus::Installed)
            }
        }
    };

    save_opencode_config(&config_path, &config)?;

    Ok(InstallMcpServerResult {
        name,
        config_path,
        status,
    })
}

pub fn install_claude_mcp_server() -> Result<InstallMcpServerResult, Box<dyn Error>> {
    let config_path = claude_config_path()?;
    let mut config = load_claude_config(&config_path)?;
    let desired_server = proxy_stdio_server(CLAUDE_PROVIDER_NAME);

    let (name, status) = {
        let root = config
            .as_object_mut()
            .ok_or_else(|| "Claude Code config root must be a JSON object".to_string())?;
        let servers_value = root
            .entry("mcpServers".to_string())
            .or_insert_with(|| JsonValue::Object(JsonMap::new()));
        let servers = servers_value
            .as_object_mut()
            .ok_or_else(|| "`mcpServers` in Claude Code config must be an object".to_string())?;

        match inspect_claude_self_server(servers, CLAUDE_PROVIDER_NAME) {
            Some((name, true)) => {
                return Ok(InstallMcpServerResult {
                    name,
                    config_path,
                    status: InstallMcpServerStatus::AlreadyInstalled,
                });
            }
            Some((name, false)) => {
                servers.insert(name.clone(), claude_server_value(&desired_server));
                (name, InstallMcpServerStatus::Updated)
            }
            None => {
                let name = next_available_server_name(servers.keys().map(String::as_str));
                servers.insert(name.clone(), claude_server_value(&desired_server));
                (name, InstallMcpServerStatus::Installed)
            }
        }
    };

    save_claude_config(&config_path, &config)?;

    Ok(InstallMcpServerResult {
        name,
        config_path,
        status,
    })
}

pub fn replace_codex_mcp_servers() -> Result<ReplaceMcpServersResult, Box<dyn Error>> {
    let config_path = codex_config_path()?;
    replace_codex_mcp_servers_from_path(&config_path)
}

pub fn replace_opencode_mcp_servers() -> Result<ReplaceMcpServersResult, Box<dyn Error>> {
    let config_path = opencode_config_path()?;
    replace_opencode_mcp_servers_from_path(&config_path)
}

pub fn replace_claude_mcp_servers() -> Result<ReplaceMcpServersResult, Box<dyn Error>> {
    let config_path = claude_config_path()?;
    replace_claude_mcp_servers_from_path(&config_path)
}

pub fn restore_codex_mcp_servers() -> Result<RestoreMcpServersResult, Box<dyn Error>> {
    let config_path = codex_config_path()?;
    restore_codex_mcp_servers_from_path(&config_path)
}

pub fn restore_opencode_mcp_servers() -> Result<RestoreMcpServersResult, Box<dyn Error>> {
    let config_path = opencode_config_path()?;
    restore_opencode_mcp_servers_from_path(&config_path)
}

pub fn restore_claude_mcp_servers() -> Result<RestoreMcpServersResult, Box<dyn Error>> {
    let config_path = claude_config_path()?;
    restore_claude_mcp_servers_from_path(&config_path)
}

pub fn load_codex_runtime_config() -> CodexRuntimeConfig {
    CodexRuntimeConfig {
        model: DEFAULT_MODEL.to_string(),
    }
}

pub fn load_opencode_runtime_config() -> OpencodeRuntimeConfig {
    OpencodeRuntimeConfig {
        model: DEFAULT_OPENCODE_MODEL.to_string(),
    }
}

pub fn load_claude_runtime_config() -> ClaudeRuntimeConfig {
    ClaudeRuntimeConfig {
        model: DEFAULT_CLAUDE_MODEL.to_string(),
    }
}

pub fn load_model_provider_config(provider: &str) -> Result<ModelProviderConfig, Box<dyn Error>> {
    match provider {
        CODEX_PROVIDER_NAME => Ok(ModelProviderConfig::Codex(load_codex_runtime_config())),
        OPENCODE_PROVIDER_NAME => Ok(ModelProviderConfig::Opencode(load_opencode_runtime_config())),
        CLAUDE_PROVIDER_NAME => Ok(ModelProviderConfig::Claude(load_claude_runtime_config())),
        _ => Err(format!(
            "unsupported provider `{provider}`; supported providers are `codex`, `opencode`, and `claude`"
        )
        .into()),
    }
}

pub fn contains_server_name(config: &Table, requested_name: &str) -> bool {
    let normalized = sanitize_name(requested_name);
    if normalized.is_empty() {
        return false;
    }

    has_server_name(config, &normalized)
}

fn codex_config_path() -> Result<PathBuf, Box<dyn Error>> {
    if let Some(codex_home) = env::var_os(CODEX_HOME_ENV).filter(|value| !value.is_empty()) {
        return Ok(PathBuf::from(codex_home).join("config.toml"));
    }

    expand_tilde(Path::new(DEFAULT_CODEX_CONFIG_PATH))
}

fn opencode_config_path() -> Result<PathBuf, Box<dyn Error>> {
    expand_tilde(Path::new(DEFAULT_OPENCODE_CONFIG_PATH))
}

fn claude_config_path() -> Result<PathBuf, Box<dyn Error>> {
    expand_tilde(Path::new(DEFAULT_CLAUDE_CONFIG_PATH))
}

fn replace_codex_mcp_servers_from_path(
    config_path: &Path,
) -> Result<ReplaceMcpServersResult, Box<dyn Error>> {
    let mut config = load_config_table(config_path)?;
    let existing_servers = match config.get("mcp_servers") {
        None => Table::new(),
        Some(Value::Table(servers)) => servers.clone(),
        Some(_) => return Err("`mcp_servers` in Codex config must be a table".into()),
    };
    let backup_path = sibling_backup_path(config_path, "msp-backup");

    merge_codex_servers_into_backup(&backup_path, &existing_servers)?;

    if config.remove("mcp_servers").is_some() {
        save_config_table(config_path, &config)?;
    }

    Ok(ReplaceMcpServersResult {
        config_path: config_path.to_path_buf(),
        backup_path,
        backed_up_server_count: existing_servers.len(),
        removed_server_count: existing_servers.len(),
    })
}

fn replace_opencode_mcp_servers_from_path(
    config_path: &Path,
) -> Result<ReplaceMcpServersResult, Box<dyn Error>> {
    let mut config = load_opencode_config(config_path)?;
    let root = config
        .as_object_mut()
        .ok_or_else(|| "OpenCode config root must be a JSON object".to_string())?;
    let existing_servers = match root.get("mcp") {
        None => JsonMap::new(),
        Some(JsonValue::Object(servers)) => servers.clone(),
        Some(_) => return Err("`mcp` in OpenCode config must be an object".into()),
    };
    let backup_path = sibling_backup_path(config_path, "msp-backup");

    merge_opencode_servers_into_backup(&backup_path, &existing_servers)?;

    if root.remove("mcp").is_some() {
        save_opencode_config(config_path, &config)?;
    }

    Ok(ReplaceMcpServersResult {
        config_path: config_path.to_path_buf(),
        backup_path,
        backed_up_server_count: existing_servers.len(),
        removed_server_count: existing_servers.len(),
    })
}

fn replace_claude_mcp_servers_from_path(
    config_path: &Path,
) -> Result<ReplaceMcpServersResult, Box<dyn Error>> {
    let mut config = load_claude_config(config_path)?;
    let root = config
        .as_object_mut()
        .ok_or_else(|| "Claude Code config root must be a JSON object".to_string())?;
    let existing_servers = match root.get("mcpServers") {
        None => JsonMap::new(),
        Some(JsonValue::Object(servers)) => servers.clone(),
        Some(_) => return Err("`mcpServers` in Claude Code config must be an object".into()),
    };
    let backup_path = sibling_backup_path(config_path, "msp-backup");

    merge_claude_servers_into_backup(&backup_path, &existing_servers)?;

    if root.remove("mcpServers").is_some() {
        save_claude_config(config_path, &config)?;
    }

    Ok(ReplaceMcpServersResult {
        config_path: config_path.to_path_buf(),
        backup_path,
        backed_up_server_count: existing_servers.len(),
        removed_server_count: existing_servers.len(),
    })
}

fn restore_codex_mcp_servers_from_path(
    config_path: &Path,
) -> Result<RestoreMcpServersResult, Box<dyn Error>> {
    let backup_path = sibling_backup_path(config_path, "msp-backup");
    let backup = load_required_codex_backup(&backup_path)?;
    let restored_servers = backup
        .get("mcp_servers")
        .and_then(Value::as_table)
        .ok_or_else(|| {
            format!(
                "no `mcp_servers` table found in Codex backup {}",
                format_path_for_display(&backup_path)
            )
        })?
        .clone();

    let mut config = load_config_table(config_path)?;
    let removed_self_server_count = remove_codex_self_servers(&mut config)?;
    merge_codex_servers_into_target(&mut config, &restored_servers)?;
    save_config_table(config_path, &config)?;

    Ok(RestoreMcpServersResult {
        config_path: config_path.to_path_buf(),
        backup_path,
        removed_self_server_count,
        restored_server_count: restored_servers.len(),
    })
}

fn restore_opencode_mcp_servers_from_path(
    config_path: &Path,
) -> Result<RestoreMcpServersResult, Box<dyn Error>> {
    let backup_path = sibling_backup_path(config_path, "msp-backup");
    let backup = load_required_opencode_backup(&backup_path)?;
    let restored_servers = backup
        .get("mcp")
        .and_then(JsonValue::as_object)
        .ok_or_else(|| {
            format!(
                "no `mcp` object found in OpenCode backup {}",
                format_path_for_display(&backup_path)
            )
        })?
        .clone();

    let mut config = load_opencode_config(config_path)?;
    let removed_self_server_count = remove_opencode_self_servers(&mut config)?;
    merge_opencode_servers_into_target(&mut config, &restored_servers)?;
    save_opencode_config(config_path, &config)?;

    Ok(RestoreMcpServersResult {
        config_path: config_path.to_path_buf(),
        backup_path,
        removed_self_server_count,
        restored_server_count: restored_servers.len(),
    })
}

fn restore_claude_mcp_servers_from_path(
    config_path: &Path,
) -> Result<RestoreMcpServersResult, Box<dyn Error>> {
    let backup_path = sibling_backup_path(config_path, "msp-backup");
    let backup = load_required_claude_backup(&backup_path)?;
    let restored_servers = backup
        .get("mcpServers")
        .and_then(JsonValue::as_object)
        .ok_or_else(|| {
            format!(
                "no `mcpServers` object found in Claude Code backup {}",
                format_path_for_display(&backup_path)
            )
        })?
        .clone();

    let mut config = load_claude_config(config_path)?;
    let removed_self_server_count = remove_claude_self_servers(&mut config)?;
    merge_claude_servers_into_target(&mut config, &restored_servers)?;
    save_claude_config(config_path, &config)?;

    Ok(RestoreMcpServersResult {
        config_path: config_path.to_path_buf(),
        backup_path,
        removed_self_server_count,
        restored_server_count: restored_servers.len(),
    })
}

fn load_codex_servers_for_import_from_path(path: &Path) -> Result<ImportPlan, Box<dyn Error>> {
    if !path.exists() {
        return Err(format!(
            "Codex config not found at {}",
            format_path_for_display(path)
        )
        .into());
    }

    let config = load_config_table(path)?;
    let servers = config
        .get("mcp_servers")
        .and_then(Value::as_table)
        .ok_or_else(|| {
            format!(
                "no `mcp_servers` table found in Codex config {}",
                format_path_for_display(path)
            )
        })?;

    if servers.is_empty() {
        return Err(format!(
            "no MCP servers found in Codex config {}",
            format_path_for_display(path)
        )
        .into());
    }

    let mut names = servers.keys().cloned().collect::<Vec<_>>();
    names.sort();

    let mut importable_servers = Vec::new();
    let mut skipped_self_servers = Vec::new();

    for name in names {
        let server = servers[&name]
            .as_table()
            .ok_or_else(|| format!("Codex MCP server `{name}` must be a table"))?;
        validate_importable_codex_server(&name, server)?;
        let enabled = parse_codex_import_server_enabled(server, &name)?;
        let imported = codex_imported_server_command(server, &name)?;

        if imported.url.is_none() && is_self_server_command(&imported.command) {
            skipped_self_servers.push(name);
            continue;
        }

        importable_servers.push(ImportableServer {
            name,
            command: imported.command,
            url: imported.url,
            headers: imported.headers,
            enabled,
            env: imported.env,
            env_vars: imported.env_vars,
        });
    }

    Ok(ImportPlan {
        servers: importable_servers,
        skipped_self_servers,
    })
}

fn load_opencode_servers_for_import_from_path(path: &Path) -> Result<ImportPlan, Box<dyn Error>> {
    if !path.exists() {
        return Err(format!(
            "OpenCode config not found at {}",
            format_path_for_display(path)
        )
        .into());
    }

    let contents = fs::read_to_string(path)?;
    let config: serde_json::Value = serde_json::from_str(&contents)?;
    let servers = config
        .get("mcp")
        .and_then(serde_json::Value::as_object)
        .ok_or_else(|| {
            format!(
                "no `mcp` object found in OpenCode config {}",
                format_path_for_display(path)
            )
        })?;

    if servers.is_empty() {
        return Err(format!(
            "no MCP servers found in OpenCode config {}",
            format_path_for_display(path)
        )
        .into());
    }

    let mut names = servers.keys().cloned().collect::<Vec<_>>();
    names.sort();

    let mut importable_servers = Vec::new();
    let mut skipped_self_servers = Vec::new();

    for name in names {
        let server = servers[&name]
            .as_object()
            .ok_or_else(|| format!("OpenCode MCP server `{name}` must be an object"))?;
        validate_importable_opencode_server(&name, server)?;
        let enabled = parse_opencode_import_server_enabled(server, &name)?;
        let imported = opencode_imported_server_command(server, &name)?;

        if imported.url.is_none() && is_self_server_command(&imported.command) {
            skipped_self_servers.push(name);
            continue;
        }

        importable_servers.push(ImportableServer {
            name,
            command: imported.command,
            url: imported.url,
            headers: imported.headers,
            enabled,
            env: imported.env,
            env_vars: imported.env_vars,
        });
    }

    Ok(ImportPlan {
        servers: importable_servers,
        skipped_self_servers,
    })
}

fn load_claude_servers_for_import_from_path(path: &Path) -> Result<ImportPlan, Box<dyn Error>> {
    if !path.exists() {
        return Err(format!(
            "Claude Code config not found at {}",
            format_path_for_display(path)
        )
        .into());
    }

    let contents = fs::read_to_string(path)?;
    let config: serde_json::Value = serde_json::from_str(&contents)?;
    let servers = config
        .get("mcpServers")
        .and_then(serde_json::Value::as_object)
        .ok_or_else(|| {
            format!(
                "no `mcpServers` object found in Claude Code config {}",
                format_path_for_display(path)
            )
        })?;

    if servers.is_empty() {
        return Err(format!(
            "no MCP servers found in Claude Code config {}",
            format_path_for_display(path)
        )
        .into());
    }

    let mut names = servers.keys().cloned().collect::<Vec<_>>();
    names.sort();

    let mut importable_servers = Vec::new();
    let mut skipped_self_servers = Vec::new();

    for name in names {
        let server = servers[&name]
            .as_object()
            .ok_or_else(|| format!("Claude Code MCP server `{name}` must be an object"))?;
        validate_importable_claude_server(&name, server)?;
        let imported = claude_imported_server_command(server, &name)?;

        if imported.url.is_none() && is_self_server_command(&imported.command) {
            skipped_self_servers.push(name);
            continue;
        }

        importable_servers.push(ImportableServer {
            name,
            command: imported.command,
            url: imported.url,
            headers: imported.headers,
            enabled: true,
            env: imported.env,
            env_vars: imported.env_vars,
        });
    }

    Ok(ImportPlan {
        servers: importable_servers,
        skipped_self_servers,
    })
}

fn insert_server(
    config: &mut Table,
    name: &str,
    server: &StdioServer,
    enabled: bool,
    env: BTreeMap<String, String>,
    env_vars: Vec<String>,
) -> Result<(), Box<dyn Error>> {
    let servers_value = config
        .entry("servers")
        .or_insert_with(|| Value::Table(Table::new()));
    let servers = servers_value
        .as_table_mut()
        .ok_or_else(|| "`servers` in config must be a table".to_string())?;

    let mut server_table = Table::new();
    server_table.insert("command".to_string(), Value::String(server.command.clone()));
    server_table.insert(
        "args".to_string(),
        Value::Array(server.args.iter().cloned().map(Value::String).collect()),
    );
    if !enabled {
        server_table.insert("enabled".to_string(), Value::Boolean(false));
    }
    if !env.is_empty() {
        server_table.insert(
            "env".to_string(),
            Value::Table(
                env.into_iter()
                    .map(|(key, value)| (key, Value::String(value)))
                    .collect(),
            ),
        );
    }
    if !env_vars.is_empty() {
        server_table.insert(
            "env_vars".to_string(),
            Value::Array(env_vars.into_iter().map(Value::String).collect()),
        );
    }

    servers.insert(name.to_string(), Value::Table(server_table));
    Ok(())
}

fn insert_remote_server(
    config: &mut Table,
    name: &str,
    url: &str,
    headers: BTreeMap<String, String>,
    enabled: bool,
    env: BTreeMap<String, String>,
    env_vars: Vec<String>,
) -> Result<(), Box<dyn Error>> {
    let servers_value = config
        .entry("servers")
        .or_insert_with(|| Value::Table(Table::new()));
    let servers = servers_value
        .as_table_mut()
        .ok_or_else(|| "`servers` in config must be a table".to_string())?;

    let mut server_table = Table::new();
    server_table.insert("url".to_string(), Value::String(url.to_string()));
    if !headers.is_empty() {
        server_table.insert(
            "headers".to_string(),
            Value::Table(
                headers
                    .into_iter()
                    .map(|(key, value)| (key, Value::String(value)))
                    .collect(),
            ),
        );
    }
    if !enabled {
        server_table.insert("enabled".to_string(), Value::Boolean(false));
    }
    if !env.is_empty() {
        server_table.insert(
            "env".to_string(),
            Value::Table(
                env.into_iter()
                    .map(|(key, value)| (key, Value::String(value)))
                    .collect(),
            ),
        );
    }
    if !env_vars.is_empty() {
        server_table.insert(
            "env_vars".to_string(),
            Value::Array(env_vars.into_iter().map(Value::String).collect()),
        );
    }

    servers.insert(name.to_string(), Value::Table(server_table));
    Ok(())
}

fn parse_server_enabled(server: &Table, name: &str) -> Result<bool, Box<dyn Error>> {
    match server.get("enabled") {
        Some(Value::Boolean(enabled)) => Ok(*enabled),
        Some(_) => Err(format!("server `{name}` has a non-boolean `enabled` field").into()),
        None => Ok(true),
    }
}

fn resolved_server_transport(server: &Table, name: &str) -> Result<&'static str, Box<dyn Error>> {
    if let Some(transport) = configured_server_transport(server, name)? {
        return Ok(transport);
    }

    infer_server_transport(server, name)
}

fn configured_server_transport(
    server: &Table,
    name: &str,
) -> Result<Option<&'static str>, Box<dyn Error>> {
    match server.get("transport") {
        Some(Value::String(transport)) => match transport.as_str() {
            "stdio" => Ok(Some("stdio")),
            "remote" => Ok(Some("remote")),
            other => Err(format!(
                "server `{name}` uses unsupported transport `{other}`, only `stdio` and `remote` are supported"
            )
            .into()),
        },
        Some(_) => Err(format!("server `{name}` has a non-string `transport` field").into()),
        None => Ok(None),
    }
}

fn infer_server_transport(server: &Table, name: &str) -> Result<&'static str, Box<dyn Error>> {
    let has_command = server.contains_key("command");
    let has_url = server.contains_key("url");

    match (has_command, has_url) {
        (true, _) => Ok("stdio"),
        (false, true) => Ok("remote"),
        (false, false) => Err(format!("server `{name}` must define `command` or `url`").into()),
    }
}

fn parse_remote_server_url<'a>(server: &'a Table, name: &str) -> Result<&'a str, Box<dyn Error>> {
    server
        .get("url")
        .and_then(Value::as_str)
        .ok_or_else(|| format!("server `{name}` is missing `url`").into())
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

fn parse_toml_string_table(
    value: Option<&Value>,
    field_name: &str,
    kind: &str,
    name: &str,
) -> Result<BTreeMap<String, String>, Box<dyn Error>> {
    match value {
        None => Ok(BTreeMap::new()),
        Some(Value::Table(table)) => table
            .iter()
            .map(|(key, value)| {
                value
                    .as_str()
                    .map(|value| (key.clone(), value.to_string()))
                    .ok_or_else(|| {
                        format!(
                            "{kind} `{name}` contains a non-string `{field_name}` value `{key}`"
                        )
                    })
            })
            .collect::<Result<BTreeMap<_, _>, _>>()
            .map_err(Into::into),
        Some(_) => Err(format!("{kind} `{name}` has a non-table `{field_name}` field").into()),
    }
}

fn parse_toml_string_array(
    value: Option<&Value>,
    field_name: &str,
    kind: &str,
    name: &str,
) -> Result<Vec<String>, Box<dyn Error>> {
    match value {
        None => Ok(Vec::new()),
        Some(Value::Array(items)) => items
            .iter()
            .map(|value| {
                value.as_str().map(ToOwned::to_owned).ok_or_else(|| {
                    format!("{kind} `{name}` contains a non-string `{field_name}` entry")
                })
            })
            .collect::<Result<Vec<_>, _>>()
            .map_err(Into::into),
        Some(_) => Err(format!("{kind} `{name}` has a non-array `{field_name}` field").into()),
    }
}

fn parse_json_string_object(
    value: Option<&JsonValue>,
    field_name: &str,
    kind: &str,
    name: &str,
) -> Result<BTreeMap<String, String>, Box<dyn Error>> {
    match value {
        None => Ok(BTreeMap::new()),
        Some(JsonValue::Object(map)) => map
            .iter()
            .map(|(key, value)| {
                value
                    .as_str()
                    .map(|value| (key.clone(), value.to_string()))
                    .ok_or_else(|| {
                        format!(
                            "{kind} `{name}` contains a non-string `{field_name}` value `{key}`"
                        )
                    })
            })
            .collect::<Result<BTreeMap<_, _>, _>>()
            .map_err(Into::into),
        Some(_) => Err(format!("{kind} `{name}` has a non-object `{field_name}` field").into()),
    }
}

fn resolved_server_table<'a>(
    config: &'a Table,
    requested_name: &str,
) -> Result<(String, &'a Table), Box<dyn Error>> {
    let servers = config
        .get("servers")
        .and_then(Value::as_table)
        .ok_or_else(|| "no `servers` table found in config".to_string())?;

    let resolved_name = resolve_server_name(servers, requested_name)
        .ok_or_else(|| format!("server `{requested_name}` not found"))?;
    let server = servers
        .get(&resolved_name)
        .and_then(Value::as_table)
        .ok_or_else(|| format!("server `{resolved_name}` must be a table"))?;

    Ok((resolved_name, server))
}

fn resolved_server_table_mut<'a>(
    config: &'a mut Table,
    requested_name: &str,
) -> Result<(String, &'a mut Table), Box<dyn Error>> {
    let servers = config
        .get_mut("servers")
        .and_then(Value::as_table_mut)
        .ok_or_else(|| "no `servers` table found in config".to_string())?;

    let resolved_name = resolve_server_name(servers, requested_name)
        .ok_or_else(|| format!("server `{requested_name}` not found"))?;
    let server = servers
        .get_mut(&resolved_name)
        .and_then(Value::as_table_mut)
        .ok_or_else(|| format!("server `{resolved_name}` must be a table"))?;

    Ok((resolved_name, server))
}

fn server_config_snapshot(
    resolved_name: &str,
    server: &Table,
) -> Result<ServerConfigSnapshot, Box<dyn Error>> {
    let transport = resolved_server_transport(server, resolved_name)?;
    let enabled = parse_server_enabled(server, resolved_name)?;
    let env = parse_toml_string_table(server.get("env"), "env", "server", resolved_name)?;
    let env_vars =
        parse_toml_string_array(server.get("env_vars"), "env_vars", "server", resolved_name)?;
    let (command, args, url, headers) = match transport {
        "stdio" => (
            Some(
                server
                    .get("command")
                    .and_then(Value::as_str)
                    .ok_or_else(|| format!("server `{resolved_name}` is missing `command`"))?
                    .to_string(),
            ),
            parse_toml_string_array(server.get("args"), "args", "server", resolved_name)?,
            None,
            BTreeMap::new(),
        ),
        "remote" => (
            None,
            Vec::new(),
            Some(parse_remote_server_url(server, resolved_name)?.to_string()),
            parse_toml_string_table(server.get("headers"), "headers", "server", resolved_name)?,
        ),
        other => {
            return Err(format!(
                "server `{resolved_name}` uses unsupported transport `{other}`, only `stdio` and `remote` are supported"
            )
            .into())
        }
    };

    Ok(ServerConfigSnapshot {
        name: resolved_name.to_string(),
        transport: transport.to_string(),
        enabled,
        command,
        args,
        url,
        headers,
        env,
        env_vars,
    })
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

fn collect_remote_header_env_vars(headers: &BTreeMap<String, String>) -> Vec<String> {
    let mut env_vars = Vec::new();

    for value in headers.values() {
        merge_env_vars(&mut env_vars, collect_remote_header_value_env_vars(value));
    }

    env_vars
}

fn collect_remote_header_value_env_vars(value: &str) -> Vec<String> {
    collect_env_var_names(value)
}

fn merge_env_vars(target: &mut Vec<String>, additions: Vec<String>) {
    for name in additions {
        if !target.contains(&name) {
            target.push(name);
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

fn has_server_name(config: &Table, name: &str) -> bool {
    config
        .get("servers")
        .and_then(Value::as_table)
        .map(|servers| servers.contains_key(name))
        .unwrap_or(false)
}

fn resolve_server_name(servers: &Table, requested_name: &str) -> Option<String> {
    if servers.contains_key(requested_name) {
        return Some(requested_name.to_string());
    }

    let normalized = sanitize_name(requested_name);
    if normalized.is_empty() {
        return None;
    }

    servers.contains_key(&normalized).then_some(normalized)
}

fn load_opencode_config(path: &Path) -> Result<JsonValue, Box<dyn Error>> {
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

fn load_claude_config(path: &Path) -> Result<JsonValue, Box<dyn Error>> {
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

fn merge_codex_servers_into_backup(
    backup_path: &Path,
    servers: &Table,
) -> Result<(), Box<dyn Error>> {
    let mut backup = load_config_table(backup_path)?;
    let backup_servers_value = backup
        .entry("mcp_servers")
        .or_insert_with(|| Value::Table(Table::new()));
    let backup_servers = backup_servers_value
        .as_table_mut()
        .ok_or_else(|| "`mcp_servers` in Codex backup must be a table".to_string())?;

    for (name, server) in servers {
        backup_servers.insert(name.clone(), server.clone());
    }

    save_config_table(backup_path, &backup)?;
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

fn merge_codex_servers_into_target(
    config: &mut Table,
    servers: &Table,
) -> Result<(), Box<dyn Error>> {
    let target_servers_value = config
        .entry("mcp_servers")
        .or_insert_with(|| Value::Table(Table::new()));
    let target_servers = target_servers_value
        .as_table_mut()
        .ok_or_else(|| "`mcp_servers` in Codex config must be a table".to_string())?;

    for (name, server) in servers {
        target_servers.insert(name.clone(), server.clone());
    }

    Ok(())
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

fn load_required_codex_backup(path: &Path) -> Result<Table, Box<dyn Error>> {
    if !path.exists() {
        return Err(format!(
            "Codex backup not found at {}",
            format_path_for_display(path)
        )
        .into());
    }

    load_config_table(path)
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

fn remove_codex_self_servers(config: &mut Table) -> Result<usize, Box<dyn Error>> {
    let Some(servers_value) = config.get_mut("mcp_servers") else {
        return Ok(0);
    };
    let servers = servers_value
        .as_table_mut()
        .ok_or_else(|| "`mcp_servers` in Codex config must be a table".to_string())?;

    let names = servers
        .iter()
        .filter_map(|(name, value)| {
            let server = value.as_table()?;
            let raw_command = codex_server_raw_command(server)?;
            is_self_server_command(&raw_command).then_some(name.clone())
        })
        .collect::<Vec<_>>();

    for name in &names {
        servers.remove(name);
    }

    if servers.is_empty() {
        config.remove("mcp_servers");
    }

    Ok(names.len())
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
            is_self_server_command(&raw_command).then_some(name.clone())
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
            is_self_server_command(&raw_command).then_some(name.clone())
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

fn looks_like_url(value: &str) -> bool {
    value.starts_with("http://") || value.starts_with("https://")
}

#[cfg(test)]
mod tests;
