use std::env;
use std::error::Error;
use std::fs;
use std::path::{Path, PathBuf};

use serde_json::{Map as JsonMap, Value as JsonValue};
use toml::{Table, Value};

use crate::paths::{cache_file_path, expand_tilde, sanitize_name, sibling_backup_path};
use crate::types::{
    CachedTools, CodexRuntimeConfig, ConfiguredServer, ModelProviderConfig, OpenAiRuntimeConfig,
    OpencodeRuntimeConfig,
};

const DEFAULT_MODEL: &str = "gpt-5.2";
const DEFAULT_OPENCODE_MODEL: &str = "openai/gpt-5.2";
const DEFAULT_CODEX_CONFIG_PATH: &str = "~/.codex/config.toml";
const DEFAULT_OPENCODE_CONFIG_PATH: &str = "~/.config/opencode/opencode.json";
const CODEX_HOME_ENV: &str = "CODEX_HOME";
const DEFAULT_PROVIDER_KEY: &str = "default_provider";
const CODEX_PROVIDER_NAME: &str = "codex";
const OPENCODE_PROVIDER_NAME: &str = "opencode";
const OPENAI_API_BASE_ENV: &str = "OPENAI_API_BASE";
const OPENAI_API_KEY_ENV: &str = "OPENAI_API_KEY";
const OPENAI_PROVIDER_NAME: &str = "openai";
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
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ListedServer {
    pub name: String,
    pub command: String,
    pub args: Vec<String>,
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

pub struct OpenAiConfigUpdate {
    pub baseurl: Option<String>,
    pub key: Option<String>,
    pub model: Option<String>,
    pub make_default: bool,
}

pub struct CodexConfigUpdate {
    pub model: Option<String>,
    pub make_default: bool,
}

pub struct OpencodeConfigUpdate {
    pub model: Option<String>,
    pub make_default: bool,
}

pub fn add_server(
    config_path: &Path,
    name: &str,
    raw_command: Vec<String>,
) -> Result<String, Box<dyn Error>> {
    save_server(config_path, name, raw_command, true)
}

pub fn import_server(
    config_path: &Path,
    name: &str,
    raw_command: Vec<String>,
) -> Result<String, Box<dyn Error>> {
    save_server(config_path, name, raw_command, false)
}

fn save_server(
    config_path: &Path,
    name: &str,
    raw_command: Vec<String>,
    require_default_provider: bool,
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
    if require_default_provider {
        load_default_model_provider_config(&config)?;
    }
    if has_server_name(&config, &name) {
        return Err(format!("server `{name}` already exists").into());
    }

    insert_server(&mut config, &name, &server)?;
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

            let last_updated_at = read_cached_tools_timestamp(&name);

            Ok(ListedServer {
                name,
                command,
                args,
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

    Ok((resolved_name, ConfiguredServer { command, args }))
}

pub fn update_openai_config(
    config_path: &Path,
    update: OpenAiConfigUpdate,
) -> Result<(), Box<dyn Error>> {
    let mut config = load_config_table(config_path)?;
    let openai_value = config
        .entry("openai")
        .or_insert_with(|| Value::Table(Table::new()));
    let openai = openai_value
        .as_table_mut()
        .ok_or_else(|| "`openai` in config must be a table".to_string())?;

    set_optional_string(openai, "baseurl", update.baseurl);
    set_optional_string(openai, "key", update.key);
    set_optional_string(openai, "model", update.model);
    if update.make_default {
        config.insert(
            DEFAULT_PROVIDER_KEY.to_string(),
            Value::String(OPENAI_PROVIDER_NAME.to_string()),
        );
    }

    save_config_table(config_path, &config)?;
    Ok(())
}

pub fn update_codex_config(
    config_path: &Path,
    update: CodexConfigUpdate,
) -> Result<(), Box<dyn Error>> {
    let mut config = load_config_table(config_path)?;
    let codex_value = config
        .entry(CODEX_PROVIDER_NAME)
        .or_insert_with(|| Value::Table(Table::new()));
    let codex = codex_value
        .as_table_mut()
        .ok_or_else(|| "`codex` in config must be a table".to_string())?;

    set_optional_string(codex, "model", update.model);
    if update.make_default {
        config.insert(
            DEFAULT_PROVIDER_KEY.to_string(),
            Value::String(CODEX_PROVIDER_NAME.to_string()),
        );
    }

    save_config_table(config_path, &config)?;
    Ok(())
}

pub fn update_opencode_config(
    config_path: &Path,
    update: OpencodeConfigUpdate,
) -> Result<(), Box<dyn Error>> {
    let mut config = load_config_table(config_path)?;
    let opencode_value = config
        .entry(OPENCODE_PROVIDER_NAME)
        .or_insert_with(|| Value::Table(Table::new()));
    let opencode = opencode_value
        .as_table_mut()
        .ok_or_else(|| "`opencode` in config must be a table".to_string())?;

    set_optional_string(opencode, "model", update.model);
    if update.make_default {
        config.insert(
            DEFAULT_PROVIDER_KEY.to_string(),
            Value::String(OPENCODE_PROVIDER_NAME.to_string()),
        );
    }

    save_config_table(config_path, &config)?;
    Ok(())
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

pub fn load_openai_runtime_config(config: &Table) -> Result<OpenAiRuntimeConfig, Box<dyn Error>> {
    let table = config.get("openai").and_then(Value::as_table);

    let baseurl = table_optional_string(table, "baseurl", Some(OPENAI_API_BASE_ENV));
    let key = openai_string(table, "key", Some(OPENAI_API_KEY_ENV))?;
    let model =
        table_optional_string(table, "model", None).unwrap_or_else(|| DEFAULT_MODEL.to_string());

    Ok(OpenAiRuntimeConfig {
        baseurl,
        key,
        model,
    })
}

pub fn load_codex_runtime_config(config: &Table) -> Result<CodexRuntimeConfig, Box<dyn Error>> {
    let table = config.get(CODEX_PROVIDER_NAME).and_then(Value::as_table);
    let model =
        table_optional_string(table, "model", None).unwrap_or_else(|| DEFAULT_MODEL.to_string());

    Ok(CodexRuntimeConfig { model })
}

pub fn load_opencode_runtime_config(
    config: &Table,
) -> Result<OpencodeRuntimeConfig, Box<dyn Error>> {
    let table = config.get(OPENCODE_PROVIDER_NAME).and_then(Value::as_table);
    let model = table_optional_string(table, "model", None)
        .unwrap_or_else(|| DEFAULT_OPENCODE_MODEL.to_string());

    Ok(OpencodeRuntimeConfig { model })
}

pub fn load_default_model_provider_config(
    config: &Table,
) -> Result<ModelProviderConfig, Box<dyn Error>> {
    let provider = config
        .get(DEFAULT_PROVIDER_KEY)
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| {
            "missing `default_provider` in config; model-backed commands cannot run".to_string()
        })?;

    load_model_provider_config(config, provider)
}

pub fn load_model_provider_config(
    config: &Table,
    provider: &str,
) -> Result<ModelProviderConfig, Box<dyn Error>> {
    match provider {
        OPENAI_PROVIDER_NAME => load_openai_runtime_config(config).map(ModelProviderConfig::OpenAi),
        CODEX_PROVIDER_NAME => load_codex_runtime_config(config).map(ModelProviderConfig::Codex),
        OPENCODE_PROVIDER_NAME => {
            load_opencode_runtime_config(config).map(ModelProviderConfig::Opencode)
        }
        _ => Err(format!(
            "unsupported provider `{provider}`; supported providers are `openai`, `codex`, and `opencode`"
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
        return vec![
            "npx".to_string(),
            "-y".to_string(),
            "mcp-remote".to_string(),
            raw_command[0].clone(),
        ];
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

        let command = server
            .get("command")
            .and_then(Value::as_str)
            .ok_or_else(|| format!("Codex MCP server `{name}` is missing `command`"))?
            .to_string();
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

        let mut raw_command = vec![command];
        raw_command.extend(args);

        if is_self_server_command(&raw_command) {
            skipped_self_servers.push(name);
            continue;
        }

        importable_servers.push(ImportableServer {
            name,
            command: raw_command,
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

        if is_self_server_command(&raw_command) {
            skipped_self_servers.push(name);
            continue;
        }

        importable_servers.push(ImportableServer {
            name,
            command: raw_command,
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

    servers.insert(name.to_string(), Value::Table(server_table));
    Ok(())
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

fn set_optional_string(table: &mut Table, key: &str, value: Option<String>) {
    if let Some(value) = value {
        table.insert(key.to_string(), Value::String(value));
    }
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
        .filter(|key| !matches!(key.as_str(), "command" | "args"))
        .map(|key| format!("`{key}`"))
        .collect::<Vec<_>>();

    if unsupported_keys.is_empty() {
        return Ok(());
    }

    Err(format!(
        "Codex MCP server `{name}` uses unsupported settings {}; only `command` and `args` can be imported",
        unsupported_keys.join(", ")
    )
    .into())
}

fn validate_importable_opencode_server(
    name: &str,
    server: &serde_json::Map<String, serde_json::Value>,
) -> Result<(), Box<dyn Error>> {
    let unsupported_keys = server
        .keys()
        .filter(|key| !matches!(key.as_str(), "command" | "type"))
        .map(|key| format!("`{key}`"))
        .collect::<Vec<_>>();

    if !unsupported_keys.is_empty() {
        return Err(format!(
            "OpenCode MCP server `{name}` uses unsupported settings {}; only `command` and optional `type` can be imported",
            unsupported_keys.join(", ")
        )
        .into());
    }

    let Some(server_type) = server.get("type") else {
        return Ok(());
    };

    match server_type.as_str() {
        Some("local") => Ok(()),
        Some(other) => Err(format!(
            "OpenCode MCP server `{name}` uses unsupported type `{other}`, only `local` can be imported"
        )
        .into()),
        None => Err(format!("OpenCode MCP server `{name}` has a non-string `type` field").into()),
    }
}

fn looks_like_url(value: &str) -> bool {
    value.starts_with("http://") || value.starts_with("https://")
}

fn openai_string(
    table: Option<&Table>,
    key: &str,
    env_key: Option<&str>,
) -> Result<String, Box<dyn Error>> {
    table_optional_string(table, key, env_key).ok_or_else(|| {
        let message = match env_key {
            Some(env_key) => {
                format!("missing `openai.{key}` in config and `{env_key}` in environment")
            }
            None => format!("missing `openai.{key}` in config"),
        };

        message.into()
    })
}

fn table_optional_string(
    table: Option<&Table>,
    key: &str,
    env_key: Option<&str>,
) -> Option<String> {
    if let Some(value) = table
        .and_then(|table| table.get(key))
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
    {
        return Some(value.to_string());
    }

    env_key.and_then(|env_key| match env::var(env_key) {
        Ok(value) if !value.is_empty() => Some(value),
        _ => None,
    })
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

    fn with_openai_env<T>(base: Option<&str>, key: Option<&str>, test: impl FnOnce() -> T) -> T {
        let _guard = env_lock().lock().unwrap();
        let previous_base = env::var(OPENAI_API_BASE_ENV).ok();
        let previous_key = env::var(OPENAI_API_KEY_ENV).ok();

        match base {
            Some(value) => unsafe { env::set_var(OPENAI_API_BASE_ENV, value) },
            None => unsafe { env::remove_var(OPENAI_API_BASE_ENV) },
        }
        match key {
            Some(value) => unsafe { env::set_var(OPENAI_API_KEY_ENV, value) },
            None => unsafe { env::remove_var(OPENAI_API_KEY_ENV) },
        }

        let result = test();

        match previous_base {
            Some(value) => unsafe { env::set_var(OPENAI_API_BASE_ENV, value) },
            None => unsafe { env::remove_var(OPENAI_API_BASE_ENV) },
        }
        match previous_key {
            Some(value) => unsafe { env::set_var(OPENAI_API_KEY_ENV, value) },
            None => unsafe { env::remove_var(OPENAI_API_KEY_ENV) },
        }

        result
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
                },
                ImportableServer {
                    name: "beta".to_string(),
                    command: vec!["uvx".to_string(), "beta-server".to_string()],
                },
            ]
        );
        assert!(plan.skipped_self_servers.is_empty());

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
                },
                ImportableServer {
                    name: "beta".to_string(),
                    command: vec!["uvx".to_string(), "beta-server".to_string()],
                },
            ]
        );
        assert!(plan.skipped_self_servers.is_empty());

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
            "OpenCode MCP server `demo` uses unsupported settings `env`; only `command` and optional `type` can be imported"
        );

        fs::remove_file(config_path).unwrap();
    }

    #[test]
    fn rejects_opencode_import_when_server_type_is_not_local() {
        let config_path = unique_test_path("opencode-import-remote.json");
        fs::write(
            &config_path,
            r#"{
                "mcp": {
                    "demo": {
                        "command": ["npx", "-y", "demo-server"],
                        "type": "remote"
                    }
                }
            }"#,
        )
        .unwrap();

        let error = load_opencode_servers_for_import_from_path(&config_path).unwrap_err();

        assert_eq!(
            error.to_string(),
            "OpenCode MCP server `demo` uses unsupported type `remote`, only `local` can be imported"
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

                [mcp_servers.demo.env]
                DEMO_TOKEN = "secret"
            "#,
        )
        .unwrap();

        let error = load_codex_servers_for_import_from_path(&config_path).unwrap_err();

        assert_eq!(
            error.to_string(),
            "Codex MCP server `demo` uses unsupported settings `env`; only `command` and `args` can be imported"
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
        update_codex_config(
            &config_path,
            CodexConfigUpdate {
                model: None,
                make_default: true,
            },
        )
        .unwrap();
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
                    last_updated_at: None,
                },
                ListedServer {
                    name: "beta".to_string(),
                    command: "uvx".to_string(),
                    args: vec!["beta-server".to_string()],
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
        update_codex_config(
            &config_path,
            CodexConfigUpdate {
                model: None,
                make_default: true,
            },
        )
        .unwrap();
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
    fn rejects_add_when_default_provider_is_missing() {
        let config_path = unique_test_path("missing-default-provider.toml");

        let error = add_server(
            &config_path,
            "ones",
            vec!["https://ones.com/mcp".to_string()],
        )
        .unwrap_err();

        assert_eq!(
            error.to_string(),
            "missing `default_provider` in config; model-backed commands cannot run"
        );
        assert!(!config_path.exists());
    }

    #[test]
    fn allows_import_when_default_provider_is_missing() {
        let config_path = unique_test_path("import-without-default-provider.toml");

        let server_name = import_server(
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
        update_codex_config(
            &config_path,
            CodexConfigUpdate {
                model: None,
                make_default: true,
            },
        )
        .unwrap();

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

        fs::remove_file(config_path).unwrap();
    }

    #[test]
    fn updates_openai_config_with_partial_fields() {
        let config_path = unique_test_path("openai-config.toml");
        update_openai_config(
            &config_path,
            OpenAiConfigUpdate {
                baseurl: Some("https://api.example.com/v1".to_string()),
                key: None,
                model: Some("gpt-4.1-mini".to_string()),
                make_default: false,
            },
        )
        .unwrap();

        let config = load_config_table(&config_path).unwrap();
        let openai = config["openai"].as_table().unwrap();

        assert_eq!(
            openai["baseurl"].as_str(),
            Some("https://api.example.com/v1")
        );
        assert_eq!(openai["model"].as_str(), Some("gpt-4.1-mini"));
        assert!(openai.get("key").is_none());

        fs::remove_file(config_path).unwrap();
    }

    #[test]
    fn updates_codex_config_with_partial_fields() {
        let config_path = unique_test_path("codex-config.toml");
        update_codex_config(
            &config_path,
            CodexConfigUpdate {
                model: Some("gpt-5.2".to_string()),
                make_default: false,
            },
        )
        .unwrap();

        let config = load_config_table(&config_path).unwrap();
        let codex = config["codex"].as_table().unwrap();

        assert_eq!(codex["model"].as_str(), Some("gpt-5.2"));

        fs::remove_file(config_path).unwrap();
    }

    #[test]
    fn updates_opencode_config_with_partial_fields() {
        let config_path = unique_test_path("opencode-config.toml");
        update_opencode_config(
            &config_path,
            OpencodeConfigUpdate {
                model: Some("openai/gpt-5".to_string()),
                make_default: false,
            },
        )
        .unwrap();

        let config = load_config_table(&config_path).unwrap();
        let opencode = config["opencode"].as_table().unwrap();

        assert_eq!(opencode["model"].as_str(), Some("openai/gpt-5"));

        fs::remove_file(config_path).unwrap();
    }

    #[test]
    fn preserves_existing_openai_fields_when_updating_subset() {
        let config_path = unique_test_path("openai-config-preserve.toml");
        update_openai_config(
            &config_path,
            OpenAiConfigUpdate {
                baseurl: Some("https://api.example.com/v1".to_string()),
                key: Some("sk-old".to_string()),
                model: Some("gpt-4.1".to_string()),
                make_default: false,
            },
        )
        .unwrap();
        update_openai_config(
            &config_path,
            OpenAiConfigUpdate {
                baseurl: None,
                key: Some("sk-new".to_string()),
                model: None,
                make_default: false,
            },
        )
        .unwrap();

        let config = load_config_table(&config_path).unwrap();
        let openai = config["openai"].as_table().unwrap();

        assert_eq!(
            openai["baseurl"].as_str(),
            Some("https://api.example.com/v1")
        );
        assert_eq!(openai["key"].as_str(), Some("sk-new"));
        assert_eq!(openai["model"].as_str(), Some("gpt-4.1"));

        fs::remove_file(config_path).unwrap();
    }

    #[test]
    fn sets_openai_as_default_provider_when_requested() {
        let config_path = unique_test_path("openai-config-default.toml");
        update_openai_config(
            &config_path,
            OpenAiConfigUpdate {
                baseurl: None,
                key: Some("sk-default".to_string()),
                model: None,
                make_default: true,
            },
        )
        .unwrap();

        let config = load_config_table(&config_path).unwrap();

        assert_eq!(config["default_provider"].as_str(), Some("openai"));

        fs::remove_file(config_path).unwrap();
    }

    #[test]
    fn sets_codex_as_default_provider_when_requested() {
        let config_path = unique_test_path("codex-config-default.toml");
        update_codex_config(
            &config_path,
            CodexConfigUpdate {
                model: Some("gpt-5.2".to_string()),
                make_default: true,
            },
        )
        .unwrap();

        let config = load_config_table(&config_path).unwrap();

        assert_eq!(config["default_provider"].as_str(), Some("codex"));

        fs::remove_file(config_path).unwrap();
    }

    #[test]
    fn sets_opencode_as_default_provider_when_requested() {
        let config_path = unique_test_path("opencode-config-default.toml");
        update_opencode_config(
            &config_path,
            OpencodeConfigUpdate {
                model: Some("openai/gpt-5".to_string()),
                make_default: true,
            },
        )
        .unwrap();

        let config = load_config_table(&config_path).unwrap();

        assert_eq!(config["default_provider"].as_str(), Some("opencode"));

        fs::remove_file(config_path).unwrap();
    }

    #[test]
    fn loads_openai_base_and_key_from_environment_when_config_is_missing_them() {
        with_openai_env(Some("https://env.example.com/v1"), Some("sk-env"), || {
            let config: Table = toml::from_str(
                r#"
                        default_provider = "openai"

                        [openai]
                        model = "gpt-4.1-mini"
                    "#,
            )
            .unwrap();

            let runtime = load_openai_runtime_config(&config).unwrap();

            assert_eq!(
                runtime.baseurl.as_deref(),
                Some("https://env.example.com/v1")
            );
            assert_eq!(runtime.key, "sk-env");
            assert_eq!(runtime.model, "gpt-4.1-mini");
        });
    }

    #[test]
    fn prefers_openai_config_file_over_environment_variables() {
        with_openai_env(Some("https://env.example.com/v1"), Some("sk-env"), || {
            let config: Table = toml::from_str(
                r#"
                        default_provider = "openai"

                        [openai]
                        baseurl = "https://config.example.com/v1"
                        key = "sk-config"
                        model = "gpt-4.1"
                    "#,
            )
            .unwrap();

            let runtime = load_openai_runtime_config(&config).unwrap();

            assert_eq!(
                runtime.baseurl.as_deref(),
                Some("https://config.example.com/v1")
            );
            assert_eq!(runtime.key, "sk-config");
            assert_eq!(runtime.model, "gpt-4.1");
        });
    }

    #[test]
    fn allows_missing_openai_baseurl_when_no_config_or_env_value_exists() {
        with_openai_env(None, Some("sk-env"), || {
            let config: Table = toml::from_str(
                r#"
                    default_provider = "openai"

                    [openai]
                    model = "gpt-4.1-mini"
                "#,
            )
            .unwrap();

            let runtime = load_openai_runtime_config(&config).unwrap();

            assert_eq!(runtime.baseurl, None);
            assert_eq!(runtime.key, "sk-env");
            assert_eq!(runtime.model, "gpt-4.1-mini");
        });
    }

    #[test]
    fn uses_default_openai_model_when_config_is_missing_it() {
        with_openai_env(None, Some("sk-env"), || {
            let config: Table = toml::from_str(
                r#"
                    default_provider = "openai"

                    [openai]
                "#,
            )
            .unwrap();

            let runtime = load_openai_runtime_config(&config).unwrap();

            assert_eq!(runtime.baseurl, None);
            assert_eq!(runtime.key, "sk-env");
            assert_eq!(runtime.model, DEFAULT_MODEL);
        });
    }

    #[test]
    fn requires_openai_key_in_config_or_environment() {
        with_openai_env(None, None, || {
            let config: Table = toml::from_str(
                r#"
                    default_provider = "openai"

                    [openai]
                "#,
            )
            .unwrap();

            let error = load_openai_runtime_config(&config).unwrap_err();

            assert_eq!(
                error.to_string(),
                "missing `openai.key` in config and `OPENAI_API_KEY` in environment"
            );
        });
    }

    #[test]
    fn requires_default_provider_for_model_backed_runtime() {
        with_openai_env(None, Some("sk-env"), || {
            let config: Table = toml::from_str(
                r#"
                    [openai]
                "#,
            )
            .unwrap();

            let error = load_default_model_provider_config(&config).unwrap_err();

            assert_eq!(
                error.to_string(),
                "missing `default_provider` in config; model-backed commands cannot run"
            );
        });
    }

    #[test]
    fn rejects_unsupported_default_provider_for_model_backed_runtime() {
        with_openai_env(None, Some("sk-env"), || {
            let config: Table = toml::from_str(
                r#"
                    default_provider = "anthropic"

                    [openai]
                "#,
            )
            .unwrap();

            let error = load_default_model_provider_config(&config).unwrap_err();

            assert_eq!(
                error.to_string(),
                "unsupported provider `anthropic`; supported providers are `openai`, `codex`, and `opencode`"
            );
        });
    }

    #[test]
    fn loads_explicit_provider_runtime_without_default_provider() {
        with_openai_env(None, Some("sk-env"), || {
            let config: Table = toml::from_str(
                r#"
                    [openai]
                    model = "gpt-4.1-mini"
                "#,
            )
            .unwrap();

            let runtime = load_model_provider_config(&config, "openai").unwrap();

            match runtime {
                ModelProviderConfig::OpenAi(openai) => {
                    assert_eq!(openai.model, "gpt-4.1-mini");
                    assert_eq!(openai.key, "sk-env");
                }
                ModelProviderConfig::Codex(_) | ModelProviderConfig::Opencode(_) => {
                    panic!("expected openai provider")
                }
            }
        });
    }

    #[test]
    fn loads_default_openai_provider_runtime() {
        with_openai_env(None, Some("sk-env"), || {
            let config: Table = toml::from_str(
                r#"
                    default_provider = "openai"

                    [openai]
                    model = "gpt-4.1-mini"
                "#,
            )
            .unwrap();

            let runtime = load_default_model_provider_config(&config).unwrap();

            match runtime {
                ModelProviderConfig::OpenAi(openai) => {
                    assert_eq!(openai.model, "gpt-4.1-mini");
                    assert_eq!(openai.key, "sk-env");
                }
                ModelProviderConfig::Codex(_) | ModelProviderConfig::Opencode(_) => {
                    panic!("expected openai provider")
                }
            }
        });
    }

    #[test]
    fn loads_default_codex_provider_runtime_with_default_model() {
        let config: Table = toml::from_str(
            r#"
                default_provider = "codex"
            "#,
        )
        .unwrap();

        let runtime = load_default_model_provider_config(&config).unwrap();

        match runtime {
            ModelProviderConfig::Codex(codex) => {
                assert_eq!(codex.model, DEFAULT_MODEL);
            }
            ModelProviderConfig::OpenAi(_) | ModelProviderConfig::Opencode(_) => {
                panic!("expected codex provider")
            }
        }
    }

    #[test]
    fn loads_default_opencode_provider_runtime_with_default_model() {
        let config: Table = toml::from_str(
            r#"
                default_provider = "opencode"
            "#,
        )
        .unwrap();

        let runtime = load_default_model_provider_config(&config).unwrap();

        match runtime {
            ModelProviderConfig::Opencode(opencode) => {
                assert_eq!(opencode.model, DEFAULT_OPENCODE_MODEL);
            }
            ModelProviderConfig::OpenAi(_) | ModelProviderConfig::Codex(_) => {
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
            }
        );

        let (sanitized_name, _) = configured_server(&config, "My Server").unwrap();
        assert_eq!(sanitized_name, "my-server");
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
