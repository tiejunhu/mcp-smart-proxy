use std::collections::BTreeMap;
use std::env;
use std::error::Error;
use std::fs;
use std::path::{Path, PathBuf};

use serde_json::{Map as JsonMap, Value as JsonValue};
use toml::{Table, Value};

use crate::paths::{cache_file_path, expand_tilde, sanitize_name, sibling_backup_path};
use crate::types::{
    CachedTools, CodexRuntimeConfig, ConfiguredServer, ModelProviderConfig, OpencodeRuntimeConfig,
};

const DEFAULT_MODEL: &str = "gpt-5.2";
const DEFAULT_OPENCODE_MODEL: &str = "openai/gpt-5.2";
const DEFAULT_CODEX_CONFIG_PATH: &str = "~/.codex/config.toml";
const DEFAULT_OPENCODE_CONFIG_PATH: &str = "~/.config/opencode/opencode.json";
const CODEX_HOME_ENV: &str = "CODEX_HOME";
const CODEX_PROVIDER_NAME: &str = "codex";
const OPENCODE_PROVIDER_NAME: &str = "opencode";
const SELF_EXECUTABLE_NAME: &str = "msp";
const SELF_SUBCOMMAND_NAME: &str = "mcp";

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
pub struct ImportableServer {
    pub name: String,
    pub command: Vec<String>,
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
    pub command: String,
    pub args: Vec<String>,
    pub env: BTreeMap<String, String>,
    pub env_vars: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct UpdateServerConfig {
    pub transport: Option<String>,
    pub command: Option<String>,
    pub clear_args: bool,
    pub add_args: Vec<String>,
    pub enabled: Option<bool>,
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
            || self.enabled.is_some()
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
    save_server(
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
    save_server(
        config_path,
        &server.name,
        server.command.clone(),
        server.enabled,
        server.env.clone(),
        server.env_vars.clone(),
    )
}

fn save_server(
    config_path: &Path,
    name: &str,
    raw_command: Vec<String>,
    enabled: bool,
    env: BTreeMap<String, String>,
    env_vars: Vec<String>,
) -> Result<String, Box<dyn Error>> {
    let normalized = normalize_add_command(raw_command);
    if is_self_server_command(&normalized) {
        return Err("cannot add `msp mcp` as a managed server".into());
    }
    let server = StdioServer::from_command(normalized)?;

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

            let transport = server
                .get("transport")
                .and_then(Value::as_str)
                .unwrap_or("stdio");
            if transport != "stdio" {
                return Err(format!(
                    "server `{name}` uses unsupported transport `{transport}`, only `stdio` is supported"
                )
                .into());
            }

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
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }

    let contents = toml::to_string_pretty(config)?;
    fs::write(path, contents)?;
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

    let transport = server
        .get("transport")
        .and_then(Value::as_str)
        .unwrap_or("stdio");
    if transport != "stdio" {
        return Err(format!(
            "server `{resolved_name}` uses unsupported transport `{transport}`, only `stdio` is supported"
        )
        .into());
    }

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

    let env = parse_toml_string_table(server.get("env"), "env", "server", &resolved_name)?;
    let env_vars =
        parse_toml_string_array(server.get("env_vars"), "env_vars", "server", &resolved_name)?;

    Ok((
        resolved_name,
        ConfiguredServer {
            command,
            args,
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

        if let Some(transport) = &update.transport {
            if transport != "stdio" {
                return Err(format!(
                    "server `{resolved_name}` uses unsupported transport `{transport}`, only `stdio` is supported"
                )
                .into());
            }
            server.insert("transport".to_string(), Value::String(transport.clone()));
        }

        if let Some(command) = &update.command {
            server.insert("command".to_string(), Value::String(command.clone()));
        }

        if update.clear_args || !update.add_args.is_empty() {
            let mut args = if update.clear_args {
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

pub fn replace_codex_mcp_servers() -> Result<ReplaceMcpServersResult, Box<dyn Error>> {
    let config_path = codex_config_path()?;
    replace_codex_mcp_servers_from_path(&config_path)
}

pub fn replace_opencode_mcp_servers() -> Result<ReplaceMcpServersResult, Box<dyn Error>> {
    let config_path = opencode_config_path()?;
    replace_opencode_mcp_servers_from_path(&config_path)
}

pub fn restore_codex_mcp_servers() -> Result<RestoreMcpServersResult, Box<dyn Error>> {
    let config_path = codex_config_path()?;
    restore_codex_mcp_servers_from_path(&config_path)
}

pub fn restore_opencode_mcp_servers() -> Result<RestoreMcpServersResult, Box<dyn Error>> {
    let config_path = opencode_config_path()?;
    restore_opencode_mcp_servers_from_path(&config_path)
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

pub fn load_model_provider_config(provider: &str) -> Result<ModelProviderConfig, Box<dyn Error>> {
    match provider {
        CODEX_PROVIDER_NAME => Ok(ModelProviderConfig::Codex(load_codex_runtime_config())),
        OPENCODE_PROVIDER_NAME => Ok(ModelProviderConfig::Opencode(load_opencode_runtime_config())),
        _ => Err(format!(
            "unsupported provider `{provider}`; supported providers are `codex` and `opencode`"
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

pub fn is_self_server_command(raw_command: &[String]) -> bool {
    let Some(command) = raw_command.first() else {
        return false;
    };

    let executable = Path::new(command)
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or(command.as_str())
        .trim_end_matches(".exe");

    executable == SELF_EXECUTABLE_NAME
        && raw_command.get(1).map(String::as_str) == Some(SELF_SUBCOMMAND_NAME)
}

fn normalize_add_command(raw_command: Vec<String>) -> Vec<String> {
    if raw_command.len() == 1 && looks_like_url(&raw_command[0]) {
        return mcp_remote_command(&raw_command[0], &BTreeMap::new()).0;
    }

    raw_command
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
                backup_path.display()
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
                backup_path.display()
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

fn load_codex_servers_for_import_from_path(path: &Path) -> Result<ImportPlan, Box<dyn Error>> {
    if !path.exists() {
        return Err(format!("Codex config not found at {}", path.display()).into());
    }

    let config = load_config_table(path)?;
    let servers = config
        .get("mcp_servers")
        .and_then(Value::as_table)
        .ok_or_else(|| {
            format!(
                "no `mcp_servers` table found in Codex config {}",
                path.display()
            )
        })?;

    if servers.is_empty() {
        return Err(format!("no MCP servers found in Codex config {}", path.display()).into());
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
        let (raw_command, env, env_vars) = codex_imported_server_command(server, &name)?;

        if is_self_server_command(&raw_command) {
            skipped_self_servers.push(name);
            continue;
        }

        importable_servers.push(ImportableServer {
            name,
            command: raw_command,
            enabled,
            env,
            env_vars,
        });
    }

    Ok(ImportPlan {
        servers: importable_servers,
        skipped_self_servers,
    })
}

fn load_opencode_servers_for_import_from_path(path: &Path) -> Result<ImportPlan, Box<dyn Error>> {
    if !path.exists() {
        return Err(format!("OpenCode config not found at {}", path.display()).into());
    }

    let contents = fs::read_to_string(path)?;
    let config: serde_json::Value = serde_json::from_str(&contents)?;
    let servers = config
        .get("mcp")
        .and_then(serde_json::Value::as_object)
        .ok_or_else(|| {
            format!(
                "no `mcp` object found in OpenCode config {}",
                path.display()
            )
        })?;

    if servers.is_empty() {
        return Err(format!("no MCP servers found in OpenCode config {}", path.display()).into());
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
        let (raw_command, env, env_vars) = opencode_imported_server_command(server, &name)?;

        if is_self_server_command(&raw_command) {
            skipped_self_servers.push(name);
            continue;
        }

        importable_servers.push(ImportableServer {
            name,
            command: raw_command,
            enabled,
            env,
            env_vars,
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
    server_table.insert("transport".to_string(), Value::String("stdio".to_string()));
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

fn parse_server_enabled(server: &Table, name: &str) -> Result<bool, Box<dyn Error>> {
    match server.get("enabled") {
        Some(Value::Boolean(enabled)) => Ok(*enabled),
        Some(_) => Err(format!("server `{name}` has a non-boolean `enabled` field").into()),
        None => Ok(true),
    }
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
    let transport = server
        .get("transport")
        .and_then(Value::as_str)
        .unwrap_or("stdio");
    if transport != "stdio" {
        return Err(format!(
            "server `{resolved_name}` uses unsupported transport `{transport}`, only `stdio` is supported"
        )
        .into());
    }

    let command = server
        .get("command")
        .and_then(Value::as_str)
        .ok_or_else(|| format!("server `{resolved_name}` is missing `command`"))?
        .to_string();
    let args = parse_toml_string_array(server.get("args"), "args", "server", resolved_name)?;
    let enabled = parse_server_enabled(server, resolved_name)?;
    let env = parse_toml_string_table(server.get("env"), "env", "server", resolved_name)?;
    let env_vars =
        parse_toml_string_array(server.get("env_vars"), "env_vars", "server", resolved_name)?;

    Ok(ServerConfigSnapshot {
        name: resolved_name.to_string(),
        transport: transport.to_string(),
        enabled,
        command,
        args,
        env,
        env_vars,
    })
}

fn codex_imported_server_command(
    server: &Table,
    name: &str,
) -> Result<(Vec<String>, BTreeMap<String, String>, Vec<String>), Box<dyn Error>> {
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
            } else if matches!(server.get("bearer_token_env_var"), Some(_)) {
                return Err(format!(
                    "Codex MCP server `{name}` has a non-string `bearer_token_env_var` field"
                )
                .into());
            }
            let (command, header_env_vars) = mcp_remote_command(url, &headers);
            merge_env_vars(&mut env_vars, header_env_vars);
            Ok((command, env, env_vars))
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
            Ok((raw_command, env, env_vars))
        }
        (None, None) => {
            Err(format!("Codex MCP server `{name}` is missing `command` or `url`").into())
        }
    }
}

fn opencode_imported_server_command(
    server: &JsonMap<String, JsonValue>,
    name: &str,
) -> Result<(Vec<String>, BTreeMap<String, String>, Vec<String>), Box<dyn Error>> {
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
            Ok((raw_command, env, Vec::new()))
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
            let (command, env_vars) = mcp_remote_command(url, &headers);
            Ok((command, BTreeMap::new(), env_vars))
        }
        other => Err(format!(
            "OpenCode MCP server `{name}` uses unsupported type `{other}`, only `local` and `remote` can be imported"
        )
        .into()),
    }
}

fn mcp_remote_command(url: &str, headers: &BTreeMap<String, String>) -> (Vec<String>, Vec<String>) {
    let mut command = vec![
        "npx".to_string(),
        "-y".to_string(),
        "mcp-remote".to_string(),
        url.to_string(),
    ];
    let mut env_vars = Vec::new();

    for (name, value) in headers {
        let (header_value, header_env_vars) = mcp_remote_header_value(value);
        command.push("--header".to_string());
        command.push(format!("{name}: {header_value}"));
        merge_env_vars(&mut env_vars, header_env_vars);
    }

    (command, env_vars)
}

fn mcp_remote_header_value(value: &str) -> (String, Vec<String>) {
    let mut rendered = String::new();
    let mut env_vars = Vec::new();
    let mut remaining = value;

    while let Some(start) = remaining.find("{env:") {
        rendered.push_str(&remaining[..start]);
        let Some(end) = remaining[start + 5..].find('}') else {
            rendered.push_str(&remaining[start..]);
            remaining = "";
            break;
        };
        let name = &remaining[start + 5..start + 5 + end];
        if name.is_empty() {
            rendered.push_str(&remaining[start..start + 6 + end]);
        } else {
            rendered.push_str("${");
            rendered.push_str(name);
            rendered.push('}');
            merge_env_vars(&mut env_vars, vec![name.to_string()]);
        }
        remaining = &remaining[start + 6 + end..];
    }

    rendered.push_str(remaining);
    (rendered, env_vars)
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

fn proxy_stdio_server(provider: &str) -> StdioServer {
    StdioServer {
        command: SELF_EXECUTABLE_NAME.to_string(),
        args: vec![
            SELF_SUBCOMMAND_NAME.to_string(),
            "--provider".to_string(),
            provider.to_string(),
        ],
    }
}

fn inspect_codex_self_server(servers: &Table, provider: &str) -> Option<(String, bool)> {
    inspect_self_server(
        servers.iter().filter_map(|(name, value)| {
            let server = value.as_table()?;
            let raw_command = codex_server_raw_command(server)?;
            Some((name.clone(), raw_command))
        }),
        provider,
    )
}

fn inspect_opencode_self_server(
    servers: &JsonMap<String, JsonValue>,
    provider: &str,
) -> Option<(String, bool)> {
    inspect_self_server(
        servers.iter().filter_map(|(name, value)| {
            let server = value.as_object()?;
            let raw_command = opencode_server_raw_command(server)?;
            Some((name.clone(), raw_command))
        }),
        provider,
    )
}

fn inspect_self_server(
    candidates: impl Iterator<Item = (String, Vec<String>)>,
    provider: &str,
) -> Option<(String, bool)> {
    let mut self_server_names = Vec::new();

    for (name, raw_command) in candidates {
        if !is_self_server_command(&raw_command) {
            continue;
        }
        if self_server_uses_provider(&raw_command, provider) {
            return Some((name, true));
        }
        self_server_names.push(name);
    }

    pick_existing_self_server_name(self_server_names).map(|name| (name, false))
}

fn codex_server_raw_command(server: &Table) -> Option<Vec<String>> {
    let command = server.get("command")?.as_str()?.to_string();
    let args = match server.get("args") {
        None => Vec::new(),
        Some(Value::Array(items)) => items
            .iter()
            .map(|value| value.as_str().map(ToOwned::to_owned))
            .collect::<Option<Vec<_>>>()?,
        Some(_) => return None,
    };

    let mut raw_command = vec![command];
    raw_command.extend(args);
    Some(raw_command)
}

fn opencode_server_raw_command(server: &JsonMap<String, JsonValue>) -> Option<Vec<String>> {
    server
        .get("command")?
        .as_array()?
        .iter()
        .map(|value| value.as_str().map(ToOwned::to_owned))
        .collect()
}

fn self_server_uses_provider(raw_command: &[String], provider: &str) -> bool {
    is_self_server_command(raw_command)
        && raw_command.len() == 4
        && raw_command[1] == SELF_SUBCOMMAND_NAME
        && raw_command[2] == "--provider"
        && raw_command[3] == provider
}

fn pick_existing_self_server_name(mut names: Vec<String>) -> Option<String> {
    names.sort_by(|left, right| match (left == "msp", right == "msp") {
        (true, false) => std::cmp::Ordering::Less,
        (false, true) => std::cmp::Ordering::Greater,
        _ => left.cmp(right),
    });
    names.into_iter().next()
}

fn next_available_server_name<'a>(existing_names: impl Iterator<Item = &'a str>) -> String {
    let existing_names = existing_names.collect::<std::collections::BTreeSet<_>>();
    if !existing_names.contains("msp") {
        return "msp".to_string();
    }

    let mut index = 1usize;
    loop {
        let candidate = format!("msp{index}");
        if !existing_names.contains(candidate.as_str()) {
            return candidate;
        }
        index += 1;
    }
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
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }

    let contents = serde_json::to_string_pretty(config)?;
    fs::write(path, contents)?;
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

fn load_required_codex_backup(path: &Path) -> Result<Table, Box<dyn Error>> {
    if !path.exists() {
        return Err(format!("Codex backup not found at {}", path.display()).into());
    }

    load_config_table(path)
}

fn load_required_opencode_backup(path: &Path) -> Result<JsonValue, Box<dyn Error>> {
    if !path.exists() {
        return Err(format!("OpenCode backup not found at {}", path.display()).into());
    }

    load_opencode_config(path)
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

fn looks_like_url(value: &str) -> bool {
    value.starts_with("http://") || value.starts_with("https://")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::DEFAULT_CONFIG_PATH;
    use crate::paths::{cache_file_path_from_home, expand_tilde};
    use std::path::PathBuf;
    use std::sync::{Mutex, OnceLock};
    use std::time::{SystemTime, UNIX_EPOCH};

    fn env_lock() -> &'static Mutex<()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(()))
    }

    fn with_home_env<T>(home: &Path, test: impl FnOnce() -> T) -> T {
        let _guard = env_lock().lock().unwrap();
        let previous_home = env::var("HOME").ok();

        unsafe {
            env::set_var("HOME", home);
        }

        let result = test();

        match previous_home {
            Some(value) => unsafe { env::set_var("HOME", value) },
            None => unsafe { env::remove_var("HOME") },
        }

        result
    }

    fn with_codex_home_env<T>(codex_home: &Path, test: impl FnOnce() -> T) -> T {
        let _guard = env_lock().lock().unwrap();
        let previous_codex_home = env::var(CODEX_HOME_ENV).ok();

        unsafe {
            env::set_var(CODEX_HOME_ENV, codex_home);
        }

        let result = test();

        match previous_codex_home {
            Some(value) => unsafe { env::set_var(CODEX_HOME_ENV, value) },
            None => unsafe { env::remove_var(CODEX_HOME_ENV) },
        }

        result
    }

    #[test]
    fn expands_default_config_path() {
        let home = PathBuf::from("/tmp/mcp-smart-proxy-home");
        unsafe {
            env::set_var("HOME", &home);
        }

        let expanded = expand_tilde(Path::new(DEFAULT_CONFIG_PATH)).unwrap();

        assert_eq!(expanded, home.join(".config/mcp-smart-proxy/config.toml"));
    }

    #[test]
    fn parses_arbitrary_toml_content() {
        let config: Table = toml::from_str(
            r#"
                listen_addr = "127.0.0.1:8080"

                [upstream]
                url = "https://example.com/mcp"
            "#,
        )
        .unwrap();

        assert_eq!(config["listen_addr"].as_str(), Some("127.0.0.1:8080"));
        assert_eq!(
            config["upstream"]
                .as_table()
                .and_then(|table| table["url"].as_str()),
            Some("https://example.com/mcp")
        );
    }

    #[test]
    fn normalizes_bare_url_add_command() {
        assert_eq!(
            normalize_add_command(vec!["https://ones.com/mcp".to_string()]),
            vec![
                "npx".to_string(),
                "-y".to_string(),
                "mcp-remote".to_string(),
                "https://ones.com/mcp".to_string()
            ]
        );
    }

    #[test]
    fn resolves_codex_config_path_from_codex_home() {
        let _guard = env_lock().lock().unwrap();
        let previous_codex_home = env::var(CODEX_HOME_ENV).ok();

        unsafe {
            env::set_var(CODEX_HOME_ENV, "/tmp/codex-home");
        }

        let path = codex_config_path().unwrap();

        assert_eq!(path, PathBuf::from("/tmp/codex-home/config.toml"));

        match previous_codex_home {
            Some(value) => unsafe { env::set_var(CODEX_HOME_ENV, value) },
            None => unsafe { env::remove_var(CODEX_HOME_ENV) },
        }
    }

    #[test]
    fn installs_codex_mcp_server_when_missing() {
        let codex_home = unique_test_path("codex-install-home");
        fs::create_dir_all(&codex_home).unwrap();

        with_codex_home_env(&codex_home, || {
            let installed = install_codex_mcp_server().unwrap();

            assert_eq!(installed.name, "msp");
            assert_eq!(installed.status, InstallMcpServerStatus::Installed);
            assert_eq!(installed.config_path, codex_home.join("config.toml"));

            let config = load_config_table(&installed.config_path).unwrap();
            let server = config["mcp_servers"]["msp"].as_table().unwrap();
            assert_eq!(server["command"].as_str(), Some("msp"));
            assert_eq!(
                server["args"].as_array().unwrap(),
                &vec![
                    Value::String("mcp".to_string()),
                    Value::String("--provider".to_string()),
                    Value::String("codex".to_string()),
                ]
            );
        });

        fs::remove_dir_all(codex_home).unwrap();
    }

    #[test]
    fn updates_existing_codex_self_server_to_requested_provider() {
        let codex_home = unique_test_path("codex-update-home");
        fs::create_dir_all(&codex_home).unwrap();
        let config_path = codex_home.join("config.toml");
        fs::write(
            &config_path,
            r#"
                [mcp_servers.proxy]
                command = "msp"
                args = ["mcp", "--provider", "opencode"]
            "#,
        )
        .unwrap();

        with_codex_home_env(&codex_home, || {
            let installed = install_codex_mcp_server().unwrap();

            assert_eq!(installed.name, "proxy");
            assert_eq!(installed.status, InstallMcpServerStatus::Updated);

            let config = load_config_table(&config_path).unwrap();
            let server = config["mcp_servers"]["proxy"].as_table().unwrap();
            assert_eq!(server["command"].as_str(), Some("msp"));
            assert_eq!(
                server["args"].as_array().unwrap(),
                &vec![
                    Value::String("mcp".to_string()),
                    Value::String("--provider".to_string()),
                    Value::String("codex".to_string()),
                ]
            );
        });

        fs::remove_dir_all(codex_home).unwrap();
    }

    #[test]
    fn installs_codex_mcp_server_with_numbered_name_when_msp_is_taken() {
        let codex_home = unique_test_path("codex-conflict-home");
        fs::create_dir_all(&codex_home).unwrap();
        let config_path = codex_home.join("config.toml");
        fs::write(
            &config_path,
            r#"
                [mcp_servers.msp]
                command = "npx"
                args = ["-y", "@modelcontextprotocol/server-github"]
            "#,
        )
        .unwrap();

        with_codex_home_env(&codex_home, || {
            let installed = install_codex_mcp_server().unwrap();

            assert_eq!(installed.name, "msp1");
            assert_eq!(installed.status, InstallMcpServerStatus::Installed);

            let config = load_config_table(&config_path).unwrap();
            let server = config["mcp_servers"]["msp1"].as_table().unwrap();
            assert_eq!(server["command"].as_str(), Some("msp"));
        });

        fs::remove_dir_all(codex_home).unwrap();
    }

    #[test]
    fn installs_opencode_mcp_server_when_missing() {
        let home = unique_test_path("opencode-install-home");
        fs::create_dir_all(&home).unwrap();

        with_home_env(&home, || {
            let installed = install_opencode_mcp_server().unwrap();

            assert_eq!(installed.name, "msp");
            assert_eq!(installed.status, InstallMcpServerStatus::Installed);
            assert_eq!(
                installed.config_path,
                home.join(".config/opencode/opencode.json")
            );

            let contents = fs::read_to_string(&installed.config_path).unwrap();
            let config: JsonValue = serde_json::from_str(&contents).unwrap();
            let server = config["mcp"]["msp"].as_object().unwrap();
            assert_eq!(server["type"].as_str(), Some("local"));
            assert_eq!(
                server["command"].as_array().unwrap(),
                &vec![
                    JsonValue::String("msp".to_string()),
                    JsonValue::String("mcp".to_string()),
                    JsonValue::String("--provider".to_string()),
                    JsonValue::String("opencode".to_string()),
                ]
            );
        });

        fs::remove_dir_all(home).unwrap();
    }

    #[test]
    fn replaces_codex_servers_after_merging_backup_without_duplicates() {
        let config_path = unique_test_path("codex-replace.toml");
        let backup_path = sibling_backup_path(&config_path, "msp-backup");
        fs::write(
            &config_path,
            r#"
                [mcp_servers.alpha]
                command = "npx"
                args = ["-y", "alpha-server"]

                [mcp_servers.beta]
                command = "uvx"
                args = ["beta-server"]
            "#,
        )
        .unwrap();
        fs::write(
            &backup_path,
            r#"
                [mcp_servers.beta]
                command = "old"
                args = ["beta-old"]

                [mcp_servers.gamma]
                command = "npx"
                args = ["-y", "gamma-server"]
            "#,
        )
        .unwrap();

        let replaced = replace_codex_mcp_servers_from_path(&config_path).unwrap();

        assert_eq!(replaced.config_path, config_path);
        assert_eq!(replaced.backup_path, backup_path);
        assert_eq!(replaced.backed_up_server_count, 2);
        assert_eq!(replaced.removed_server_count, 2);

        let config = load_config_table(&config_path).unwrap();
        assert!(config.get("mcp_servers").is_none());

        let backup = load_config_table(&backup_path).unwrap();
        let backup_servers = backup["mcp_servers"].as_table().unwrap();
        assert_eq!(backup_servers.len(), 3);
        assert_eq!(backup_servers["alpha"]["command"].as_str(), Some("npx"));
        assert_eq!(backup_servers["beta"]["command"].as_str(), Some("uvx"));
        assert_eq!(backup_servers["gamma"]["command"].as_str(), Some("npx"));

        fs::remove_file(config_path).unwrap();
        fs::remove_file(backup_path).unwrap();
    }

    #[test]
    fn restores_codex_servers_from_backup_after_removing_self_servers() {
        let config_path = unique_test_path("codex-restore.toml");
        let backup_path = sibling_backup_path(&config_path, "msp-backup");
        fs::write(
            &config_path,
            r#"
                [mcp_servers.msp]
                command = "msp"
                args = ["mcp", "--provider", "codex"]

                [mcp_servers.proxy]
                command = "msp"
                args = ["mcp", "--provider", "opencode"]
            "#,
        )
        .unwrap();
        fs::write(
            &backup_path,
            r#"
                [mcp_servers.alpha]
                command = "npx"
                args = ["-y", "alpha-server"]

                [mcp_servers.beta]
                command = "uvx"
                args = ["beta-server"]
            "#,
        )
        .unwrap();

        let restored = restore_codex_mcp_servers_from_path(&config_path).unwrap();

        assert_eq!(restored.config_path, config_path);
        assert_eq!(restored.backup_path, backup_path);
        assert_eq!(restored.removed_self_server_count, 2);
        assert_eq!(restored.restored_server_count, 2);

        let config = load_config_table(&config_path).unwrap();
        let servers = config["mcp_servers"].as_table().unwrap();
        assert_eq!(servers.len(), 2);
        assert!(servers.get("msp").is_none());
        assert!(servers.get("proxy").is_none());
        assert_eq!(servers["alpha"]["command"].as_str(), Some("npx"));
        assert_eq!(servers["beta"]["command"].as_str(), Some("uvx"));

        fs::remove_file(config_path).unwrap();
        fs::remove_file(backup_path).unwrap();
    }

    #[test]
    fn recognizes_existing_opencode_self_server_with_matching_provider() {
        let home = unique_test_path("opencode-existing-home");
        fs::create_dir_all(home.join(".config/opencode")).unwrap();
        let config_path = home.join(".config/opencode/opencode.json");
        fs::write(
            &config_path,
            r#"{
                "mcp": {
                    "proxy": {
                        "type": "local",
                        "command": ["msp", "mcp", "--provider", "opencode"]
                    }
                }
            }"#,
        )
        .unwrap();

        with_home_env(&home, || {
            let installed = install_opencode_mcp_server().unwrap();

            assert_eq!(installed.name, "proxy");
            assert_eq!(installed.status, InstallMcpServerStatus::AlreadyInstalled);
        });

        fs::remove_dir_all(home).unwrap();
    }

    #[test]
    fn replaces_opencode_servers_after_merging_backup_without_duplicates() {
        let config_path = unique_test_path("opencode-replace.json");
        let backup_path = sibling_backup_path(&config_path, "msp-backup");
        fs::write(
            &config_path,
            r#"{
                "mcp": {
                    "alpha": {
                        "type": "local",
                        "command": ["npx", "-y", "alpha-server"]
                    },
                    "beta": {
                        "type": "local",
                        "command": ["uvx", "beta-server"]
                    }
                }
            }"#,
        )
        .unwrap();
        fs::write(
            &backup_path,
            r#"{
                "mcp": {
                    "beta": {
                        "type": "local",
                        "command": ["old", "beta-old"]
                    },
                    "gamma": {
                        "type": "local",
                        "command": ["npx", "-y", "gamma-server"]
                    }
                }
            }"#,
        )
        .unwrap();

        let replaced = replace_opencode_mcp_servers_from_path(&config_path).unwrap();

        assert_eq!(replaced.config_path, config_path);
        assert_eq!(replaced.backup_path, backup_path);
        assert_eq!(replaced.backed_up_server_count, 2);
        assert_eq!(replaced.removed_server_count, 2);

        let config = load_opencode_config(&config_path).unwrap();
        assert!(config.get("mcp").is_none());

        let backup = load_opencode_config(&backup_path).unwrap();
        let backup_servers = backup["mcp"].as_object().unwrap();
        assert_eq!(backup_servers.len(), 3);
        assert_eq!(
            backup_servers["alpha"]["command"].as_array().unwrap(),
            &vec![
                JsonValue::String("npx".to_string()),
                JsonValue::String("-y".to_string()),
                JsonValue::String("alpha-server".to_string()),
            ]
        );
        assert_eq!(
            backup_servers["beta"]["command"].as_array().unwrap(),
            &vec![
                JsonValue::String("uvx".to_string()),
                JsonValue::String("beta-server".to_string()),
            ]
        );
        assert!(backup_servers.get("gamma").is_some());

        fs::remove_file(config_path).unwrap();
        fs::remove_file(backup_path).unwrap();
    }

    #[test]
    fn restores_opencode_servers_from_backup_after_removing_self_servers() {
        let config_path = unique_test_path("opencode-restore.json");
        let backup_path = sibling_backup_path(&config_path, "msp-backup");
        fs::write(
            &config_path,
            r#"{
                "mcp": {
                    "msp": {
                        "type": "local",
                        "command": ["msp", "mcp", "--provider", "opencode"]
                    },
                    "proxy": {
                        "type": "local",
                        "command": ["msp", "mcp", "--provider", "codex"]
                    }
                }
            }"#,
        )
        .unwrap();
        fs::write(
            &backup_path,
            r#"{
                "mcp": {
                    "alpha": {
                        "type": "local",
                        "command": ["npx", "-y", "alpha-server"]
                    },
                    "beta": {
                        "type": "local",
                        "command": ["uvx", "beta-server"]
                    }
                }
            }"#,
        )
        .unwrap();

        let restored = restore_opencode_mcp_servers_from_path(&config_path).unwrap();

        assert_eq!(restored.config_path, config_path);
        assert_eq!(restored.backup_path, backup_path);
        assert_eq!(restored.removed_self_server_count, 2);
        assert_eq!(restored.restored_server_count, 2);

        let config = load_opencode_config(&config_path).unwrap();
        let servers = config["mcp"].as_object().unwrap();
        assert_eq!(servers.len(), 2);
        assert!(servers.get("msp").is_none());
        assert!(servers.get("proxy").is_none());
        assert_eq!(
            servers["alpha"]["command"].as_array().unwrap(),
            &vec![
                JsonValue::String("npx".to_string()),
                JsonValue::String("-y".to_string()),
                JsonValue::String("alpha-server".to_string()),
            ]
        );
        assert_eq!(
            servers["beta"]["command"].as_array().unwrap(),
            &vec![
                JsonValue::String("uvx".to_string()),
                JsonValue::String("beta-server".to_string()),
            ]
        );

        fs::remove_file(config_path).unwrap();
        fs::remove_file(backup_path).unwrap();
    }

    #[test]
    fn loads_codex_servers_for_import_from_path() {
        let config_path = unique_test_path("codex-import.toml");
        fs::write(
            &config_path,
            r#"
                [mcp_servers.beta]
                command = "uvx"
                args = ["beta-server"]

                [mcp_servers.alpha]
                command = "npx"
                args = ["-y", "@modelcontextprotocol/server-github"]
            "#,
        )
        .unwrap();

        let plan = load_codex_servers_for_import_from_path(&config_path).unwrap();

        assert_eq!(
            plan.servers,
            vec![
                ImportableServer {
                    name: "alpha".to_string(),
                    command: vec![
                        "npx".to_string(),
                        "-y".to_string(),
                        "@modelcontextprotocol/server-github".to_string(),
                    ],
                    enabled: true,
                    env: BTreeMap::new(),
                    env_vars: Vec::new(),
                },
                ImportableServer {
                    name: "beta".to_string(),
                    command: vec!["uvx".to_string(), "beta-server".to_string()],
                    enabled: true,
                    env: BTreeMap::new(),
                    env_vars: Vec::new(),
                },
            ]
        );
        assert!(plan.skipped_self_servers.is_empty());

        fs::remove_file(config_path).unwrap();
    }

    #[test]
    fn preserves_codex_enabled_state_when_loading_import_plan() {
        let config_path = unique_test_path("codex-import-enabled.toml");
        fs::write(
            &config_path,
            r#"
                [mcp_servers.alpha]
                command = "npx"
                args = ["-y", "@modelcontextprotocol/server-github"]
                enabled = false

                [mcp_servers.beta]
                command = "uvx"
                args = ["beta-server"]
                enabled = true
            "#,
        )
        .unwrap();

        let plan = load_codex_servers_for_import_from_path(&config_path).unwrap();

        assert_eq!(plan.servers.len(), 2);
        assert!(!plan.servers[0].enabled);
        assert!(plan.servers[1].enabled);

        fs::remove_file(config_path).unwrap();
    }

    #[test]
    fn loads_codex_server_env_and_env_vars_for_import() {
        let config_path = unique_test_path("codex-import-env.toml");
        fs::write(
            &config_path,
            r#"
                [mcp_servers.demo]
                command = "npx"
                args = ["-y", "demo-server"]
                env_vars = ["DEMO_TOKEN"]

                [mcp_servers.demo.env]
                DEMO_REGION = "global"
            "#,
        )
        .unwrap();

        let plan = load_codex_servers_for_import_from_path(&config_path).unwrap();

        assert_eq!(plan.servers.len(), 1);
        assert_eq!(
            plan.servers[0].env.get("DEMO_REGION"),
            Some(&"global".to_string())
        );
        assert_eq!(plan.servers[0].env_vars, vec!["DEMO_TOKEN".to_string()]);

        fs::remove_file(config_path).unwrap();
    }

    #[test]
    fn loads_codex_remote_server_http_headers_for_import() {
        let config_path = unique_test_path("codex-import-remote.toml");
        fs::write(
            &config_path,
            r#"
                [mcp_servers.demo]
                url = "https://example.com/mcp"

                [mcp_servers.demo.http_headers]
                Authorization = "Bearer secret"
            "#,
        )
        .unwrap();

        let plan = load_codex_servers_for_import_from_path(&config_path).unwrap();

        assert_eq!(plan.servers.len(), 1);
        assert_eq!(
            plan.servers[0].command,
            vec![
                "npx".to_string(),
                "-y".to_string(),
                "mcp-remote".to_string(),
                "https://example.com/mcp".to_string(),
                "--header".to_string(),
                "Authorization: Bearer secret".to_string(),
            ]
        );
        assert!(plan.servers[0].env.is_empty());
        assert!(plan.servers[0].env_vars.is_empty());

        fs::remove_file(config_path).unwrap();
    }

    #[test]
    fn loads_codex_remote_server_bearer_token_env_var_for_import() {
        let config_path = unique_test_path("codex-import-remote-bearer.toml");
        fs::write(
            &config_path,
            r#"
                [mcp_servers.demo]
                url = "https://example.com/mcp"
                bearer_token_env_var = "DEMO_TOKEN"
            "#,
        )
        .unwrap();

        let plan = load_codex_servers_for_import_from_path(&config_path).unwrap();

        assert_eq!(plan.servers.len(), 1);
        assert_eq!(
            plan.servers[0].command,
            vec![
                "npx".to_string(),
                "-y".to_string(),
                "mcp-remote".to_string(),
                "https://example.com/mcp".to_string(),
                "--header".to_string(),
                "Authorization: Bearer ${DEMO_TOKEN}".to_string(),
            ]
        );
        assert_eq!(plan.servers[0].env_vars, vec!["DEMO_TOKEN".to_string()]);

        fs::remove_file(config_path).unwrap();
    }

    #[test]
    fn loads_codex_remote_server_env_http_headers_for_import() {
        let config_path = unique_test_path("codex-import-remote-env-headers.toml");
        fs::write(
            &config_path,
            r#"
                [mcp_servers.demo]
                url = "https://example.com/mcp"

                [mcp_servers.demo.env_http_headers]
                X-Workspace = "DEMO_WORKSPACE"
            "#,
        )
        .unwrap();

        let plan = load_codex_servers_for_import_from_path(&config_path).unwrap();

        assert_eq!(plan.servers.len(), 1);
        assert_eq!(
            plan.servers[0].command,
            vec![
                "npx".to_string(),
                "-y".to_string(),
                "mcp-remote".to_string(),
                "https://example.com/mcp".to_string(),
                "--header".to_string(),
                "X-Workspace: ${DEMO_WORKSPACE}".to_string(),
            ]
        );
        assert_eq!(plan.servers[0].env_vars, vec!["DEMO_WORKSPACE".to_string()]);

        fs::remove_file(config_path).unwrap();
    }

    #[test]
    fn loads_opencode_servers_for_import_from_path() {
        let config_path = unique_test_path("opencode-import.json");
        fs::write(
            &config_path,
            r#"{
                "mcp": {
                    "beta": {
                        "command": ["uvx", "beta-server"],
                        "type": "local"
                    },
                    "alpha": {
                        "command": ["npx", "-y", "@modelcontextprotocol/server-github"],
                        "type": "local"
                    }
                }
            }"#,
        )
        .unwrap();

        let plan = load_opencode_servers_for_import_from_path(&config_path).unwrap();

        assert_eq!(
            plan.servers,
            vec![
                ImportableServer {
                    name: "alpha".to_string(),
                    command: vec![
                        "npx".to_string(),
                        "-y".to_string(),
                        "@modelcontextprotocol/server-github".to_string(),
                    ],
                    enabled: true,
                    env: BTreeMap::new(),
                    env_vars: Vec::new(),
                },
                ImportableServer {
                    name: "beta".to_string(),
                    command: vec!["uvx".to_string(), "beta-server".to_string()],
                    enabled: true,
                    env: BTreeMap::new(),
                    env_vars: Vec::new(),
                },
            ]
        );
        assert!(plan.skipped_self_servers.is_empty());

        fs::remove_file(config_path).unwrap();
    }

    #[test]
    fn loads_opencode_server_environment_for_import() {
        let config_path = unique_test_path("opencode-import-environment.json");
        fs::write(
            &config_path,
            r#"{
                "mcp": {
                    "demo": {
                        "command": ["npx", "-y", "demo-server"],
                        "type": "local",
                        "environment": {
                            "DEMO_REGION": "global"
                        }
                    }
                }
            }"#,
        )
        .unwrap();

        let plan = load_opencode_servers_for_import_from_path(&config_path).unwrap();

        assert_eq!(plan.servers.len(), 1);
        assert_eq!(
            plan.servers[0].env.get("DEMO_REGION"),
            Some(&"global".to_string())
        );
        assert!(plan.servers[0].env_vars.is_empty());

        fs::remove_file(config_path).unwrap();
    }

    #[test]
    fn loads_opencode_remote_headers_for_import() {
        let config_path = unique_test_path("opencode-import-remote.json");
        fs::write(
            &config_path,
            r#"{
                "mcp": {
                    "demo": {
                        "type": "remote",
                        "url": "https://example.com/mcp",
                        "headers": {
                            "Authorization": "Bearer {env:DEMO_TOKEN}"
                        }
                    }
                }
            }"#,
        )
        .unwrap();

        let plan = load_opencode_servers_for_import_from_path(&config_path).unwrap();

        assert_eq!(plan.servers.len(), 1);
        assert_eq!(
            plan.servers[0].command,
            vec![
                "npx".to_string(),
                "-y".to_string(),
                "mcp-remote".to_string(),
                "https://example.com/mcp".to_string(),
                "--header".to_string(),
                "Authorization: Bearer ${DEMO_TOKEN}".to_string(),
            ]
        );
        assert!(plan.servers[0].env.is_empty());
        assert_eq!(plan.servers[0].env_vars, vec!["DEMO_TOKEN".to_string()]);

        fs::remove_file(config_path).unwrap();
    }

    #[test]
    fn preserves_opencode_enabled_state_when_loading_import_plan() {
        let config_path = unique_test_path("opencode-import-enabled.json");
        fs::write(
            &config_path,
            r#"{
                "mcp": {
                    "alpha": {
                        "command": ["npx", "-y", "@modelcontextprotocol/server-github"],
                        "type": "local",
                        "enabled": false
                    },
                    "beta": {
                        "command": ["uvx", "beta-server"],
                        "type": "local",
                        "enabled": true
                    }
                }
            }"#,
        )
        .unwrap();

        let plan = load_opencode_servers_for_import_from_path(&config_path).unwrap();

        assert_eq!(plan.servers.len(), 2);
        assert!(!plan.servers[0].enabled);
        assert!(plan.servers[1].enabled);

        fs::remove_file(config_path).unwrap();
    }

    #[test]
    fn skips_self_server_when_loading_opencode_import_plan() {
        let config_path = unique_test_path("opencode-import-self.json");
        fs::write(
            &config_path,
            r#"{
                "mcp": {
                    "proxy": {
                        "command": ["msp", "mcp"],
                        "type": "local"
                    },
                    "github": {
                        "command": ["npx", "-y", "@modelcontextprotocol/server-github"],
                        "type": "local"
                    }
                }
            }"#,
        )
        .unwrap();

        let plan = load_opencode_servers_for_import_from_path(&config_path).unwrap();

        assert_eq!(
            plan.servers,
            vec![ImportableServer {
                name: "github".to_string(),
                command: vec![
                    "npx".to_string(),
                    "-y".to_string(),
                    "@modelcontextprotocol/server-github".to_string(),
                ],
                enabled: true,
                env: BTreeMap::new(),
                env_vars: Vec::new(),
            }]
        );
        assert_eq!(plan.skipped_self_servers, vec!["proxy".to_string()]);

        fs::remove_file(config_path).unwrap();
    }

    #[test]
    fn rejects_opencode_import_when_server_uses_unsupported_fields() {
        let config_path = unique_test_path("opencode-import-unsupported.json");
        fs::write(
            &config_path,
            r#"{
                "mcp": {
                    "demo": {
                        "command": ["npx", "-y", "demo-server"],
                        "type": "local",
                        "env": {
                            "DEMO_TOKEN": "secret"
                        }
                    }
                }
            }"#,
        )
        .unwrap();

        let error = load_opencode_servers_for_import_from_path(&config_path).unwrap_err();

        assert_eq!(
            error.to_string(),
            "OpenCode MCP server `demo` uses unsupported settings `env`; only `command` and optional `type`, `enabled`, and `environment` can be imported"
        );

        fs::remove_file(config_path).unwrap();
    }

    #[test]
    fn rejects_opencode_import_when_server_type_is_not_local() {
        let config_path = unique_test_path("opencode-import-invalid-type.json");
        fs::write(
            &config_path,
            r#"{
                "mcp": {
                    "demo": {
                        "command": ["npx", "-y", "demo-server"],
                        "type": "stdio"
                    }
                }
            }"#,
        )
        .unwrap();

        let error = load_opencode_servers_for_import_from_path(&config_path).unwrap_err();

        assert_eq!(
            error.to_string(),
            "OpenCode MCP server `demo` uses unsupported type `stdio`, only `local` and `remote` can be imported"
        );

        fs::remove_file(config_path).unwrap();
    }

    #[test]
    fn rejects_opencode_import_when_command_is_not_a_string_array() {
        let config_path = unique_test_path("opencode-import-invalid-command.json");
        fs::write(
            &config_path,
            r#"{
                "mcp": {
                    "demo": {
                        "command": ["npx", 1],
                        "type": "local"
                    }
                }
            }"#,
        )
        .unwrap();

        let error = load_opencode_servers_for_import_from_path(&config_path).unwrap_err();

        assert_eq!(
            error.to_string(),
            "OpenCode MCP server `demo` contains a non-string command part"
        );

        fs::remove_file(config_path).unwrap();
    }

    #[test]
    fn rejects_opencode_import_when_no_servers_are_configured() {
        let config_path = unique_test_path("opencode-import-empty.json");
        fs::write(&config_path, "{}").unwrap();

        let error = load_opencode_servers_for_import_from_path(&config_path).unwrap_err();

        assert_eq!(
            error.to_string(),
            format!(
                "no `mcp` object found in OpenCode config {}",
                config_path.display()
            )
        );

        fs::remove_file(config_path).unwrap();
    }

    #[test]
    fn skips_self_server_when_loading_codex_import_plan() {
        let config_path = unique_test_path("codex-import-self.toml");
        fs::write(
            &config_path,
            r#"
                [mcp_servers.proxy]
                command = "msp"
                args = ["mcp"]

                [mcp_servers.github]
                command = "npx"
                args = ["-y", "@modelcontextprotocol/server-github"]
            "#,
        )
        .unwrap();

        let plan = load_codex_servers_for_import_from_path(&config_path).unwrap();

        assert_eq!(
            plan.servers,
            vec![ImportableServer {
                name: "github".to_string(),
                command: vec![
                    "npx".to_string(),
                    "-y".to_string(),
                    "@modelcontextprotocol/server-github".to_string(),
                ],
                enabled: true,
                env: BTreeMap::new(),
                env_vars: Vec::new(),
            }]
        );
        assert_eq!(plan.skipped_self_servers, vec!["proxy".to_string()]);

        fs::remove_file(config_path).unwrap();
    }

    #[test]
    fn rejects_codex_import_when_server_uses_unsupported_fields() {
        let config_path = unique_test_path("codex-import-unsupported.toml");
        fs::write(
            &config_path,
            r#"
                [mcp_servers.demo]
                command = "npx"
                args = ["-y", "demo-server"]
                cwd = "/tmp/demo"
            "#,
        )
        .unwrap();

        let error = load_codex_servers_for_import_from_path(&config_path).unwrap_err();

        assert_eq!(
            error.to_string(),
            "Codex MCP server `demo` uses unsupported settings `cwd`; only `command`, `args`, optional `enabled`, `env`, `env_vars`, or remote `url` with optional `http_headers`, `bearer_token_env_var`, and `env_http_headers` can be imported"
        );

        fs::remove_file(config_path).unwrap();
    }

    #[test]
    fn rejects_codex_import_when_args_is_not_an_array() {
        let config_path = unique_test_path("codex-import-invalid-args.toml");
        fs::write(
            &config_path,
            r#"
                [mcp_servers.demo]
                command = "npx"
                args = "demo-server"
            "#,
        )
        .unwrap();

        let error = load_codex_servers_for_import_from_path(&config_path).unwrap_err();

        assert_eq!(
            error.to_string(),
            "Codex MCP server `demo` has a non-array `args` field"
        );

        fs::remove_file(config_path).unwrap();
    }

    #[test]
    fn rejects_codex_import_when_no_servers_are_configured() {
        let config_path = unique_test_path("codex-import-empty.toml");
        fs::write(&config_path, "").unwrap();

        let error = load_codex_servers_for_import_from_path(&config_path).unwrap_err();

        assert_eq!(
            error.to_string(),
            format!(
                "no `mcp_servers` table found in Codex config {}",
                config_path.display()
            )
        );

        fs::remove_file(config_path).unwrap();
    }

    #[test]
    fn writes_stdio_server_to_config() {
        let config_path = unique_test_path("write-server-config.toml");
        let server_name = add_server(
            &config_path,
            "ones",
            vec!["https://ones.com/mcp".to_string()],
        )
        .unwrap();
        let config = load_config_table(&config_path).unwrap();

        let saved = config["servers"][&server_name].as_table().unwrap();
        assert_eq!(saved["transport"].as_str(), Some("stdio"));
        assert_eq!(saved["command"].as_str(), Some("npx"));
        assert_eq!(
            saved["args"].as_array().unwrap(),
            &vec![
                Value::String("-y".to_string()),
                Value::String("mcp-remote".to_string()),
                Value::String("https://ones.com/mcp".to_string()),
            ]
        );

        fs::remove_file(config_path).unwrap();
    }

    #[test]
    fn writes_imported_disabled_server_to_config() {
        let config_path = unique_test_path("write-imported-disabled-server-config.toml");
        let server_name = import_server(
            &config_path,
            &ImportableServer {
                name: "ones".to_string(),
                command: vec!["https://ones.com/mcp".to_string()],
                enabled: false,
                env: BTreeMap::new(),
                env_vars: Vec::new(),
            },
        )
        .unwrap();
        let config = load_config_table(&config_path).unwrap();

        let saved = config["servers"][&server_name].as_table().unwrap();
        assert_eq!(saved["enabled"].as_bool(), Some(false));

        fs::remove_file(config_path).unwrap();
    }

    #[test]
    fn writes_imported_server_env_and_env_vars_to_config() {
        let config_path = unique_test_path("write-imported-server-env-config.toml");
        let server_name = import_server(
            &config_path,
            &ImportableServer {
                name: "demo".to_string(),
                command: vec![
                    "npx".to_string(),
                    "-y".to_string(),
                    "demo-server".to_string(),
                ],
                enabled: true,
                env: BTreeMap::from([("DEMO_REGION".to_string(), "global".to_string())]),
                env_vars: vec!["DEMO_TOKEN".to_string()],
            },
        )
        .unwrap();
        let config = load_config_table(&config_path).unwrap();

        let saved = config["servers"][&server_name].as_table().unwrap();
        assert_eq!(saved["env"]["DEMO_REGION"].as_str(), Some("global"));
        assert_eq!(
            saved["env_vars"].as_array().unwrap(),
            &vec![Value::String("DEMO_TOKEN".to_string())]
        );

        fs::remove_file(config_path).unwrap();
    }

    #[test]
    fn loads_server_config_snapshot() {
        let config_path = unique_test_path("load-server-config.toml");
        fs::write(
            &config_path,
            r#"
                [servers.demo]
                transport = "stdio"
                command = "uvx"
                args = ["demo-server"]
                enabled = false
                env_vars = ["DEMO_TOKEN"]

                [servers.demo.env]
                DEMO_REGION = "global"
            "#,
        )
        .unwrap();

        let snapshot = load_server_config(&config_path, "demo").unwrap();

        assert_eq!(
            snapshot,
            ServerConfigSnapshot {
                name: "demo".to_string(),
                transport: "stdio".to_string(),
                enabled: false,
                command: "uvx".to_string(),
                args: vec!["demo-server".to_string()],
                env: BTreeMap::from([("DEMO_REGION".to_string(), "global".to_string())]),
                env_vars: vec!["DEMO_TOKEN".to_string()],
            }
        );

        fs::remove_file(config_path).unwrap();
    }

    #[test]
    fn updates_server_config_fields() {
        let config_path = unique_test_path("update-server-config.toml");
        fs::write(
            &config_path,
            r#"
                [servers.demo]
                transport = "stdio"
                command = "npx"
                args = ["-y", "demo-server"]
                enabled = false
                env_vars = ["OLD_TOKEN"]

                [servers.demo.env]
                OLD_REGION = "legacy"
            "#,
        )
        .unwrap();

        let updated = update_server_config(
            &config_path,
            "demo",
            &UpdateServerConfig {
                transport: Some("stdio".to_string()),
                command: Some("uvx".to_string()),
                clear_args: true,
                add_args: vec!["new-server".to_string()],
                enabled: Some(true),
                clear_env: true,
                set_env: BTreeMap::from([("DEMO_REGION".to_string(), "global".to_string())]),
                unset_env: vec!["OLD_REGION".to_string()],
                clear_env_vars: true,
                add_env_vars: vec!["DEMO_TOKEN".to_string()],
                unset_env_vars: vec!["OLD_TOKEN".to_string()],
            },
        )
        .unwrap();

        assert_eq!(
            updated,
            ServerConfigSnapshot {
                name: "demo".to_string(),
                transport: "stdio".to_string(),
                enabled: true,
                command: "uvx".to_string(),
                args: vec!["new-server".to_string()],
                env: BTreeMap::from([("DEMO_REGION".to_string(), "global".to_string())]),
                env_vars: vec!["DEMO_TOKEN".to_string()],
            }
        );

        fs::remove_file(config_path).unwrap();
    }

    #[test]
    fn appends_server_args_and_env_vars_without_clearing() {
        let config_path = unique_test_path("append-server-config.toml");
        fs::write(
            &config_path,
            r#"
                [servers.demo]
                transport = "stdio"
                command = "uvx"
                args = ["demo-server"]
                env_vars = ["DEMO_TOKEN"]
            "#,
        )
        .unwrap();

        let updated = update_server_config(
            &config_path,
            "demo",
            &UpdateServerConfig {
                add_args: vec!["--verbose".to_string()],
                add_env_vars: vec!["DEMO_TOKEN".to_string(), "DEMO_REGION".to_string()],
                ..UpdateServerConfig::default()
            },
        )
        .unwrap();

        assert_eq!(
            updated.args,
            vec!["demo-server".to_string(), "--verbose".to_string()]
        );
        assert_eq!(
            updated.env_vars,
            vec!["DEMO_TOKEN".to_string(), "DEMO_REGION".to_string()]
        );

        fs::remove_file(config_path).unwrap();
    }

    #[test]
    fn rewrites_remote_header_env_placeholders_for_mcp_remote() {
        let (value, env_vars) = mcp_remote_header_value("Bearer {env:DEMO_TOKEN}");

        assert_eq!(value, "Bearer ${DEMO_TOKEN}");
        assert_eq!(env_vars, vec!["DEMO_TOKEN".to_string()]);
    }

    #[test]
    fn detects_existing_server_name_after_normalization() {
        let config: Table = toml::from_str(
            r#"
                [servers.github-tools]
                transport = "stdio"
                command = "npx"
                args = ["-y", "@modelcontextprotocol/server-github"]
            "#,
        )
        .unwrap();

        assert!(contains_server_name(&config, "GitHub Tools"));
        assert!(!contains_server_name(&config, "filesystem"));
    }

    #[test]
    fn lists_configured_servers_sorted_by_name() {
        let config_path = unique_test_path("list-servers.toml");
        let cache_home = unique_test_path("list-servers-home");

        fs::create_dir_all(&cache_home).unwrap();
        fs::write(
            &config_path,
            r#"
                [servers.beta]
                transport = "stdio"
                command = "uvx"
                args = ["beta-server"]

                [servers.alpha]
                transport = "stdio"
                command = "npx"
                args = ["-y", "@modelcontextprotocol/server-github"]
            "#,
        )
        .unwrap();

        let servers = with_home_env(&cache_home, || list_servers(&config_path).unwrap());

        assert_eq!(
            servers,
            vec![
                ListedServer {
                    name: "alpha".to_string(),
                    command: "npx".to_string(),
                    args: vec![
                        "-y".to_string(),
                        "@modelcontextprotocol/server-github".to_string(),
                    ],
                    enabled: true,
                    last_updated_at: None,
                },
                ListedServer {
                    name: "beta".to_string(),
                    command: "uvx".to_string(),
                    args: vec!["beta-server".to_string()],
                    enabled: true,
                    last_updated_at: None,
                },
            ]
        );

        fs::remove_file(config_path).unwrap();
        fs::remove_dir_all(cache_home).unwrap();
    }

    #[test]
    fn lists_cached_reload_timestamp_when_cache_exists() {
        let config_path = unique_test_path("list-servers-with-cache.toml");
        let cache_home = unique_test_path("list-servers-with-cache-home");

        fs::create_dir_all(cache_home.join(".cache/mcp-smart-proxy")).unwrap();
        fs::write(
            &config_path,
            r#"
                [servers.alpha]
                transport = "stdio"
                command = "npx"
                args = ["-y", "@modelcontextprotocol/server-github"]
            "#,
        )
        .unwrap();

        let cache_path = cache_file_path_from_home(&cache_home, "alpha").unwrap();
        fs::write(
            &cache_path,
            serde_json::to_string(&CachedTools {
                server: "alpha".to_string(),
                summary: "summary".to_string(),
                fetched_at_epoch_ms: 1_742_103_456_000,
                tools: Vec::new(),
            })
            .unwrap(),
        )
        .unwrap();

        let servers = with_home_env(&cache_home, || list_servers(&config_path).unwrap());

        assert_eq!(servers.len(), 1);
        assert!(servers[0].enabled);
        assert_eq!(servers[0].last_updated_at, Some(1_742_103_456_000));

        fs::remove_file(config_path).unwrap();
        fs::remove_dir_all(cache_home).unwrap();
    }

    #[test]
    fn remove_server_deletes_config_entry_and_cache_file() {
        let config_path = unique_test_path("remove-server.toml");
        let cache_home = unique_test_path("remove-server-home");

        fs::create_dir_all(cache_home.join(".cache/mcp-smart-proxy")).unwrap();
        fs::write(
            &config_path,
            r#"
                [servers.github-tools]
                transport = "stdio"
                command = "npx"
                args = ["-y", "@modelcontextprotocol/server-github"]

                [servers.beta]
                transport = "stdio"
                command = "uvx"
                args = ["beta-server"]
            "#,
        )
        .unwrap();
        let cache_path = cache_file_path_from_home(&cache_home, "github-tools").unwrap();
        fs::write(&cache_path, "{}").unwrap();

        let removed = with_home_env(&cache_home, || {
            remove_server(&config_path, "GitHub Tools").unwrap()
        });

        assert_eq!(removed.name, "github-tools");
        assert_eq!(removed.cache_path, cache_path);
        assert!(removed.cache_deleted);
        assert!(!cache_path.exists());

        let config = load_config_table(&config_path).unwrap();
        assert!(
            config["servers"]
                .as_table()
                .unwrap()
                .get("github-tools")
                .is_none()
        );
        assert!(config["servers"].as_table().unwrap().get("beta").is_some());

        fs::remove_file(config_path).unwrap();
        fs::remove_dir_all(cache_home).unwrap();
    }

    #[test]
    fn remove_server_drops_servers_table_when_last_entry_is_removed() {
        let config_path = unique_test_path("remove-last-server.toml");
        let cache_home = unique_test_path("remove-last-server-home");

        fs::create_dir_all(cache_home.join(".cache/mcp-smart-proxy")).unwrap();
        fs::write(
            &config_path,
            r#"
                [servers.github]
                transport = "stdio"
                command = "npx"
                args = ["-y", "@modelcontextprotocol/server-github"]
            "#,
        )
        .unwrap();

        let removed = with_home_env(&cache_home, || {
            remove_server(&config_path, "github").unwrap()
        });

        assert_eq!(removed.name, "github");
        assert!(!removed.cache_deleted);

        let config = load_config_table(&config_path).unwrap();
        assert!(config.get("servers").is_none());

        fs::remove_file(config_path).unwrap();
        fs::remove_dir_all(cache_home).unwrap();
    }

    #[test]
    fn rejects_duplicate_server_name() {
        let config_path = unique_test_path("duplicate-server-config.toml");
        add_server(
            &config_path,
            "ones",
            vec!["https://ones.com/mcp".to_string()],
        )
        .unwrap();

        let error = add_server(
            &config_path,
            "ones",
            vec!["https://example.com/mcp".to_string()],
        )
        .unwrap_err();

        assert_eq!(error.to_string(), "server `ones` already exists");
        fs::remove_file(config_path).unwrap();
    }

    #[test]
    fn writes_server_without_provider_configuration() {
        let config_path = unique_test_path("server-without-provider-config.toml");

        let server_name = add_server(
            &config_path,
            "ones",
            vec!["https://ones.com/mcp".to_string()],
        )
        .unwrap();
        let config = load_config_table(&config_path).unwrap();

        assert_eq!(server_name, "ones");
        assert_eq!(config["servers"]["ones"]["command"].as_str(), Some("npx"));
        assert_eq!(
            config["servers"]["ones"]["args"].as_array().unwrap(),
            &vec![
                Value::String("-y".to_string()),
                Value::String("mcp-remote".to_string()),
                Value::String("https://ones.com/mcp".to_string()),
            ]
        );

        fs::remove_file(config_path).unwrap();
    }

    #[test]
    fn rejects_adding_self_as_server() {
        let config_path = unique_test_path("self-server-config.toml");
        let error = add_server(
            &config_path,
            "proxy",
            vec!["msp".to_string(), "mcp".to_string()],
        )
        .unwrap_err();

        assert_eq!(
            error.to_string(),
            "cannot add `msp mcp` as a managed server"
        );

        assert!(!config_path.exists());
    }

    #[test]
    fn rejects_unsupported_provider_for_model_backed_runtime() {
        let error = load_model_provider_config("anthropic").unwrap_err();

        assert_eq!(
            error.to_string(),
            "unsupported provider `anthropic`; supported providers are `codex` and `opencode`"
        );
    }

    #[test]
    fn loads_codex_provider_runtime_with_default_model() {
        let runtime = load_model_provider_config("codex").unwrap();

        match runtime {
            ModelProviderConfig::Codex(codex) => {
                assert_eq!(codex.model, DEFAULT_MODEL);
            }
            ModelProviderConfig::Opencode(_) => {
                panic!("expected codex provider")
            }
        }
    }

    #[test]
    fn loads_opencode_provider_runtime_with_default_model() {
        let runtime = load_model_provider_config("opencode").unwrap();

        match runtime {
            ModelProviderConfig::Opencode(opencode) => {
                assert_eq!(opencode.model, DEFAULT_OPENCODE_MODEL);
            }
            ModelProviderConfig::Codex(_) => {
                panic!("expected opencode provider")
            }
        }
    }

    #[test]
    fn finds_server_by_exact_or_sanitized_name() {
        let config: Table = toml::from_str(
            r#"
                [servers.my-server]
                transport = "stdio"
                command = "uvx"
                args = ["mcp-server"]
            "#,
        )
        .unwrap();

        let (exact_name, exact_server) = configured_server(&config, "my-server").unwrap();
        assert_eq!(exact_name, "my-server");
        assert_eq!(
            exact_server,
            ConfiguredServer {
                command: "uvx".to_string(),
                args: vec!["mcp-server".to_string()],
                env: BTreeMap::new(),
                env_vars: Vec::new(),
            }
        );

        let (sanitized_name, _) = configured_server(&config, "My Server").unwrap();
        assert_eq!(sanitized_name, "my-server");
    }

    #[test]
    fn lists_disabled_servers() {
        let config_path = unique_test_path("list-disabled-servers.toml");
        fs::write(
            &config_path,
            r#"
                [servers.alpha]
                transport = "stdio"
                command = "npx"
                args = ["-y", "@modelcontextprotocol/server-github"]
                enabled = false
            "#,
        )
        .unwrap();

        let servers = list_servers(&config_path).unwrap();

        assert_eq!(servers.len(), 1);
        assert!(!servers[0].enabled);

        fs::remove_file(config_path).unwrap();
    }

    #[test]
    fn enables_server_by_sanitized_name() {
        let config_path = unique_test_path("enable-server.toml");
        fs::write(
            &config_path,
            r#"
                [servers.my-server]
                transport = "stdio"
                command = "uvx"
                args = ["demo-server"]
                enabled = false
            "#,
        )
        .unwrap();

        let updated = set_server_enabled(&config_path, "My Server", true).unwrap();
        let config = load_config_table(&config_path).unwrap();

        assert_eq!(
            updated,
            SetServerEnabledResult {
                name: "my-server".to_string(),
                enabled: true,
            }
        );
        assert_eq!(
            config["servers"]["my-server"]["enabled"].as_bool(),
            Some(true)
        );

        fs::remove_file(config_path).unwrap();
    }

    #[test]
    fn disables_server_by_exact_name() {
        let config_path = unique_test_path("disable-server.toml");
        fs::write(
            &config_path,
            r#"
                [servers.server1]
                transport = "stdio"
                command = "uvx"
                args = ["demo-server"]
            "#,
        )
        .unwrap();

        let updated = set_server_enabled(&config_path, "server1", false).unwrap();
        let config = load_config_table(&config_path).unwrap();

        assert_eq!(
            updated,
            SetServerEnabledResult {
                name: "server1".to_string(),
                enabled: false,
            }
        );
        assert_eq!(
            config["servers"]["server1"]["enabled"].as_bool(),
            Some(false)
        );

        fs::remove_file(config_path).unwrap();
    }

    #[test]
    fn reads_enabled_state_with_default_true() {
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
                enabled = false
            "#,
        )
        .unwrap();

        assert!(server_is_enabled(&config, "alpha").unwrap());
        assert!(!server_is_enabled(&config, "beta").unwrap());
    }

    #[test]
    fn builds_cache_file_path_under_default_cache_dir() {
        let home = PathBuf::from("/tmp/mcp-smart-proxy-cache-home");
        let path = cache_file_path_from_home(&home, "demo-server").unwrap();

        assert_eq!(path, home.join(".cache/mcp-smart-proxy/demo-server.json"));
    }

    fn unique_test_path(name: &str) -> PathBuf {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();

        env::temp_dir().join(format!("mcp-smart-proxy-{unique}-{name}"))
    }
}
