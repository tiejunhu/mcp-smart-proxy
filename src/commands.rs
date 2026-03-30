use std::error::Error;
use std::ffi::OsString;
use std::path::{Path, PathBuf};

use chrono::{Local, TimeZone};
use clap::Parser;

use crate::cli::{Cli, Command, ImportSource, InstallTarget, ProviderName};
use crate::config::{
    InstallMcpServerResult, InstallMcpServerStatus, ReplaceMcpServersResult,
    RestoreMcpServersResult, add_server, contains_server_name, import_server, list_servers,
    load_config_table, load_server_config, remove_server, set_server_enabled, update_server_config,
};
use crate::console::{describe_command, operation_error, print_app_event};
use crate::mcp_server;
use crate::paths::{expand_tilde, format_path_for_display};
use crate::reload::reload_server_with_provider;
use crate::remote::{login_remote_server, logout_remote_server};
use crate::types::{ConfiguredTransport, ModelProviderConfig};
use crate::version_check;

mod config_cmd;
mod provider;

use config_cmd::{ConfigCommandArgs, print_server_config};
use provider::{
    ImportPlanLoader, import_stage, install_stage, missing_provider_error,
    provider_hooks_for_import_source, provider_hooks_for_install_target,
    resolve_default_command_provider, resolve_import_provider, resolve_install_import_provider,
    restore_stage,
};

struct ImportExecutionResult {
    source_config_path: PathBuf,
    imported_messages: Vec<String>,
    skipped_existing_servers: Vec<String>,
    skipped_self_servers: Vec<String>,
}

pub async fn run() -> Result<(), Box<dyn Error>> {
    let raw_args = std::env::args_os().collect::<Vec<OsString>>();
    let cli = Cli::parse();
    if matches!(&cli.command, Some(Command::Mcp { .. })) {
        version_check::prepare_executable_for_background_update(&raw_args);
        version_check::spawn_periodic_self_update(raw_args.clone());
    } else if !matches!(&cli.command, Some(Command::Update)) {
        version_check::print_cached_update_notice();
    }
    let config_path = expand_tilde(&cli.config).map_err(|error| {
        operation_error("cli.config_path", "failed to resolve config path", error)
    })?;

    match cli.command {
        Some(Command::Add {
            provider,
            name,
            command,
        }) => run_add_command(&config_path, provider, &name, command).await?,
        Some(Command::List) => run_list_command(&config_path)?,
        Some(Command::Enable { name }) => run_set_enabled_command(&config_path, &name, true)?,
        Some(Command::Disable { name }) => run_set_enabled_command(&config_path, &name, false)?,
        Some(Command::Config {
            name,
            transport,
            command,
            args,
            clear_args,
            url,
            enabled,
            headers,
            unset_headers,
            clear_headers,
            env,
            unset_env,
            clear_env,
            env_vars,
            unset_env_vars,
            clear_env_vars,
        }) => run_config_command(
            &config_path,
            &name,
            ConfigCommandArgs {
                transport,
                command,
                args,
                clear_args,
                url,
                enabled,
                headers,
                unset_headers,
                clear_headers,
                env,
                unset_env,
                clear_env,
                env_vars,
                unset_env_vars,
                clear_env_vars,
            },
        )?,
        Some(Command::Import { provider, source }) => {
            run_import_command(&config_path, provider, source).await?
        }
        Some(Command::Install { replace, target }) => {
            run_install_command(&config_path, replace, target).await?
        }
        Some(Command::Restore { target }) => run_restore_command(target)?,
        Some(Command::Remove { name }) => run_remove_command(&config_path, &name)?,
        Some(Command::Login { name }) => run_login_command(&config_path, &name).await?,
        Some(Command::Logout { name }) => run_logout_command(&config_path, &name)?,
        Some(Command::Update) => run_update_command().await?,
        Some(Command::Reload {
            provider,
            name: Some(name),
        }) => run_reload_one_command(&config_path, provider, &name).await?,
        Some(Command::Reload {
            provider,
            name: None,
        }) => run_reload_all_command(&config_path, provider).await?,
        Some(Command::Mcp { provider }) => run_mcp_command(&config_path, provider).await?,
        None => {
            if config_path.exists() {
                let _ = load_config_table(&config_path).map_err(|error| {
                    operation_error(
                        "cli.validate_config",
                        format!(
                            "failed to load config from {}",
                            format_path_for_display(&config_path)
                        ),
                        error,
                    )
                })?;
            }
        }
    }

    Ok(())
}

async fn run_add_command(
    config_path: &Path,
    provider_override: Option<ProviderName>,
    name: &str,
    command: Vec<String>,
) -> Result<(), Box<dyn Error>> {
    let resolved_provider =
        resolve_default_command_provider(provider_override).map_err(|error| {
            operation_error(
                "cli.add.load_provider",
                "failed to resolve the summary provider before adding the server",
                error,
            )
        })?;
    let server_name = add_server(config_path, name, command).map_err(|error| {
        operation_error(
            "cli.add",
            format!(
                "failed to add MCP server `{name}` into {}",
                format_path_for_display(config_path)
            ),
            error,
        )
    })?;
    let reload_result = match reload_server_with_provider(
        config_path,
        &server_name,
        &resolved_provider,
    )
    .await
    {
        Ok(result) => result,
        Err(error) => {
            remove_server(config_path, &server_name).map_err(|rollback_error| {
                    operation_error(
                        "cli.add.rollback",
                        format!(
                            "failed to roll back newly added MCP server `{server_name}` in {} after reload failure",
                            format_path_for_display(config_path)
                        ),
                        rollback_error,
                    )
                })?;
            return Err(operation_error(
                "cli.add.reload",
                format!("failed to reload newly added MCP server `{server_name}`"),
                error,
            ));
        }
    };
    print_app_event(
        "cli.add",
        format!(
            "Added MCP server `{server_name}` to {} and reloaded cached tools into {}",
            format_path_for_display(config_path),
            format_path_for_display(&reload_result.cache_path)
        ),
    );
    Ok(())
}

fn run_list_command(config_path: &Path) -> Result<(), Box<dyn Error>> {
    let servers = list_servers(config_path).map_err(|error| {
        operation_error(
            "cli.list",
            format!(
                "failed to list MCP servers from {}",
                format_path_for_display(config_path)
            ),
            error,
        )
    })?;
    let enabled_count = servers.iter().filter(|server| server.enabled).count();
    let disabled_count = servers.len() - enabled_count;

    print_app_event(
        "cli.list",
        format!(
            "Configured {} MCP server(s) in {} ({} enabled, {} disabled)",
            servers.len(),
            format_path_for_display(config_path),
            enabled_count,
            disabled_count
        ),
    );

    for server in servers {
        let command_line = describe_command(&server.command, &server.args);
        let last_updated = format_last_updated(server.last_updated_at);
        let state = if server.enabled {
            "enabled"
        } else {
            "disabled"
        };
        print_app_event(
            "cli.list.server",
            format!(
                "`{}` [{}]: {} (last updated: {})",
                server.name, state, command_line, last_updated
            ),
        );
    }

    Ok(())
}

fn run_set_enabled_command(
    config_path: &Path,
    name: &str,
    enabled: bool,
) -> Result<(), Box<dyn Error>> {
    let stage = if enabled { "cli.enable" } else { "cli.disable" };
    let action = if enabled { "enable" } else { "disable" };
    let result = set_server_enabled(config_path, name, enabled).map_err(|error| {
        operation_error(
            stage,
            format!(
                "failed to {action} MCP server `{name}` in {}",
                format_path_for_display(config_path)
            ),
            error,
        )
    })?;

    print_app_event(
        stage,
        format!(
            "{} MCP server `{}` in {}",
            if enabled { "Enabled" } else { "Disabled" },
            result.name,
            format_path_for_display(config_path)
        ),
    );
    Ok(())
}

fn run_config_command(
    config_path: &Path,
    name: &str,
    args: ConfigCommandArgs,
) -> Result<(), Box<dyn Error>> {
    let update = args.into_update_config(name).map_err(|error| {
        operation_error(
            "cli.config.parse",
            format!("failed to parse config update arguments for server `{name}`"),
            error,
        )
    })?;

    if update.has_changes() {
        let snapshot = update_server_config(config_path, name, &update).map_err(|error| {
            operation_error(
                "cli.config.update",
                format!(
                    "failed to update MCP server `{name}` in {}",
                    format_path_for_display(config_path)
                ),
                error,
            )
        })?;
        print_server_config("cli.config", config_path, &snapshot);
    } else {
        let snapshot = load_server_config(config_path, name).map_err(|error| {
            operation_error(
                "cli.config.read",
                format!(
                    "failed to read MCP server `{name}` from {}",
                    format_path_for_display(config_path)
                ),
                error,
            )
        })?;
        print_server_config("cli.config", config_path, &snapshot);
    }

    Ok(())
}

async fn run_import_command(
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

async fn run_install_command(
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

fn run_restore_command(target: InstallTarget) -> Result<(), Box<dyn Error>> {
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

fn run_remove_command(config_path: &Path, name: &str) -> Result<(), Box<dyn Error>> {
    let removed = remove_server(config_path, name).map_err(|error| {
        operation_error(
            "cli.remove",
            format!(
                "failed to remove MCP server `{name}` from {}",
                format_path_for_display(config_path)
            ),
            error,
        )
    })?;

    let cache_message = if removed.cache_deleted {
        format!(
            "deleted cache {}",
            format_path_for_display(&removed.cache_path)
        )
    } else {
        format!(
            "cache not found at {}",
            format_path_for_display(&removed.cache_path)
        )
    };

    print_app_event(
        "cli.remove",
        format!(
            "Removed MCP server `{}` from {}; cache: {}",
            removed.name,
            format_path_for_display(config_path),
            cache_message
        ),
    );
    Ok(())
}

async fn run_login_command(config_path: &Path, name: &str) -> Result<(), Box<dyn Error>> {
    let config = load_config_table(config_path).map_err(|error| {
        operation_error(
            "cli.login.load_config",
            format!(
                "failed to load config from {}",
                format_path_for_display(config_path)
            ),
            error,
        )
    })?;
    let (resolved_name, server) =
        crate::config::configured_server(&config, name).map_err(|error| {
            operation_error(
                "cli.login.resolve_server",
                format!("failed to resolve configured server `{name}`"),
                error,
            )
        })?;
    if !matches!(server.transport, ConfiguredTransport::Remote { .. }) {
        return Err(operation_error(
            "cli.login.unsupported_transport",
            format!("MCP server `{resolved_name}` is not configured as `remote`"),
            "only remote servers support OAuth login".into(),
        ));
    }

    login_remote_server(&resolved_name, &server)
        .await
        .map_err(|error| {
            operation_error(
                "cli.login",
                format!("failed to complete OAuth login for `{resolved_name}`"),
                error,
            )
        })?;
    print_app_event(
        "cli.login",
        format!("Completed OAuth login for remote MCP server `{resolved_name}`"),
    );
    Ok(())
}

fn run_logout_command(config_path: &Path, name: &str) -> Result<(), Box<dyn Error>> {
    let config = load_config_table(config_path).map_err(|error| {
        operation_error(
            "cli.logout.load_config",
            format!(
                "failed to load config from {}",
                format_path_for_display(config_path)
            ),
            error,
        )
    })?;
    let (resolved_name, server) =
        crate::config::configured_server(&config, name).map_err(|error| {
            operation_error(
                "cli.logout.resolve_server",
                format!("failed to resolve configured server `{name}`"),
                error,
            )
        })?;
    if !matches!(server.transport, ConfiguredTransport::Remote { .. }) {
        return Err(operation_error(
            "cli.logout.unsupported_transport",
            format!("MCP server `{resolved_name}` is not configured as `remote`"),
            "only remote servers store OAuth credentials".into(),
        ));
    }

    let removed = logout_remote_server(&resolved_name).map_err(|error| {
        operation_error(
            "cli.logout",
            format!("failed to clear OAuth credentials for `{resolved_name}`"),
            error,
        )
    })?;
    print_app_event(
        "cli.logout",
        if removed {
            format!("Cleared OAuth credentials for remote MCP server `{resolved_name}`")
        } else {
            format!("No stored OAuth credentials found for remote MCP server `{resolved_name}`")
        },
    );
    Ok(())
}

async fn run_update_command() -> Result<(), Box<dyn Error>> {
    let update_result = version_check::run_manual_self_update()
        .await
        .map_err(|error| {
            operation_error(
                "cli.update",
                "failed to update the running msp binary",
                error,
            )
        })?;
    let executable_path = format_path_for_display(&update_result.executable_path);
    if update_result.updated {
        print_app_event(
            "cli.update",
            format!(
                "Updated msp from v{} to v{} at {}",
                version_check::current_version(),
                update_result.latest_version,
                executable_path
            ),
        );
    } else {
        print_app_event(
            "cli.update",
            format!(
                "msp is already up to date at v{} ({})",
                update_result.latest_version, executable_path
            ),
        );
    }
    Ok(())
}

async fn run_reload_one_command(
    config_path: &Path,
    provider_override: Option<ProviderName>,
    name: &str,
) -> Result<(), Box<dyn Error>> {
    let resolved_provider =
        resolve_default_command_provider(provider_override).map_err(|error| {
            operation_error(
                "cli.reload.load_provider",
                "failed to resolve the summary provider before reloading the server",
                error,
            )
        })?;
    let reload_result = reload_server_with_provider(config_path, name, &resolved_provider)
        .await
        .map_err(|error| {
            operation_error(
                "cli.reload",
                format!("failed to reload MCP server `{name}`"),
                error,
            )
        })?;
    print_app_event(
        "cli.reload",
        if reload_result.updated {
            format!(
                "Reloaded MCP server `{name}`. Cache file: {}",
                format_path_for_display(&reload_result.cache_path)
            )
        } else {
            format!(
                "Skipped cache update for MCP server `{name}` because fetched tools matched {}",
                format_path_for_display(&reload_result.cache_path)
            )
        },
    );
    Ok(())
}

async fn run_reload_all_command(
    config_path: &Path,
    provider_override: Option<ProviderName>,
) -> Result<(), Box<dyn Error>> {
    let servers = list_servers(config_path).map_err(|error| {
        operation_error(
            "cli.reload.list_servers",
            format!(
                "failed to list MCP servers from {} before reloading all",
                format_path_for_display(config_path)
            ),
            error,
        )
    })?;

    if servers.is_empty() {
        print_app_event(
            "cli.reload",
            format!(
                "Reloaded 0 MCP server(s) from {}",
                format_path_for_display(config_path)
            ),
        );
        return Ok(());
    }

    let resolved_provider =
        resolve_default_command_provider(provider_override).map_err(|error| {
            operation_error(
                "cli.reload.load_provider",
                "failed to resolve the summary provider before reloading all servers",
                error,
            )
        })?;
    let mut results = Vec::new();
    for server in servers.into_iter().filter(|server| server.enabled) {
        let server_name = server.name;
        let reload_result =
            reload_server_with_provider(config_path, &server_name, &resolved_provider)
                .await
                .map_err(|error| {
                    operation_error(
                        "cli.reload.all",
                        format!("failed to reload MCP server `{server_name}`"),
                        error,
                    )
                })?;
        let status = if reload_result.updated {
            "cache updated"
        } else {
            "cache unchanged"
        };
        results.push(format!(
            "`{server_name}`: {status} at {}",
            format_path_for_display(&reload_result.cache_path)
        ));
    }

    print_app_event(
        "cli.reload",
        format!(
            "Reloaded {} MCP server(s) from {}",
            results.len(),
            format_path_for_display(config_path)
        ),
    );
    for result in results {
        print_app_event("cli.reload.server", result);
    }
    Ok(())
}

async fn run_mcp_command(
    config_path: &Path,
    provider_override: Option<ProviderName>,
) -> Result<(), Box<dyn Error>> {
    let resolved_provider =
        resolve_default_command_provider(provider_override).map_err(|error| {
            operation_error(
                "cli.mcp.load_provider",
                "failed to resolve the summary provider before starting the proxy",
                error,
            )
        })?;
    mcp_server::serve_cached_toolsets(config_path, Some(resolved_provider))
        .await
        .map_err(|error| {
            operation_error(
                "cli.mcp",
                format!(
                    "failed to start proxy MCP server with config {}",
                    format_path_for_display(config_path)
                ),
                error,
            )
        })?;
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

        let server_name = import_server(config_path, &server).map_err(|error| {
            operation_error(
                run_stage,
                format!(
                    "failed to import MCP server `{}` from {} into {}",
                    server.name,
                    format_path_for_display(&source_config_path),
                    format_path_for_display(config_path)
                ),
                error,
            )
        })?;
        imported_server_names.push(server_name.clone());

        let import_result = if server.enabled {
            reload_server_with_provider(config_path, &server_name, provider)
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
                            format_path_for_display(&source_config_path)
                        ),
                        error,
                    )
                })
        } else {
            Ok(format!(
                "Imported `{server_name}` [disabled] without reloading cached tools"
            ))
        };

        let message = match import_result {
            Ok(message) => message,
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
        imported_messages.push(message);

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

fn format_last_updated(epoch_ms: Option<u128>) -> String {
    epoch_ms
        .and_then(format_local_timestamp)
        .unwrap_or_else(|| "never".to_string())
}

fn format_local_timestamp(epoch_ms: u128) -> Option<String> {
    let epoch_ms = i64::try_from(epoch_ms).ok()?;
    let datetime = Local.timestamp_millis_opt(epoch_ms).single()?;
    Some(datetime.format("%Y-%m-%d %H:%M:%S").to_string())
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_missing_last_updated_as_never() {
        assert_eq!(format_last_updated(None), "never");
    }

    #[test]
    fn formats_last_updated_with_requested_shape() {
        let rendered = format_local_timestamp(1_742_103_456_000).unwrap();

        assert_eq!(rendered.len(), 19);
        assert_eq!(rendered.chars().nth(4), Some('-'));
        assert_eq!(rendered.chars().nth(7), Some('-'));
        assert_eq!(rendered.chars().nth(10), Some(' '));
        assert_eq!(rendered.chars().nth(13), Some(':'));
        assert_eq!(rendered.chars().nth(16), Some(':'));
    }

    #[test]
    fn resolves_import_provider_from_source_when_override_is_missing() {
        let provider = resolve_import_provider(None, ImportSource::Codex).unwrap();

        assert!(matches!(provider, ModelProviderConfig::Codex(_)));
    }

    #[test]
    fn resolves_import_provider_from_override_before_source() {
        let provider =
            resolve_import_provider(Some(ProviderName::Opencode), ImportSource::Codex).unwrap();

        assert!(matches!(provider, ModelProviderConfig::Opencode(_)));
    }

    #[test]
    fn resolves_import_provider_from_claude_source_when_override_is_missing() {
        let provider = resolve_import_provider(None, ImportSource::Claude).unwrap();

        assert!(matches!(provider, ModelProviderConfig::Claude(_)));
    }

    #[test]
    fn rejects_default_command_provider_when_override_is_missing() {
        let error = resolve_default_command_provider(None).unwrap_err();

        assert_eq!(error.to_string(), missing_provider_error());
    }

    #[test]
    fn resolves_install_import_provider_from_source() {
        let provider = resolve_install_import_provider(ImportSource::Codex).unwrap();

        assert!(matches!(provider, ModelProviderConfig::Codex(_)));
    }

    #[test]
    fn resolves_install_import_provider_from_claude_source() {
        let provider = resolve_install_import_provider(ImportSource::Claude).unwrap();

        assert!(matches!(provider, ModelProviderConfig::Claude(_)));
    }
}
