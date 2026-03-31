use std::error::Error;
use std::path::{Path, PathBuf};

use crate::cli::{ImportSource, InstallTarget, ProviderName};
use crate::config::{
    ImportableServer, InstallMcpServerResult, InstallMcpServerStatus, ReplaceMcpServersResult,
    RestoreMcpServersResult, contains_server_name, import_server, load_config_table, remove_server,
};
use crate::console::{operation_error, print_app_event};
use crate::paths::format_path_for_display;
use crate::reload::reload_server_with_provider;
use crate::types::ModelProviderConfig;

use super::provider::{
    ImportPlanLoader, import_stage, install_stage, provider_hooks_for_import_source,
    provider_hooks_for_install_target, resolve_import_provider, resolve_install_import_provider,
    restore_stage,
};

struct ImportExecutionResult {
    source_config_path: PathBuf,
    imported_messages: Vec<String>,
    skipped_existing_servers: Vec<String>,
    skipped_self_servers: Vec<String>,
}

pub(super) async fn run_import_command(
    config_path: &Path,
    provider_override: Option<ProviderName>,
    source: ImportSource,
) -> Result<(), Box<dyn Error>> {
    let hooks = provider_hooks_for_import_source(source);
    let provider =
        resolve_import_provider(provider_override, hooks.import_source).map_err(|error| {
            operation_error(
                import_stage(hooks.provider_name, "load_provider"),
                format!(
                    "failed to load the provider configuration before importing into {}",
                    format_path_for_display(config_path)
                ),
                error,
            )
        })?;
    let import_result = run_import_execution(
        config_path,
        hooks.load_import_plan,
        &provider,
        import_stage(hooks.provider_name, "load_source"),
        import_stage(hooks.provider_name, "run"),
    )
    .await?;

    print_import_summary(
        import_stage(hooks.provider_name, "run"),
        config_path,
        &import_result,
    );
    Ok(())
}

pub(super) async fn run_install_command(
    config_path: &Path,
    replace: bool,
    target: InstallTarget,
) -> Result<(), Box<dyn Error>> {
    let hooks = provider_hooks_for_install_target(target);
    let install_stage = install_stage(hooks.provider_name);

    if replace {
        let provider = resolve_install_import_provider(hooks.import_source).map_err(|error| {
            operation_error(
                "cli.install.replace.load_provider",
                format!(
                    "failed to load the provider configuration before importing into {}",
                    format_path_for_display(config_path)
                ),
                error,
            )
        })?;
        let import_result = run_import_execution(
            config_path,
            hooks.load_import_plan,
            &provider,
            "cli.install.replace.load_source",
            "cli.install.replace.import",
        )
        .await?;
        print_app_event(
            "cli.install.replace.import",
            format!(
                "Imported {} MCP server(s) from {} into {} before replacing {} MCP config",
                import_result.imported_messages.len(),
                format_path_for_display(&import_result.source_config_path),
                format_path_for_display(config_path),
                hooks.provider_name
            ),
        );
        print_import_details("cli.install.replace.import", &import_result);

        let replaced = (hooks.replace_servers)().map_err(|error| {
            operation_error(
                "cli.install.replace.backup",
                format!(
                    "failed to back up and clear {} MCP servers",
                    hooks.provider_name
                ),
                error,
            )
        })?;
        print_replace_result("cli.install.replace.backup", &replaced);
    }

    let installed = (hooks.install_server)().map_err(|error| {
        operation_error(
            install_stage,
            format!("failed to install msp into {} config", hooks.provider_name),
            error,
        )
    })?;
    print_install_result(install_stage, hooks.provider_name, &installed);
    Ok(())
}

pub(super) fn run_restore_command(target: InstallTarget) -> Result<(), Box<dyn Error>> {
    let hooks = provider_hooks_for_install_target(target);
    let stage = restore_stage(hooks.provider_name);
    let restored = (hooks.restore_servers)().map_err(|error| {
        operation_error(
            stage,
            format!(
                "failed to restore MCP servers into {} config",
                hooks.provider_name
            ),
            error,
        )
    })?;
    print_restore_result(stage, hooks.provider_name, &restored);
    Ok(())
}

async fn run_import_execution(
    config_path: &Path,
    load_import_plan: ImportPlanLoader,
    provider: &ModelProviderConfig,
    load_stage: &'static str,
    run_stage: &'static str,
) -> Result<ImportExecutionResult, Box<dyn Error>> {
    let mut config = load_config_table(config_path).map_err(|error| {
        operation_error(
            "cli.import.load_config",
            format!(
                "failed to load config from {}",
                format_path_for_display(config_path)
            ),
            error,
        )
    })?;
    let (source_config_path, import_plan) = load_import_plan().map_err(|error| {
        operation_error(
            load_stage,
            "failed to load importable MCP servers from provider config",
            error,
        )
    })?;

    let mut imported_server_names = Vec::new();
    let mut imported_messages = Vec::new();
    let mut skipped_existing_servers = Vec::new();

    for server in import_plan.servers {
        if contains_server_name(&config, &server.name) {
            skipped_existing_servers.push(server.name);
            continue;
        }

        let server_name = match import_one_server(
            config_path,
            &source_config_path,
            run_stage,
            provider,
            &server,
        )
        .await
        {
            Ok(result) => result,
            Err(error) => {
                rollback_imported_servers(config_path, &imported_server_names).map_err(
                        |rollback_error| {
                            operation_error(
                                "cli.import.rollback",
                                format!(
                                    "failed to roll back imported MCP servers in {} after a batch import failure",
                                    format_path_for_display(config_path)
                                ),
                                rollback_error,
                            )
                        },
                    )?;
                return Err(error);
            }
        };
        imported_server_names.push(server_name.0);
        imported_messages.push(server_name.1);

        config = load_config_table(config_path).map_err(|error| {
            operation_error(
                "cli.import.refresh_config",
                format!(
                    "failed to refresh config from {}",
                    format_path_for_display(config_path)
                ),
                error,
            )
        })?;
    }

    Ok(ImportExecutionResult {
        source_config_path,
        imported_messages,
        skipped_existing_servers,
        skipped_self_servers: import_plan.skipped_self_servers,
    })
}

async fn import_one_server(
    config_path: &Path,
    source_config_path: &Path,
    run_stage: &'static str,
    provider: &ModelProviderConfig,
    server: &ImportableServer,
) -> Result<(String, String), Box<dyn Error>> {
    let server_name = import_server(config_path, server).map_err(|error| {
        operation_error(
            run_stage,
            format!(
                "failed to import MCP server `{}` from {} into {}",
                server.name,
                format_path_for_display(source_config_path),
                format_path_for_display(config_path)
            ),
            error,
        )
    })?;

    let message = build_import_message(
        config_path,
        source_config_path,
        run_stage,
        provider,
        server,
        &server_name,
    )
    .await?;

    Ok((server_name, message))
}

async fn build_import_message(
    config_path: &Path,
    source_config_path: &Path,
    run_stage: &'static str,
    provider: &ModelProviderConfig,
    server: &ImportableServer,
    server_name: &str,
) -> Result<String, Box<dyn Error>> {
    if !server.enabled {
        return Ok(format!(
            "Imported `{server_name}` [disabled] without reloading cached tools"
        ));
    }

    reload_server_with_provider(config_path, server_name, provider)
        .await
        .map(|reload_result| {
            format!(
                "Imported `{server_name}` [enabled] and cached tools at {}",
                format_path_for_display(&reload_result.cache_path)
            )
        })
        .map_err(|error| {
            operation_error(
                run_stage,
                format!(
                    "failed to reload imported MCP server `{server_name}` from {}",
                    format_path_for_display(source_config_path)
                ),
                error,
            )
        })
}

fn rollback_imported_servers(
    config_path: &Path,
    imported_server_names: &[String],
) -> Result<(), Box<dyn Error>> {
    for name in imported_server_names.iter().rev() {
        remove_server(config_path, name)?;
    }
    Ok(())
}

fn print_import_summary(stage: &'static str, config_path: &Path, result: &ImportExecutionResult) {
    print_app_event(
        stage,
        format!(
            "Imported {} MCP server(s) from {} into {}",
            result.imported_messages.len(),
            format_path_for_display(&result.source_config_path),
            format_path_for_display(config_path)
        ),
    );
    print_import_details(stage, result);
}

fn print_import_details(stage: &'static str, result: &ImportExecutionResult) {
    for message in &result.imported_messages {
        print_app_event(&format!("{stage}.server"), message);
    }
    for name in &result.skipped_existing_servers {
        print_app_event(
            &format!("{stage}.skipped"),
            format!("Skipped existing server `{name}`"),
        );
    }
    for name in &result.skipped_self_servers {
        print_app_event(
            &format!("{stage}.skipped"),
            format!("Skipped self-referential server `{name}`"),
        );
    }
}

fn print_install_result(stage: &str, provider: &str, installed: &InstallMcpServerResult) {
    let command_line = format!("msp mcp --provider {provider}");
    let message = match installed.status {
        InstallMcpServerStatus::AlreadyInstalled => format!(
            "MCP server `{}` already exists in {} with command `{command_line}`",
            installed.name,
            format_path_for_display(&installed.config_path)
        ),
        InstallMcpServerStatus::Updated => format!(
            "Updated MCP server `{}` in {} to command `{command_line}`",
            installed.name,
            format_path_for_display(&installed.config_path)
        ),
        InstallMcpServerStatus::Installed => format!(
            "Installed MCP server `{}` into {} with command `{command_line}`",
            installed.name,
            format_path_for_display(&installed.config_path)
        ),
    };

    print_app_event(stage, message);
}

fn print_replace_result(stage: &str, replaced: &ReplaceMcpServersResult) {
    let message = format!(
        "Backed up {} MCP server(s) from {} to {} and removed {} MCP server(s) before install",
        replaced.backed_up_server_count,
        format_path_for_display(&replaced.config_path),
        format_path_for_display(&replaced.backup_path),
        replaced.removed_server_count,
    );

    print_app_event(stage, message);
}

fn print_restore_result(stage: &str, provider: &str, restored: &RestoreMcpServersResult) {
    let message = format!(
        "Removed {} `msp mcp` server(s) from {} {} config and restored {} MCP server(s) from {}",
        restored.removed_self_server_count,
        provider,
        format_path_for_display(&restored.config_path),
        restored.restored_server_count,
        format_path_for_display(&restored.backup_path),
    );

    print_app_event(stage, message);
}
