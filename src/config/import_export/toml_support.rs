use std::error::Error;
use std::path::Path;

use toml::{Table, Value};

use crate::paths::format_path_for_display;

use super::super::{ImportPlan, ImportedServerDefinition, StdioServer};
use super::common::{
    InstallDecision, build_import_plan, build_replace_result, build_restore_result,
    decide_self_server_install, ensure_import_config_exists, importable_server_from_definition,
};

type InspectTomlSelfServer = fn(&Table, &str) -> Option<(String, bool)>;
type BuildTomlServerValue = fn(&StdioServer) -> Value;
type FilterTomlServers = fn(&Table) -> Table;
type MergeTomlServersIntoBackup = fn(&Path, &Table) -> Result<(), Box<dyn Error>>;
type RemoveTomlSelfServers = fn(&mut Table) -> Result<usize, Box<dyn Error>>;
type MergeTomlServersIntoTarget = fn(&mut Table, &Table) -> Result<(), Box<dyn Error>>;
type MissingServersError = fn(&Path) -> String;
type ValidateTomlImportServer = fn(&str, &Table) -> Result<(), Box<dyn Error>>;
type ParseTomlImportEnabled = fn(&Table, &str) -> Result<bool, Box<dyn Error>>;
type ParseTomlImportedServer = fn(&Table, &str) -> Result<ImportedServerDefinition, Box<dyn Error>>;

pub(super) struct TomlRestoreAdapter {
    pub(super) load_backup: fn(&Path) -> Result<Table, Box<dyn Error>>,
    pub(super) backup_servers_key: &'static str,
    pub(super) missing_backup_servers: MissingServersError,
    pub(super) remove_self_servers: RemoveTomlSelfServers,
    pub(super) merge_into_target: MergeTomlServersIntoTarget,
    pub(super) filter_backup_servers: FilterTomlServers,
}

pub(super) struct TomlImportAdapter {
    pub(super) config_label: &'static str,
    pub(super) servers_key: &'static str,
    pub(super) missing_servers: MissingServersError,
    pub(super) empty_servers: MissingServersError,
    pub(super) server_type_label: &'static str,
    pub(super) validate_server: ValidateTomlImportServer,
    pub(super) parse_enabled: ParseTomlImportEnabled,
    pub(super) parse_imported_server: ParseTomlImportedServer,
}

pub(super) fn install_toml_mcp_server(
    config_path: std::path::PathBuf,
    servers_key: &str,
    servers_error: &'static str,
    provider_name: &'static str,
    inspect_self_server: InspectTomlSelfServer,
    build_server_value: BuildTomlServerValue,
) -> Result<super::super::InstallMcpServerResult, Box<dyn Error>> {
    let mut config = super::super::local::load_config_table(&config_path)?;
    let desired_server = super::super::self_server::proxy_stdio_server(provider_name);

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
            return Ok(super::super::InstallMcpServerResult {
                name,
                config_path,
                status: super::super::InstallMcpServerStatus::AlreadyInstalled,
            });
        }
        InstallDecision::Write { name, status } => (name, status),
    };

    super::super::local::save_config_table(&config_path, &config)?;

    Ok(super::super::InstallMcpServerResult {
        name,
        config_path,
        status,
    })
}

pub(super) fn replace_toml_mcp_servers_from_path(
    config_path: &Path,
    servers_key: &str,
    servers_error: &'static str,
    filter_backup_servers: FilterTomlServers,
    merge_into_backup: MergeTomlServersIntoBackup,
) -> Result<super::super::ReplaceMcpServersResult, Box<dyn Error>> {
    let mut config = super::super::local::load_config_table(config_path)?;
    let existing_servers = match config.get(servers_key) {
        None => Table::new(),
        Some(Value::Table(servers)) => servers.clone(),
        Some(_) => return Err(servers_error.into()),
    };
    let backup_servers = filter_backup_servers(&existing_servers);
    let backup_path = crate::paths::sibling_backup_path(config_path, "msp-backup");

    merge_into_backup(&backup_path, &backup_servers)?;

    if config.remove(servers_key).is_some() {
        super::super::local::save_config_table(config_path, &config)?;
    }

    Ok(build_replace_result(
        config_path,
        &backup_path,
        backup_servers.len(),
        existing_servers.len(),
    ))
}

pub(super) fn restore_toml_mcp_servers_from_path(
    config_path: &Path,
    adapter: TomlRestoreAdapter,
) -> Result<super::super::RestoreMcpServersResult, Box<dyn Error>> {
    let backup_path = crate::paths::sibling_backup_path(config_path, "msp-backup");
    let backup = (adapter.load_backup)(&backup_path)?;
    let restored_servers = backup
        .get(adapter.backup_servers_key)
        .and_then(Value::as_table)
        .ok_or_else(|| (adapter.missing_backup_servers)(&backup_path))?
        .clone();
    let restored_servers = (adapter.filter_backup_servers)(&restored_servers);

    let mut config = super::super::local::load_config_table(config_path)?;
    let removed_self_server_count = (adapter.remove_self_servers)(&mut config)?;
    (adapter.merge_into_target)(&mut config, &restored_servers)?;
    super::super::local::save_config_table(config_path, &config)?;

    Ok(build_restore_result(
        config_path,
        &backup_path,
        removed_self_server_count,
        restored_servers.len(),
    ))
}

pub(super) fn load_toml_import_plan_from_path(
    path: &Path,
    adapter: TomlImportAdapter,
) -> Result<ImportPlan, Box<dyn Error>> {
    ensure_import_config_exists(path, adapter.config_label)?;

    let config = super::super::local::load_config_table(path)?;
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

pub(super) fn load_required_toml_config(
    path: &Path,
    config_label: &str,
) -> Result<Table, Box<dyn Error>> {
    if !path.exists() {
        return Err(format!(
            "{config_label} not found at {}",
            format_path_for_display(path)
        )
        .into());
    }

    super::super::local::load_config_table(path)
}

pub(super) fn merge_toml_servers_into_file(
    backup_path: &Path,
    servers_key: &str,
    servers_error: &'static str,
    servers: &Table,
) -> Result<(), Box<dyn Error>> {
    let mut backup = super::super::local::load_config_table(backup_path)?;
    merge_toml_servers_into_config(&mut backup, servers_key, servers_error, servers)?;
    super::super::local::save_config_table(backup_path, &backup)?;
    Ok(())
}

pub(super) fn merge_toml_servers_into_config(
    config: &mut Table,
    servers_key: &str,
    servers_error: &'static str,
    servers: &Table,
) -> Result<(), Box<dyn Error>> {
    let target_servers_value = config
        .entry(servers_key)
        .or_insert_with(|| Value::Table(Table::new()));
    let target_servers = target_servers_value
        .as_table_mut()
        .ok_or_else(|| servers_error.to_string())?;

    for (name, server) in servers {
        target_servers.insert(name.clone(), server.clone());
    }

    Ok(())
}

pub(super) fn remove_toml_self_servers(
    config: &mut Table,
    servers_key: &str,
    servers_error: &'static str,
    raw_command: fn(&Table) -> Option<Vec<String>>,
) -> Result<usize, Box<dyn Error>> {
    let Some(servers_value) = config.get_mut(servers_key) else {
        return Ok(0);
    };
    let servers = servers_value
        .as_table_mut()
        .ok_or_else(|| servers_error.to_string())?;

    let names = servers
        .iter()
        .filter_map(|(name, value)| {
            let server = value.as_table()?;
            let raw_command = raw_command(server)?;
            super::super::is_self_server_command(&raw_command).then_some(name.clone())
        })
        .collect::<Vec<_>>();

    for name in &names {
        servers.remove(name);
    }

    if servers.is_empty() {
        config.remove(servers_key);
    }

    Ok(names.len())
}
