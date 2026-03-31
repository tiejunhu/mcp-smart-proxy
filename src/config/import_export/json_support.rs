use std::error::Error;
use std::fs;
use std::path::Path;

use serde_json::{Map as JsonMap, Value as JsonValue};

use crate::fs_util::write_file_atomically;
use crate::paths::format_path_for_display;

use super::super::{ImportPlan, ImportedServerDefinition, StdioServer};
use super::common::{
    InstallDecision, build_import_plan, build_replace_result, build_restore_result,
    decide_self_server_install, ensure_import_config_exists, importable_server_from_definition,
};

type JsonObject = JsonMap<String, JsonValue>;
type LoadJsonConfig = fn(&Path) -> Result<JsonValue, Box<dyn Error>>;
type SaveJsonConfig = fn(&Path, &JsonValue) -> Result<(), Box<dyn Error>>;
type InspectJsonSelfServer = fn(&JsonObject, &str) -> Option<(String, bool)>;
type BuildJsonServerValue = fn(&StdioServer) -> JsonValue;
type FilterJsonServers = fn(&JsonObject) -> JsonObject;
type MergeJsonServersIntoBackup = fn(&Path, &JsonObject) -> Result<(), Box<dyn Error>>;
type RemoveJsonSelfServers = fn(&mut JsonValue) -> Result<usize, Box<dyn Error>>;
type MergeJsonServersIntoTarget = fn(&mut JsonValue, &JsonObject) -> Result<(), Box<dyn Error>>;
type MissingServersError = fn(&Path) -> String;
type ValidateJsonImportServer = fn(&str, &JsonObject) -> Result<(), Box<dyn Error>>;
type ParseJsonImportEnabled = fn(&JsonObject, &str) -> Result<bool, Box<dyn Error>>;
type ParseJsonImportedServer =
    fn(&JsonObject, &str) -> Result<ImportedServerDefinition, Box<dyn Error>>;

pub(super) struct JsonInstallAdapter {
    pub(super) load_config: LoadJsonConfig,
    pub(super) save_config: SaveJsonConfig,
    pub(super) root_error: &'static str,
    pub(super) servers_key: &'static str,
    pub(super) servers_error: &'static str,
    pub(super) inspect_self_server: InspectJsonSelfServer,
    pub(super) build_server_value: BuildJsonServerValue,
}

pub(super) struct JsonReplaceAdapter {
    pub(super) load_config: LoadJsonConfig,
    pub(super) save_config: SaveJsonConfig,
    pub(super) root_error: &'static str,
    pub(super) servers_key: &'static str,
    pub(super) servers_error: &'static str,
    pub(super) filter_backup_servers: FilterJsonServers,
    pub(super) merge_into_backup: MergeJsonServersIntoBackup,
}

pub(super) struct JsonRestoreAdapter {
    pub(super) load_config: LoadJsonConfig,
    pub(super) save_config: SaveJsonConfig,
    pub(super) load_backup: LoadJsonConfig,
    pub(super) backup_servers_key: &'static str,
    pub(super) missing_backup_servers: MissingServersError,
    pub(super) remove_self_servers: RemoveJsonSelfServers,
    pub(super) merge_into_target: MergeJsonServersIntoTarget,
    pub(super) filter_backup_servers: FilterJsonServers,
}

pub(super) struct JsonImportAdapter {
    pub(super) config_label: &'static str,
    pub(super) servers_key: &'static str,
    pub(super) missing_servers: MissingServersError,
    pub(super) empty_servers: MissingServersError,
    pub(super) server_type_label: &'static str,
    pub(super) validate_server: ValidateJsonImportServer,
    pub(super) parse_enabled: ParseJsonImportEnabled,
    pub(super) parse_imported_server: ParseJsonImportedServer,
}

pub(super) fn install_json_mcp_server(
    config_path: std::path::PathBuf,
    provider_name: &'static str,
    adapter: JsonInstallAdapter,
) -> Result<super::super::InstallMcpServerResult, Box<dyn Error>> {
    let mut config = (adapter.load_config)(&config_path)?;
    let desired_server = super::super::self_server::proxy_stdio_server(provider_name);

    let decision = {
        let root = root_object_mut(&mut config, adapter.root_error)?;
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
            return Ok(super::super::InstallMcpServerResult {
                name,
                config_path,
                status: super::super::InstallMcpServerStatus::AlreadyInstalled,
            });
        }
        InstallDecision::Write { name, status } => (name, status),
    };

    (adapter.save_config)(&config_path, &config)?;

    Ok(super::super::InstallMcpServerResult {
        name,
        config_path,
        status,
    })
}

pub(super) fn replace_json_mcp_servers_from_path(
    config_path: &Path,
    adapter: JsonReplaceAdapter,
) -> Result<super::super::ReplaceMcpServersResult, Box<dyn Error>> {
    let mut config = (adapter.load_config)(config_path)?;
    let root = root_object_mut(&mut config, adapter.root_error)?;
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

pub(super) fn restore_json_mcp_servers_from_path(
    config_path: &Path,
    adapter: JsonRestoreAdapter,
) -> Result<super::super::RestoreMcpServersResult, Box<dyn Error>> {
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

pub(super) fn load_json_import_plan_from_path(
    path: &Path,
    adapter: JsonImportAdapter,
) -> Result<ImportPlan, Box<dyn Error>> {
    ensure_import_config_exists(path, adapter.config_label)?;

    let config = load_json_object_config(path)?;
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

pub(super) fn load_json_object_config(path: &Path) -> Result<JsonValue, Box<dyn Error>> {
    if !path.exists() {
        return Ok(JsonValue::Object(JsonMap::new()));
    }

    let contents = fs::read_to_string(path)?;
    let value = serde_json::from_str(&contents)?;
    Ok(value)
}

pub(super) fn save_json_object_config(
    path: &Path,
    config: &JsonValue,
) -> Result<(), Box<dyn Error>> {
    let contents = serde_json::to_string_pretty(config)?;
    write_file_atomically(path, contents.as_bytes())?;
    Ok(())
}

pub(super) fn load_required_json_object_config(
    path: &Path,
    config_label: &str,
) -> Result<JsonValue, Box<dyn Error>> {
    if !path.exists() {
        return Err(format!(
            "{config_label} not found at {}",
            format_path_for_display(path)
        )
        .into());
    }

    load_json_object_config(path)
}

pub(super) fn merge_json_servers_into_file(
    backup_path: &Path,
    load_config: LoadJsonConfig,
    save_config: SaveJsonConfig,
    root_error: &'static str,
    servers_key: &str,
    servers_error: &'static str,
    servers: &JsonObject,
) -> Result<(), Box<dyn Error>> {
    let mut backup = load_config(backup_path)?;
    merge_json_servers_into_config(&mut backup, root_error, servers_key, servers_error, servers)?;
    save_config(backup_path, &backup)?;
    Ok(())
}

pub(super) fn merge_json_servers_into_config(
    config: &mut JsonValue,
    root_error: &'static str,
    servers_key: &str,
    servers_error: &'static str,
    servers: &JsonObject,
) -> Result<(), Box<dyn Error>> {
    let root = root_object_mut(config, root_error)?;
    let target_servers_value = root
        .entry(servers_key.to_string())
        .or_insert_with(|| JsonValue::Object(JsonMap::new()));
    let target_servers = target_servers_value
        .as_object_mut()
        .ok_or_else(|| servers_error.to_string())?;

    for (name, server) in servers {
        target_servers.insert(name.clone(), server.clone());
    }

    Ok(())
}

pub(super) fn remove_json_self_servers(
    config: &mut JsonValue,
    root_error: &'static str,
    servers_key: &str,
    servers_error: &'static str,
    raw_command: fn(&JsonObject) -> Option<Vec<String>>,
) -> Result<usize, Box<dyn Error>> {
    let root = root_object_mut(config, root_error)?;
    let Some(servers_value) = root.get_mut(servers_key) else {
        return Ok(0);
    };
    let servers = servers_value
        .as_object_mut()
        .ok_or_else(|| servers_error.to_string())?;

    let names = servers
        .iter()
        .filter_map(|(name, value)| {
            let server = value.as_object()?;
            let raw_command = raw_command(server)?;
            super::super::is_self_server_command(&raw_command).then_some(name.clone())
        })
        .collect::<Vec<_>>();

    for name in &names {
        servers.remove(name);
    }

    if servers.is_empty() {
        root.remove(servers_key);
    }

    Ok(names.len())
}

fn root_object_mut<'a>(
    config: &'a mut JsonValue,
    root_error: &'static str,
) -> Result<&'a mut JsonObject, Box<dyn Error>> {
    config
        .as_object_mut()
        .ok_or_else(|| root_error.to_string().into())
}
