use std::env;
use std::error::Error;
use std::fs;
use std::path::{Path, PathBuf};

use toml::{Table, Value};

use crate::paths::{cache_file_path, expand_tilde, sanitize_name};
use crate::types::{
    CodexRuntimeConfig, ConfiguredServer, ModelProviderConfig, OpenAiRuntimeConfig,
};

const DEFAULT_OPENAI_MODEL: &str = "gpt-5.2";
const DEFAULT_CODEX_CONFIG_PATH: &str = "~/.codex/config.toml";
const CODEX_HOME_ENV: &str = "CODEX_HOME";
const DEFAULT_PROVIDER_KEY: &str = "default_provider";
const CODEX_PROVIDER_NAME: &str = "codex";
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
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CodexImportPlan {
    pub servers: Vec<ImportableServer>,
    pub skipped_self_servers: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemovedServer {
    pub name: String,
    pub cache_path: PathBuf,
    pub cache_deleted: bool,
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

pub fn add_server(
    config_path: &Path,
    name: &str,
    raw_command: Vec<String>,
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
    load_default_model_provider_config(&config)?;
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

            Ok(ListedServer {
                name,
                command,
                args,
            })
        })
        .collect()
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

pub fn load_codex_servers_for_import() -> Result<(PathBuf, CodexImportPlan), Box<dyn Error>> {
    let path = codex_config_path()?;
    let plan = load_codex_servers_for_import_from_path(&path)?;
    Ok((path, plan))
}

pub fn load_openai_runtime_config(config: &Table) -> Result<OpenAiRuntimeConfig, Box<dyn Error>> {
    let table = config.get("openai").and_then(Value::as_table);

    let baseurl = table_optional_string(table, "baseurl", Some(OPENAI_API_BASE_ENV));
    let key = openai_string(table, "key", Some(OPENAI_API_KEY_ENV))?;
    let model = table_optional_string(table, "model", None)
        .unwrap_or_else(|| DEFAULT_OPENAI_MODEL.to_string());

    Ok(OpenAiRuntimeConfig {
        baseurl,
        key,
        model,
    })
}

pub fn load_codex_runtime_config(config: &Table) -> Result<CodexRuntimeConfig, Box<dyn Error>> {
    let table = config.get(CODEX_PROVIDER_NAME).and_then(Value::as_table);
    let model = table_optional_string(table, "model", None)
        .unwrap_or_else(|| DEFAULT_OPENAI_MODEL.to_string());

    Ok(CodexRuntimeConfig { model })
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

    match provider {
        OPENAI_PROVIDER_NAME => load_openai_runtime_config(config).map(ModelProviderConfig::OpenAi),
        CODEX_PROVIDER_NAME => load_codex_runtime_config(config).map(ModelProviderConfig::Codex),
        _ => Err(format!(
            "unsupported `default_provider` `{provider}`; supported providers are `openai` and `codex`"
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

fn load_codex_servers_for_import_from_path(path: &Path) -> Result<CodexImportPlan, Box<dyn Error>> {
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

    Ok(CodexImportPlan {
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

        let servers = list_servers(&config_path).unwrap();

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
                },
                ListedServer {
                    name: "beta".to_string(),
                    command: "uvx".to_string(),
                    args: vec!["beta-server".to_string()],
                },
            ]
        );

        fs::remove_file(config_path).unwrap();
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
            assert_eq!(runtime.model, DEFAULT_OPENAI_MODEL);
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
                "unsupported `default_provider` `anthropic`; supported providers are `openai` and `codex`"
            );
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
                ModelProviderConfig::Codex(_) => panic!("expected openai provider"),
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
                assert_eq!(codex.model, DEFAULT_OPENAI_MODEL);
            }
            ModelProviderConfig::OpenAi(_) => panic!("expected codex provider"),
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
