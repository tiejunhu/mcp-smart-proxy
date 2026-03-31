use std::collections::BTreeMap;
use std::error::Error;
use std::path::Path;

use crate::env_template::collect_env_var_names;
use crate::paths::format_path_for_display;

use super::super::{ImportPlan, ImportableServer, ImportedServerDefinition};

pub(super) enum InstallDecision {
    AlreadyInstalled {
        name: String,
    },
    Write {
        name: String,
        status: super::super::InstallMcpServerStatus,
    },
}

pub(super) fn load_provider_import_plan(
    path: fn() -> Result<std::path::PathBuf, Box<dyn Error>>,
    loader: fn(&Path) -> Result<ImportPlan, Box<dyn Error>>,
) -> Result<(std::path::PathBuf, ImportPlan), Box<dyn Error>> {
    let path = path()?;
    let plan = loader(&path)?;
    Ok((path, plan))
}

pub(super) fn decide_self_server_install<'a>(
    existing_server: Option<(String, bool)>,
    existing_names: impl Iterator<Item = &'a str>,
) -> InstallDecision {
    match existing_server {
        Some((name, true)) => InstallDecision::AlreadyInstalled { name },
        Some((name, false)) => InstallDecision::Write {
            name,
            status: super::super::InstallMcpServerStatus::Updated,
        },
        None => InstallDecision::Write {
            name: super::super::self_server::next_available_server_name(existing_names),
            status: super::super::InstallMcpServerStatus::Installed,
        },
    }
}

pub(super) fn build_replace_result(
    config_path: &Path,
    backup_path: &Path,
    backed_up_server_count: usize,
    removed_server_count: usize,
) -> super::super::ReplaceMcpServersResult {
    super::super::ReplaceMcpServersResult {
        config_path: config_path.to_path_buf(),
        backup_path: backup_path.to_path_buf(),
        backed_up_server_count,
        removed_server_count,
    }
}

pub(super) fn build_restore_result(
    config_path: &Path,
    backup_path: &Path,
    removed_self_server_count: usize,
    restored_server_count: usize,
) -> super::super::RestoreMcpServersResult {
    super::super::RestoreMcpServersResult {
        config_path: config_path.to_path_buf(),
        backup_path: backup_path.to_path_buf(),
        removed_self_server_count,
        restored_server_count,
    }
}

pub(super) fn ensure_import_config_exists(
    path: &Path,
    config_label: &str,
) -> Result<(), Box<dyn Error>> {
    if path.exists() {
        return Ok(());
    }

    Err(format!(
        "{config_label} config not found at {}",
        format_path_for_display(path)
    )
    .into())
}

pub(super) fn build_import_plan(
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

pub(super) fn importable_server_from_definition(
    name: String,
    imported: ImportedServerDefinition,
    enabled: bool,
) -> Option<ImportableServer> {
    if imported.url.is_none() && super::super::is_self_server_command(&imported.command) {
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

pub(super) fn collect_remote_header_env_vars(headers: &BTreeMap<String, String>) -> Vec<String> {
    let mut env_vars = Vec::new();

    for value in headers.values() {
        super::super::local::merge_env_vars(
            &mut env_vars,
            collect_remote_header_value_env_vars(value),
        );
    }

    env_vars
}

pub(crate) fn collect_remote_header_value_env_vars(value: &str) -> Vec<String> {
    collect_env_var_names(value)
}
