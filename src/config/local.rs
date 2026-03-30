use std::collections::BTreeMap;
use std::error::Error;
use std::fs;
use std::path::Path;

use toml::{Table, Value};

use crate::fs_util::{acquire_sibling_lock, write_file_atomically};
use crate::paths::{cache_file_path, sanitize_name};
use crate::types::{CachedTools, ConfiguredServer, ConfiguredTransport};

use super::{
    ImportableServer, RemovedServer, ServerConfigSnapshot, SetServerEnabledResult, StdioServer,
    UpdateServerConfig, is_self_server_command,
};

mod lookup;
mod schema;
mod update;

pub(crate) use lookup::{
    has_server_name, resolve_server_name, resolved_server_table, resolved_server_table_mut,
};
pub(crate) use schema::{
    ParsedServerTransport, parse_remote_server_url, parse_server_enabled, parse_server_entry,
    resolved_server_transport, server_config_snapshot,
};
pub use update::update_server_config;

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

pub fn list_servers(config_path: &Path) -> Result<Vec<super::ListedServer>, Box<dyn Error>> {
    let config = load_config_table(config_path)?;
    let Some(servers) = config.get("servers").and_then(Value::as_table) else {
        return Ok(Vec::new());
    };

    let mut names = servers.keys().cloned().collect::<Vec<_>>();
    names.sort();

    names
        .into_iter()
        .map(|name| {
            let server = servers[&name]
                .as_table()
                .ok_or_else(|| format!("server `{name}` must be a table"))?;
            let parsed = parse_server_entry(server, &name)?;
            let (command, args) = match parsed.transport {
                ParsedServerTransport::Stdio { command, args } => (command, args),
                ParsedServerTransport::Remote { url, .. } => (url, Vec::new()),
            };
            let last_updated_at = read_cached_tools_timestamp(&name);

            Ok(super::ListedServer {
                name,
                command,
                args,
                enabled: parsed.enabled,
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
    let normalized_name = sanitize_name(requested_name);
    let cache_lock = if normalized_name.is_empty() {
        None
    } else {
        Some(acquire_sibling_lock(&cache_file_path(&normalized_name)?)?)
    };
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
    drop(cache_lock);

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
    let parsed = parse_server_entry(server, &resolved_name)?;
    let configured_transport = match parsed.transport {
        ParsedServerTransport::Stdio { command, args } => {
            ConfiguredTransport::Stdio { command, args }
        }
        ParsedServerTransport::Remote { url, headers } => {
            ConfiguredTransport::Remote { url, headers }
        }
    };

    Ok((
        resolved_name,
        ConfiguredServer {
            transport: configured_transport,
            env: parsed.env,
            env_vars: parsed.env_vars,
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

pub fn contains_server_name(config: &Table, requested_name: &str) -> bool {
    let normalized = sanitize_name(requested_name);
    if normalized.is_empty() {
        return false;
    }

    has_server_name(config, &normalized)
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

pub(crate) fn parse_toml_string_table(
    value: Option<&Value>,
    field_name: &str,
    kind: &str,
    name: &str,
) -> Result<BTreeMap<String, String>, Box<dyn Error>> {
    match value {
        None => Ok(BTreeMap::new()),
        Some(Value::Table(table)) => table
            .iter()
            .map(|(key, raw_value)| match raw_value.as_str() {
                Some(string_value) => Ok((key.clone(), string_value.to_string())),
                None => Err(format!(
                    "{kind} `{name}` contains a non-string `{field_name}` value `{key}`"
                )),
            })
            .collect::<Result<BTreeMap<_, _>, _>>()
            .map_err(Into::into),
        Some(_) => Err(format!("{kind} `{name}` has a non-table `{field_name}` field").into()),
    }
}

pub(crate) fn parse_toml_string_array(
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

pub(crate) fn parse_json_string_object(
    value: Option<&serde_json::Value>,
    field_name: &str,
    kind: &str,
    name: &str,
) -> Result<BTreeMap<String, String>, Box<dyn Error>> {
    match value {
        None => Ok(BTreeMap::new()),
        Some(serde_json::Value::Object(map)) => map
            .iter()
            .map(|(key, raw_value)| match raw_value.as_str() {
                Some(string_value) => Ok((key.clone(), string_value.to_string())),
                None => Err(format!(
                    "{kind} `{name}` contains a non-string `{field_name}` value `{key}`"
                )),
            })
            .collect::<Result<BTreeMap<_, _>, _>>()
            .map_err(Into::into),
        Some(_) => Err(format!("{kind} `{name}` has a non-object `{field_name}` field").into()),
    }
}

pub(crate) fn merge_env_vars(target: &mut Vec<String>, additions: Vec<String>) {
    for name in additions {
        if !target.contains(&name) {
            target.push(name);
        }
    }
}

fn looks_like_url(value: &str) -> bool {
    value.starts_with("http://") || value.starts_with("https://")
}
