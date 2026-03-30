use std::collections::BTreeMap;
use std::error::Error;
use std::fs;
use std::path::Path;

use serde_json::{Map as JsonMap, Value as JsonValue};
use toml::{Table, Value};

use crate::env_template::collect_env_var_names;
use crate::paths::format_path_for_display;

use super::{ImportPlan, ImportableServer, ImportedServerDefinition, StdioServer};

mod claude;
mod codex;
mod opencode;

pub use claude::{
    install_claude_mcp_server, load_claude_servers_for_import, replace_claude_mcp_servers,
    restore_claude_mcp_servers,
};
pub use codex::{
    install_codex_mcp_server, load_codex_servers_for_import, replace_codex_mcp_servers,
    restore_codex_mcp_servers,
};
pub use opencode::{
    install_opencode_mcp_server, load_opencode_servers_for_import, replace_opencode_mcp_servers,
    restore_opencode_mcp_servers,
};

#[cfg(test)]
pub(crate) use claude::{
    load_claude_config, load_claude_servers_for_import_from_path,
    replace_claude_mcp_servers_from_path, restore_claude_mcp_servers_from_path,
};
#[cfg(test)]
pub(crate) use codex::{
    load_codex_servers_for_import_from_path, replace_codex_mcp_servers_from_path,
    restore_codex_mcp_servers_from_path,
};
#[cfg(test)]
pub(crate) use opencode::{
    load_opencode_config, load_opencode_servers_for_import_from_path,
    replace_opencode_mcp_servers_from_path, restore_opencode_mcp_servers_from_path,
};

type JsonObject = JsonMap<String, JsonValue>;
type LoadJsonConfig = fn(&Path) -> Result<JsonValue, Box<dyn Error>>;
type SaveJsonConfig = fn(&Path, &JsonValue) -> Result<(), Box<dyn Error>>;
type InspectTomlSelfServer = fn(&Table, &str) -> Option<(String, bool)>;
type InspectJsonSelfServer = fn(&JsonObject, &str) -> Option<(String, bool)>;
type BuildTomlServerValue = fn(&StdioServer) -> Value;
type BuildJsonServerValue = fn(&StdioServer) -> JsonValue;
type FilterTomlServers = fn(&Table) -> Table;
type FilterJsonServers = fn(&JsonObject) -> JsonObject;
type MergeTomlServersIntoBackup = fn(&Path, &Table) -> Result<(), Box<dyn Error>>;
type MergeJsonServersIntoBackup = fn(&Path, &JsonObject) -> Result<(), Box<dyn Error>>;
type RemoveTomlSelfServers = fn(&mut Table) -> Result<usize, Box<dyn Error>>;
type RemoveJsonSelfServers = fn(&mut JsonValue) -> Result<usize, Box<dyn Error>>;
type MergeTomlServersIntoTarget = fn(&mut Table, &Table) -> Result<(), Box<dyn Error>>;
type MergeJsonServersIntoTarget = fn(&mut JsonValue, &JsonObject) -> Result<(), Box<dyn Error>>;
type MissingServersError = fn(&Path) -> String;
type ValidateTomlImportServer = fn(&str, &Table) -> Result<(), Box<dyn Error>>;
type ParseTomlImportEnabled = fn(&Table, &str) -> Result<bool, Box<dyn Error>>;
type ParseTomlImportedServer = fn(&Table, &str) -> Result<ImportedServerDefinition, Box<dyn Error>>;
type ValidateJsonImportServer = fn(&str, &JsonObject) -> Result<(), Box<dyn Error>>;
type ParseJsonImportEnabled = fn(&JsonObject, &str) -> Result<bool, Box<dyn Error>>;
type ParseJsonImportedServer =
    fn(&JsonObject, &str) -> Result<ImportedServerDefinition, Box<dyn Error>>;

struct JsonInstallAdapter {
    load_config: LoadJsonConfig,
    save_config: SaveJsonConfig,
    root_error: &'static str,
    servers_key: &'static str,
    servers_error: &'static str,
    inspect_self_server: InspectJsonSelfServer,
    build_server_value: BuildJsonServerValue,
}

struct JsonReplaceAdapter {
    load_config: LoadJsonConfig,
    save_config: SaveJsonConfig,
    root_error: &'static str,
    servers_key: &'static str,
    servers_error: &'static str,
    filter_backup_servers: FilterJsonServers,
    merge_into_backup: MergeJsonServersIntoBackup,
}

struct TomlRestoreAdapter {
    load_backup: fn(&Path) -> Result<Table, Box<dyn Error>>,
    backup_servers_key: &'static str,
    missing_backup_servers: MissingServersError,
    remove_self_servers: RemoveTomlSelfServers,
    merge_into_target: MergeTomlServersIntoTarget,
    filter_backup_servers: FilterTomlServers,
}

struct JsonRestoreAdapter {
    load_config: LoadJsonConfig,
    save_config: SaveJsonConfig,
    load_backup: LoadJsonConfig,
    backup_servers_key: &'static str,
    missing_backup_servers: MissingServersError,
    remove_self_servers: RemoveJsonSelfServers,
    merge_into_target: MergeJsonServersIntoTarget,
    filter_backup_servers: FilterJsonServers,
}

struct TomlImportAdapter {
    config_label: &'static str,
    servers_key: &'static str,
    missing_servers: MissingServersError,
    empty_servers: MissingServersError,
    server_type_label: &'static str,
    validate_server: ValidateTomlImportServer,
    parse_enabled: ParseTomlImportEnabled,
    parse_imported_server: ParseTomlImportedServer,
}

struct JsonImportAdapter {
    config_label: &'static str,
    servers_key: &'static str,
    missing_servers: MissingServersError,
    empty_servers: MissingServersError,
    server_type_label: &'static str,
    validate_server: ValidateJsonImportServer,
    parse_enabled: ParseJsonImportEnabled,
    parse_imported_server: ParseJsonImportedServer,
}

enum InstallDecision {
    AlreadyInstalled {
        name: String,
    },
    Write {
        name: String,
        status: super::InstallMcpServerStatus,
    },
}

fn load_provider_import_plan(
    path: fn() -> Result<std::path::PathBuf, Box<dyn Error>>,
    loader: fn(&Path) -> Result<ImportPlan, Box<dyn Error>>,
) -> Result<(std::path::PathBuf, ImportPlan), Box<dyn Error>> {
    let path = path()?;
    let plan = loader(&path)?;
    Ok((path, plan))
}

fn decide_self_server_install<'a>(
    existing_server: Option<(String, bool)>,
    existing_names: impl Iterator<Item = &'a str>,
) -> InstallDecision {
    match existing_server {
        Some((name, true)) => InstallDecision::AlreadyInstalled { name },
        Some((name, false)) => InstallDecision::Write {
            name,
            status: super::InstallMcpServerStatus::Updated,
        },
        None => InstallDecision::Write {
            name: super::self_server::next_available_server_name(existing_names),
            status: super::InstallMcpServerStatus::Installed,
        },
    }
}

fn build_replace_result(
    config_path: &Path,
    backup_path: &Path,
    backed_up_server_count: usize,
    removed_server_count: usize,
) -> super::ReplaceMcpServersResult {
    super::ReplaceMcpServersResult {
        config_path: config_path.to_path_buf(),
        backup_path: backup_path.to_path_buf(),
        backed_up_server_count,
        removed_server_count,
    }
}

fn build_restore_result(
    config_path: &Path,
    backup_path: &Path,
    removed_self_server_count: usize,
    restored_server_count: usize,
) -> super::RestoreMcpServersResult {
    super::RestoreMcpServersResult {
        config_path: config_path.to_path_buf(),
        backup_path: backup_path.to_path_buf(),
        removed_self_server_count,
        restored_server_count,
    }
}

fn ensure_import_config_exists(path: &Path, config_label: &str) -> Result<(), Box<dyn Error>> {
    if path.exists() {
        return Ok(());
    }

    Err(format!(
        "{config_label} config not found at {}",
        format_path_for_display(path)
    )
    .into())
}

fn install_toml_mcp_server(
    config_path: std::path::PathBuf,
    servers_key: &str,
    servers_error: &'static str,
    provider_name: &'static str,
    inspect_self_server: InspectTomlSelfServer,
    build_server_value: BuildTomlServerValue,
) -> Result<super::InstallMcpServerResult, Box<dyn Error>> {
    let mut config = super::local::load_config_table(&config_path)?;
    let desired_server = super::self_server::proxy_stdio_server(provider_name);

    let decision = {
        let servers_value = config
            .entry(servers_key)
            .or_insert_with(|| Value::Table(Table::new()));
        let servers = servers_value
            .as_table_mut()
            .ok_or_else(|| servers_error.to_string())?;

        let decision = decide_self_server_install(
            inspect_self_server(servers, provider_name),
            servers.keys().map(String::as_str),
        );
        if let InstallDecision::Write { name, .. } = &decision {
            servers.insert(name.clone(), build_server_value(&desired_server));
        }

        decision
    };

    let (name, status) = match decision {
        InstallDecision::AlreadyInstalled { name } => {
            return Ok(super::InstallMcpServerResult {
                name,
                config_path,
                status: super::InstallMcpServerStatus::AlreadyInstalled,
            });
        }
        InstallDecision::Write { name, status } => (name, status),
    };

    super::local::save_config_table(&config_path, &config)?;

    Ok(super::InstallMcpServerResult {
        name,
        config_path,
        status,
    })
}

fn install_json_mcp_server(
    config_path: std::path::PathBuf,
    provider_name: &'static str,
    adapter: JsonInstallAdapter,
) -> Result<super::InstallMcpServerResult, Box<dyn Error>> {
    let mut config = (adapter.load_config)(&config_path)?;
    let desired_server = super::self_server::proxy_stdio_server(provider_name);

    let decision = {
        let root = config
            .as_object_mut()
            .ok_or_else(|| adapter.root_error.to_string())?;
        let servers_value = root
            .entry(adapter.servers_key.to_string())
            .or_insert_with(|| JsonValue::Object(JsonMap::new()));
        let servers = servers_value
            .as_object_mut()
            .ok_or_else(|| adapter.servers_error.to_string())?;

        let decision = decide_self_server_install(
            (adapter.inspect_self_server)(servers, provider_name),
            servers.keys().map(String::as_str),
        );
        if let InstallDecision::Write { name, .. } = &decision {
            servers.insert(name.clone(), (adapter.build_server_value)(&desired_server));
        }

        decision
    };

    let (name, status) = match decision {
        InstallDecision::AlreadyInstalled { name } => {
            return Ok(super::InstallMcpServerResult {
                name,
                config_path,
                status: super::InstallMcpServerStatus::AlreadyInstalled,
            });
        }
        InstallDecision::Write { name, status } => (name, status),
    };

    (adapter.save_config)(&config_path, &config)?;

    Ok(super::InstallMcpServerResult {
        name,
        config_path,
        status,
    })
}

fn replace_toml_mcp_servers_from_path(
    config_path: &Path,
    servers_key: &str,
    servers_error: &'static str,
    filter_backup_servers: FilterTomlServers,
    merge_into_backup: MergeTomlServersIntoBackup,
) -> Result<super::ReplaceMcpServersResult, Box<dyn Error>> {
    let mut config = super::local::load_config_table(config_path)?;
    let existing_servers = match config.get(servers_key) {
        None => Table::new(),
        Some(Value::Table(servers)) => servers.clone(),
        Some(_) => return Err(servers_error.into()),
    };
    let backup_servers = filter_backup_servers(&existing_servers);
    let backup_path = crate::paths::sibling_backup_path(config_path, "msp-backup");

    merge_into_backup(&backup_path, &backup_servers)?;

    if config.remove(servers_key).is_some() {
        super::local::save_config_table(config_path, &config)?;
    }

    Ok(build_replace_result(
        config_path,
        &backup_path,
        backup_servers.len(),
        existing_servers.len(),
    ))
}

fn replace_json_mcp_servers_from_path(
    config_path: &Path,
    adapter: JsonReplaceAdapter,
) -> Result<super::ReplaceMcpServersResult, Box<dyn Error>> {
    let mut config = (adapter.load_config)(config_path)?;
    let root = config
        .as_object_mut()
        .ok_or_else(|| adapter.root_error.to_string())?;
    let existing_servers = match root.get(adapter.servers_key) {
        None => JsonMap::new(),
        Some(JsonValue::Object(servers)) => servers.clone(),
        Some(_) => return Err(adapter.servers_error.into()),
    };
    let backup_servers = (adapter.filter_backup_servers)(&existing_servers);
    let backup_path = crate::paths::sibling_backup_path(config_path, "msp-backup");

    (adapter.merge_into_backup)(&backup_path, &backup_servers)?;

    if root.remove(adapter.servers_key).is_some() {
        (adapter.save_config)(config_path, &config)?;
    }

    Ok(build_replace_result(
        config_path,
        &backup_path,
        backup_servers.len(),
        existing_servers.len(),
    ))
}

fn restore_toml_mcp_servers_from_path(
    config_path: &Path,
    adapter: TomlRestoreAdapter,
) -> Result<super::RestoreMcpServersResult, Box<dyn Error>> {
    let backup_path = crate::paths::sibling_backup_path(config_path, "msp-backup");
    let backup = (adapter.load_backup)(&backup_path)?;
    let restored_servers = backup
        .get(adapter.backup_servers_key)
        .and_then(Value::as_table)
        .ok_or_else(|| (adapter.missing_backup_servers)(&backup_path))?
        .clone();
    let restored_servers = (adapter.filter_backup_servers)(&restored_servers);

    let mut config = super::local::load_config_table(config_path)?;
    let removed_self_server_count = (adapter.remove_self_servers)(&mut config)?;
    (adapter.merge_into_target)(&mut config, &restored_servers)?;
    super::local::save_config_table(config_path, &config)?;

    Ok(build_restore_result(
        config_path,
        &backup_path,
        removed_self_server_count,
        restored_servers.len(),
    ))
}

fn restore_json_mcp_servers_from_path(
    config_path: &Path,
    adapter: JsonRestoreAdapter,
) -> Result<super::RestoreMcpServersResult, Box<dyn Error>> {
    let backup_path = crate::paths::sibling_backup_path(config_path, "msp-backup");
    let backup = (adapter.load_backup)(&backup_path)?;
    let restored_servers = backup
        .get(adapter.backup_servers_key)
        .and_then(JsonValue::as_object)
        .ok_or_else(|| (adapter.missing_backup_servers)(&backup_path))?
        .clone();
    let restored_servers = (adapter.filter_backup_servers)(&restored_servers);

    let mut config = (adapter.load_config)(config_path)?;
    let removed_self_server_count = (adapter.remove_self_servers)(&mut config)?;
    (adapter.merge_into_target)(&mut config, &restored_servers)?;
    (adapter.save_config)(config_path, &config)?;

    Ok(build_restore_result(
        config_path,
        &backup_path,
        removed_self_server_count,
        restored_servers.len(),
    ))
}

fn load_toml_import_plan_from_path(
    path: &Path,
    adapter: TomlImportAdapter,
) -> Result<ImportPlan, Box<dyn Error>> {
    ensure_import_config_exists(path, adapter.config_label)?;

    let config = super::local::load_config_table(path)?;
    let servers = config
        .get(adapter.servers_key)
        .and_then(Value::as_table)
        .ok_or_else(|| (adapter.missing_servers)(path))?;

    if servers.is_empty() {
        return Err((adapter.empty_servers)(path).into());
    }

    build_import_plan(servers.keys().cloned().collect(), |name| {
        let server = servers[&name]
            .as_table()
            .ok_or_else(|| format!("{} `{name}` must be a table", adapter.server_type_label))?;
        (adapter.validate_server)(&name, server)?;
        let enabled = (adapter.parse_enabled)(server, &name)?;
        let imported = (adapter.parse_imported_server)(server, &name)?;
        Ok(importable_server_from_definition(name, imported, enabled))
    })
}

fn load_json_import_plan_from_path(
    path: &Path,
    adapter: JsonImportAdapter,
) -> Result<ImportPlan, Box<dyn Error>> {
    ensure_import_config_exists(path, adapter.config_label)?;

    let config = load_json_config(path)?;
    let servers = config
        .get(adapter.servers_key)
        .and_then(JsonValue::as_object)
        .ok_or_else(|| (adapter.missing_servers)(path))?;

    if servers.is_empty() {
        return Err((adapter.empty_servers)(path).into());
    }

    build_import_plan(servers.keys().cloned().collect(), |name| {
        let server = servers[&name]
            .as_object()
            .ok_or_else(|| format!("{} `{name}` must be an object", adapter.server_type_label))?;
        (adapter.validate_server)(&name, server)?;
        let enabled = (adapter.parse_enabled)(server, &name)?;
        let imported = (adapter.parse_imported_server)(server, &name)?;
        Ok(importable_server_from_definition(name, imported, enabled))
    })
}

fn build_import_plan(
    mut names: Vec<String>,
    mut load_server: impl FnMut(String) -> Result<Option<ImportableServer>, Box<dyn Error>>,
) -> Result<ImportPlan, Box<dyn Error>> {
    names.sort();

    let mut importable_servers = Vec::new();
    let mut skipped_self_servers = Vec::new();

    for name in names {
        if let Some(server) = load_server(name.clone())? {
            importable_servers.push(server);
        } else {
            skipped_self_servers.push(name);
        }
    }

    Ok(ImportPlan {
        servers: importable_servers,
        skipped_self_servers,
    })
}

fn importable_server_from_definition(
    name: String,
    imported: ImportedServerDefinition,
    enabled: bool,
) -> Option<ImportableServer> {
    if imported.url.is_none() && super::is_self_server_command(&imported.command) {
        return None;
    }

    Some(ImportableServer {
        name,
        command: imported.command,
        url: imported.url,
        headers: imported.headers,
        enabled,
        env: imported.env,
        env_vars: imported.env_vars,
    })
}

fn load_json_config(path: &Path) -> Result<JsonValue, Box<dyn Error>> {
    let contents = fs::read_to_string(path)?;
    let value = serde_json::from_str(&contents)?;
    Ok(value)
}

fn collect_remote_header_env_vars(headers: &BTreeMap<String, String>) -> Vec<String> {
    let mut env_vars = Vec::new();

    for value in headers.values() {
        super::local::merge_env_vars(&mut env_vars, collect_remote_header_value_env_vars(value));
    }

    env_vars
}

pub(crate) fn collect_remote_header_value_env_vars(value: &str) -> Vec<String> {
    collect_env_var_names(value)
}
