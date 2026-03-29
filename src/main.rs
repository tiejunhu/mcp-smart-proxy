use std::error::Error;

use chrono::{Local, TimeZone};
use clap::Parser;

mod cli;
mod config;
mod console;
mod mcp_server;
mod paths;
mod reload;
mod types;
mod version_check;

use cli::{Cli, Command, ImportSource, InstallTarget, ProviderName};
use config::{
    ImportPlan, InstallMcpServerResult, InstallMcpServerStatus, ReplaceMcpServersResult,
    RestoreMcpServersResult, ServerConfigSnapshot, UpdateServerConfig, add_server,
    contains_server_name, import_server, install_codex_mcp_server, install_opencode_mcp_server,
    list_servers, load_codex_servers_for_import, load_config_table, load_model_provider_config,
    load_opencode_servers_for_import, load_server_config, remove_server, replace_codex_mcp_servers,
    replace_opencode_mcp_servers, restore_codex_mcp_servers, restore_opencode_mcp_servers,
    set_server_enabled, update_server_config,
};
use console::{describe_command, operation_error, print_app_error, print_app_event};
use paths::expand_tilde;
use reload::reload_server_with_provider;
use types::ModelProviderConfig;

#[tokio::main]
async fn main() {
    if let Err(error) = run().await {
        print_app_error(error.as_ref());
        std::process::exit(1);
    }
}

async fn run() -> Result<(), Box<dyn Error>> {
    let cli = Cli::parse();
    if matches!(&cli.command, Some(Command::Mcp { .. })) {
        version_check::spawn_periodic_version_check_record_refresh();
    } else {
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
        }) => {
            let resolved_provider =
                resolve_default_command_provider(provider).map_err(|error| {
                    operation_error(
                        "cli.add.load_provider",
                        "failed to resolve the summary provider before adding the server",
                        error,
                    )
                })?;
            let server_name = add_server(&config_path, &name, command).map_err(|error| {
                operation_error(
                    "cli.add",
                    format!(
                        "failed to add MCP server `{name}` into {}",
                        config_path.display()
                    ),
                    error,
                )
            })?;
            let reload_result = match reload_server_with_provider(
                &config_path,
                &server_name,
                &resolved_provider,
            )
            .await
            {
                Ok(result) => result,
                Err(error) => {
                    remove_server(&config_path, &server_name).map_err(|rollback_error| {
                        operation_error(
                            "cli.add.rollback",
                            format!(
                                "failed to roll back newly added MCP server `{server_name}` in {} after reload failure",
                                config_path.display()
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
                    "Added stdio MCP server `{server_name}` to {} and reloaded cached tools into {}",
                    config_path.display(),
                    reload_result.cache_path.display()
                ),
            );
        }
        Some(Command::List) => {
            let servers = list_servers(&config_path).map_err(|error| {
                operation_error(
                    "cli.list",
                    format!("failed to list MCP servers from {}", config_path.display()),
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
                    config_path.display(),
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
        }
        Some(Command::Enable { name }) => {
            let result = set_server_enabled(&config_path, &name, true).map_err(|error| {
                operation_error(
                    "cli.enable",
                    format!(
                        "failed to enable MCP server `{name}` in {}",
                        config_path.display()
                    ),
                    error,
                )
            })?;

            print_app_event(
                "cli.enable",
                format!(
                    "Enabled MCP server `{}` in {}",
                    result.name,
                    config_path.display()
                ),
            );
        }
        Some(Command::Disable { name }) => {
            let result = set_server_enabled(&config_path, &name, false).map_err(|error| {
                operation_error(
                    "cli.disable",
                    format!(
                        "failed to disable MCP server `{name}` in {}",
                        config_path.display()
                    ),
                    error,
                )
            })?;

            print_app_event(
                "cli.disable",
                format!(
                    "Disabled MCP server `{}` in {}",
                    result.name,
                    config_path.display()
                ),
            );
        }
        Some(Command::Config {
            name,
            transport,
            command,
            args,
            clear_args,
            enabled,
            env,
            unset_env,
            clear_env,
            env_vars,
            unset_env_vars,
            clear_env_vars,
        }) => {
            let set_env = parse_env_assignments(&env).map_err(|error| {
                operation_error(
                    "cli.config.parse_env",
                    format!("failed to parse `--env` values for server `{name}`"),
                    error,
                )
            })?;
            let update = UpdateServerConfig {
                transport: transport.map(|value| value.as_str().to_string()),
                command,
                clear_args,
                add_args: args,
                enabled,
                clear_env,
                set_env,
                unset_env,
                clear_env_vars,
                add_env_vars: env_vars,
                unset_env_vars,
            };

            if update.has_changes() {
                let snapshot =
                    update_server_config(&config_path, &name, &update).map_err(|error| {
                        operation_error(
                            "cli.config.update",
                            format!(
                                "failed to update MCP server `{name}` in {}",
                                config_path.display()
                            ),
                            error,
                        )
                    })?;
                print_server_config("cli.config", &config_path, &snapshot);
            } else {
                let snapshot = load_server_config(&config_path, &name).map_err(|error| {
                    operation_error(
                        "cli.config.read",
                        format!(
                            "failed to read MCP server `{name}` from {}",
                            config_path.display()
                        ),
                        error,
                    )
                })?;
                print_server_config("cli.config", &config_path, &snapshot);
            }
        }
        Some(Command::Import {
            provider,
            source: ImportSource::Codex,
        }) => {
            let mut config = load_config_table(&config_path).map_err(|error| {
                operation_error(
                    "cli.import.codex.load_config",
                    format!("failed to load config from {}", config_path.display()),
                    error,
                )
            })?;
            let provider =
                resolve_import_provider(provider, ImportSource::Codex).map_err(|error| {
                    operation_error(
                        "cli.import.codex.load_provider",
                        format!(
                            "failed to load the provider configuration before importing into {}",
                            config_path.display()
                        ),
                        error,
                    )
                })?;
            let (codex_config_path, import_plan) =
                load_codex_servers_for_import().map_err(|error| {
                    operation_error(
                        "cli.import.codex.load_source",
                        "failed to load importable MCP servers from Codex config",
                        error,
                    )
                })?;

            let mut imported_servers = Vec::new();
            let mut skipped_existing_servers = Vec::new();
            for server in import_plan.servers {
                if contains_server_name(&config, &server.name) {
                    skipped_existing_servers.push(server.name);
                    continue;
                }

                let server_name = import_server(&config_path, &server).map_err(|error| {
                    operation_error(
                        "cli.import.codex.add",
                        format!(
                            "failed to import MCP server `{}` from {} into {}",
                            server.name,
                            codex_config_path.display(),
                            config_path.display()
                        ),
                        error,
                    )
                })?;
                if server.enabled {
                    let reload_result = reload_server_with_provider(
                        &config_path,
                        &server_name,
                        &provider,
                    )
                    .await
                    .map_err(|error| {
                        operation_error(
                            "cli.import.codex.reload",
                            format!(
                                "failed to reload imported MCP server `{server_name}` from {}",
                                codex_config_path.display()
                            ),
                            error,
                        )
                    })?;
                    imported_servers.push(format!(
                        "Imported `{server_name}` [enabled] and cached tools at {}",
                        reload_result.cache_path.display()
                    ));
                } else {
                    imported_servers.push(format!(
                        "Imported `{server_name}` [disabled] without reloading cached tools"
                    ));
                }
                config = load_config_table(&config_path).map_err(|error| {
                    operation_error(
                        "cli.import.codex.refresh_config",
                        format!("failed to refresh config from {}", config_path.display()),
                        error,
                    )
                })?;
            }

            print_app_event(
                "cli.import.codex",
                format!(
                    "Imported {} MCP server(s) from {} into {}",
                    imported_servers.len(),
                    codex_config_path.display(),
                    config_path.display()
                ),
            );
            for message in imported_servers {
                print_app_event("cli.import.codex.server", message);
            }
            for name in skipped_existing_servers {
                print_app_event(
                    "cli.import.codex.skipped",
                    format!("Skipped existing server `{name}`"),
                );
            }
            for name in import_plan.skipped_self_servers {
                print_app_event(
                    "cli.import.codex.skipped",
                    format!("Skipped self-referential server `{name}`"),
                );
            }
        }
        Some(Command::Import {
            provider,
            source: ImportSource::Opencode,
        }) => {
            let mut config = load_config_table(&config_path).map_err(|error| {
                operation_error(
                    "cli.import.opencode.load_config",
                    format!("failed to load config from {}", config_path.display()),
                    error,
                )
            })?;
            let provider =
                resolve_import_provider(provider, ImportSource::Opencode).map_err(|error| {
                    operation_error(
                        "cli.import.opencode.load_provider",
                        format!(
                            "failed to load the provider configuration before importing into {}",
                            config_path.display()
                        ),
                        error,
                    )
                })?;
            let (opencode_config_path, import_plan) =
                load_opencode_servers_for_import().map_err(|error| {
                    operation_error(
                        "cli.import.opencode.load_source",
                        "failed to load importable MCP servers from OpenCode config",
                        error,
                    )
                })?;

            let mut imported_servers = Vec::new();
            let mut skipped_existing_servers = Vec::new();
            for server in import_plan.servers {
                if contains_server_name(&config, &server.name) {
                    skipped_existing_servers.push(server.name);
                    continue;
                }

                let server_name = import_server(&config_path, &server).map_err(|error| {
                    operation_error(
                        "cli.import.opencode.add",
                        format!(
                            "failed to import MCP server `{}` from {} into {}",
                            server.name,
                            opencode_config_path.display(),
                            config_path.display()
                        ),
                        error,
                    )
                })?;
                if server.enabled {
                    let reload_result = reload_server_with_provider(
                        &config_path,
                        &server_name,
                        &provider,
                    )
                    .await
                    .map_err(|error| {
                        operation_error(
                            "cli.import.opencode.reload",
                            format!(
                                "failed to reload imported MCP server `{server_name}` from {}",
                                opencode_config_path.display()
                            ),
                            error,
                        )
                    })?;
                    imported_servers.push(format!(
                        "Imported `{server_name}` [enabled] and cached tools at {}",
                        reload_result.cache_path.display()
                    ));
                } else {
                    imported_servers.push(format!(
                        "Imported `{server_name}` [disabled] without reloading cached tools"
                    ));
                }
                config = load_config_table(&config_path).map_err(|error| {
                    operation_error(
                        "cli.import.opencode.refresh_config",
                        format!("failed to refresh config from {}", config_path.display()),
                        error,
                    )
                })?;
            }

            print_app_event(
                "cli.import.opencode",
                format!(
                    "Imported {} MCP server(s) from {} into {}",
                    imported_servers.len(),
                    opencode_config_path.display(),
                    config_path.display()
                ),
            );
            for message in imported_servers {
                print_app_event("cli.import.opencode.server", message);
            }
            for name in skipped_existing_servers {
                print_app_event(
                    "cli.import.opencode.skipped",
                    format!("Skipped existing server `{name}`"),
                );
            }
            for name in import_plan.skipped_self_servers {
                print_app_event(
                    "cli.import.opencode.skipped",
                    format!("Skipped self-referential server `{name}`"),
                );
            }
        }
        Some(Command::Install {
            replace,
            target: InstallTarget::Codex,
        }) => {
            if replace {
                install_with_replace(
                    &config_path,
                    ImportSource::Codex,
                    load_codex_servers_for_import,
                    replace_codex_mcp_servers,
                    install_codex_mcp_server,
                    "cli.install.codex",
                    "codex",
                )
                .await?;
            } else {
                let installed = install_codex_mcp_server().map_err(|error| {
                    operation_error(
                        "cli.install.codex",
                        "failed to install msp into Codex config",
                        error,
                    )
                })?;
                print_install_result("cli.install.codex", "codex", &installed);
            }
        }
        Some(Command::Install {
            replace,
            target: InstallTarget::Opencode,
        }) => {
            if replace {
                install_with_replace(
                    &config_path,
                    ImportSource::Opencode,
                    load_opencode_servers_for_import,
                    replace_opencode_mcp_servers,
                    install_opencode_mcp_server,
                    "cli.install.opencode",
                    "opencode",
                )
                .await?;
            } else {
                let installed = install_opencode_mcp_server().map_err(|error| {
                    operation_error(
                        "cli.install.opencode",
                        "failed to install msp into OpenCode config",
                        error,
                    )
                })?;
                print_install_result("cli.install.opencode", "opencode", &installed);
            }
        }
        Some(Command::Restore {
            target: InstallTarget::Codex,
        }) => {
            let restored = restore_codex_mcp_servers().map_err(|error| {
                operation_error(
                    "cli.restore.codex",
                    "failed to restore MCP servers into Codex config",
                    error,
                )
            })?;
            print_restore_result("cli.restore.codex", "codex", &restored);
        }
        Some(Command::Restore {
            target: InstallTarget::Opencode,
        }) => {
            let restored = restore_opencode_mcp_servers().map_err(|error| {
                operation_error(
                    "cli.restore.opencode",
                    "failed to restore MCP servers into OpenCode config",
                    error,
                )
            })?;
            print_restore_result("cli.restore.opencode", "opencode", &restored);
        }
        Some(Command::Remove { name }) => {
            let removed = remove_server(&config_path, &name).map_err(|error| {
                operation_error(
                    "cli.remove",
                    format!(
                        "failed to remove MCP server `{name}` from {}",
                        config_path.display()
                    ),
                    error,
                )
            })?;

            let cache_message = if removed.cache_deleted {
                format!("deleted cache {}", removed.cache_path.display())
            } else {
                format!("cache not found at {}", removed.cache_path.display())
            };

            print_app_event(
                "cli.remove",
                format!(
                    "Removed MCP server `{}` from {}; cache: {}",
                    removed.name,
                    config_path.display(),
                    cache_message
                ),
            );
        }
        Some(Command::Reload {
            provider,
            name: Some(name),
        }) => {
            let resolved_provider =
                resolve_default_command_provider(provider).map_err(|error| {
                    operation_error(
                        "cli.reload.load_provider",
                        "failed to resolve the summary provider before reloading the server",
                        error,
                    )
                })?;
            let reload_result =
                reload_server_with_provider(&config_path, &name, &resolved_provider)
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
                        reload_result.cache_path.display()
                    )
                } else {
                    format!(
                        "Skipped cache update for MCP server `{name}` because fetched tools matched {}",
                        reload_result.cache_path.display()
                    )
                },
            );
        }
        Some(Command::Reload {
            provider,
            name: None,
        }) => {
            let servers = list_servers(&config_path).map_err(|error| {
                operation_error(
                    "cli.reload.list_servers",
                    format!(
                        "failed to list MCP servers from {} before reloading all",
                        config_path.display()
                    ),
                    error,
                )
            })?;

            if servers.is_empty() {
                print_app_event(
                    "cli.reload",
                    format!("Reloaded 0 MCP server(s) from {}", config_path.display()),
                );
            } else {
                let resolved_provider =
                    resolve_default_command_provider(provider).map_err(|error| {
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
                        reload_server_with_provider(&config_path, &server_name, &resolved_provider)
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
                        reload_result.cache_path.display()
                    ));
                }

                print_app_event(
                    "cli.reload",
                    format!(
                        "Reloaded {} MCP server(s) from {}",
                        results.len(),
                        config_path.display()
                    ),
                );
                for result in results {
                    print_app_event("cli.reload.server", result);
                }
            }
        }
        Some(Command::Mcp { provider }) => {
            let resolved_provider =
                resolve_default_command_provider(provider).map_err(|error| {
                    operation_error(
                        "cli.mcp.load_provider",
                        "failed to resolve the summary provider before starting the proxy",
                        error,
                    )
                })?;
            mcp_server::serve_cached_toolsets(&config_path, Some(resolved_provider))
                .await
                .map_err(|error| {
                    operation_error(
                        "cli.mcp",
                        format!(
                            "failed to start proxy MCP server with config {}",
                            config_path.display()
                        ),
                        error,
                    )
                })?;
        }
        None => {
            if config_path.exists() {
                let _ = load_config_table(&config_path).map_err(|error| {
                    operation_error(
                        "cli.validate_config",
                        format!("failed to load config from {}", config_path.display()),
                        error,
                    )
                })?;
            }
        }
    }

    Ok(())
}

async fn install_with_replace(
    config_path: &std::path::Path,
    source: ImportSource,
    load_import_plan: fn() -> Result<(std::path::PathBuf, ImportPlan), Box<dyn Error>>,
    replace_target_servers: fn() -> Result<ReplaceMcpServersResult, Box<dyn Error>>,
    install_target_server: fn() -> Result<InstallMcpServerResult, Box<dyn Error>>,
    install_stage: &'static str,
    provider_name: &'static str,
) -> Result<(), Box<dyn Error>> {
    let mut config = load_config_table(config_path).map_err(|error| {
        operation_error(
            "cli.install.replace.load_config",
            format!("failed to load config from {}", config_path.display()),
            error,
        )
    })?;
    let provider = resolve_install_import_provider(source).map_err(|error| {
        operation_error(
            "cli.install.replace.load_provider",
            format!(
                "failed to load the provider configuration before importing into {}",
                config_path.display()
            ),
            error,
        )
    })?;
    let (source_config_path, import_plan) = load_import_plan().map_err(|error| {
        operation_error(
            "cli.install.replace.load_source",
            format!(
                "failed to load importable MCP servers from {}",
                provider_name
            ),
            error,
        )
    })?;

    let import_stage = format!("{install_stage}.replace.import");
    let mut imported_servers = Vec::new();
    let mut skipped_existing_servers = Vec::new();

    for server in import_plan.servers {
        if contains_server_name(&config, &server.name) {
            skipped_existing_servers.push(server.name);
            continue;
        }

        let server_name = import_server(config_path, &server).map_err(|error| {
            operation_error(
                "cli.install.replace.import",
                format!(
                    "failed to import MCP server `{}` from {} into {}",
                    server.name,
                    source_config_path.display(),
                    config_path.display()
                ),
                error,
            )
        })?;
        if server.enabled {
            let reload_result = reload_server_with_provider(config_path, &server_name, &provider)
                .await
                .map_err(|error| {
                    operation_error(
                        "cli.install.replace.reload",
                        format!(
                            "failed to reload imported MCP server `{server_name}` from {}",
                            source_config_path.display()
                        ),
                        error,
                    )
                })?;
            imported_servers.push(format!(
                "Imported `{server_name}` [enabled] and cached tools at {}",
                reload_result.cache_path.display()
            ));
        } else {
            imported_servers.push(format!(
                "Imported `{server_name}` [disabled] without reloading cached tools"
            ));
        }
        config = load_config_table(config_path).map_err(|error| {
            operation_error(
                "cli.install.replace.refresh_config",
                format!("failed to refresh config from {}", config_path.display()),
                error,
            )
        })?;
    }

    print_app_event(
        &import_stage,
        format!(
            "Imported {} MCP server(s) from {} into {} before replacing {} MCP config",
            imported_servers.len(),
            source_config_path.display(),
            config_path.display(),
            provider_name
        ),
    );
    for message in imported_servers {
        print_app_event(&format!("{import_stage}.server"), message);
    }
    for name in skipped_existing_servers {
        print_app_event(
            &format!("{import_stage}.skipped"),
            format!("Skipped existing server `{name}`"),
        );
    }
    for name in import_plan.skipped_self_servers {
        print_app_event(
            &format!("{import_stage}.skipped"),
            format!("Skipped self-referential server `{name}`"),
        );
    }

    let replaced = replace_target_servers().map_err(|error| {
        operation_error(
            "cli.install.replace.backup",
            format!("failed to back up and clear {provider_name} MCP servers"),
            error,
        )
    })?;
    print_replace_result(&format!("{install_stage}.replace.backup"), &replaced);

    let installed = install_target_server().map_err(|error| {
        operation_error(
            install_stage,
            format!("failed to install msp into {} config", provider_name),
            error,
        )
    })?;
    print_install_result(install_stage, provider_name, &installed);

    Ok(())
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

fn resolve_default_command_provider(
    provider_override: Option<ProviderName>,
) -> Result<ModelProviderConfig, Box<dyn Error>> {
    let provider = provider_override.ok_or_else(|| {
        "missing required `--provider`; supported providers are `codex` and `opencode`".to_string()
    })?;
    load_model_provider_config(provider.as_str())
}

fn resolve_import_provider(
    provider_override: Option<ProviderName>,
    source: ImportSource,
) -> Result<ModelProviderConfig, Box<dyn Error>> {
    match provider_override {
        Some(provider) => load_model_provider_config(provider.as_str()),
        None => load_model_provider_config(import_source_provider_name(source)),
    }
}

fn resolve_install_import_provider(
    source: ImportSource,
) -> Result<ModelProviderConfig, Box<dyn Error>> {
    load_model_provider_config(import_source_provider_name(source))
}

fn import_source_provider_name(source: ImportSource) -> &'static str {
    match source {
        ImportSource::Codex => "codex",
        ImportSource::Opencode => "opencode",
    }
}

#[cfg(test)]
fn missing_provider_error() -> &'static str {
    "missing required `--provider`; supported providers are `codex` and `opencode`"
}

fn print_install_result(stage: &str, provider: &str, installed: &InstallMcpServerResult) {
    let command_line = format!("msp mcp --provider {provider}");
    let message = match installed.status {
        InstallMcpServerStatus::AlreadyInstalled => format!(
            "MCP server `{}` already exists in {} with command `{command_line}`",
            installed.name,
            installed.config_path.display()
        ),
        InstallMcpServerStatus::Updated => format!(
            "Updated MCP server `{}` in {} to command `{command_line}`",
            installed.name,
            installed.config_path.display()
        ),
        InstallMcpServerStatus::Installed => format!(
            "Installed MCP server `{}` into {} with command `{command_line}`",
            installed.name,
            installed.config_path.display()
        ),
    };

    print_app_event(stage, message);
}

fn print_replace_result(stage: &str, replaced: &ReplaceMcpServersResult) {
    let message = format!(
        "Backed up {} MCP server(s) from {} to {} and removed {} MCP server(s) before install",
        replaced.backed_up_server_count,
        replaced.config_path.display(),
        replaced.backup_path.display(),
        replaced.removed_server_count,
    );

    print_app_event(stage, message);
}

fn print_restore_result(stage: &str, provider: &str, restored: &RestoreMcpServersResult) {
    let message = format!(
        "Removed {} `msp mcp` server(s) from {} {} config and restored {} MCP server(s) from {}",
        restored.removed_self_server_count,
        provider,
        restored.config_path.display(),
        restored.restored_server_count,
        restored.backup_path.display(),
    );

    print_app_event(stage, message);
}

fn print_server_config(
    stage: &str,
    config_path: &std::path::Path,
    snapshot: &ServerConfigSnapshot,
) {
    print_app_event(
        stage,
        format!("Server `{}` in {}", snapshot.name, config_path.display()),
    );
    print_app_event(stage, format!("transport: {}", snapshot.transport));
    print_app_event(stage, format!("enabled: {}", snapshot.enabled));
    print_app_event(stage, format!("command: {}", snapshot.command));
    if snapshot.args.is_empty() {
        print_app_event(stage, "args: []");
    } else {
        print_app_event(stage, format!("args: [{}]", snapshot.args.join(", ")));
    }
    if snapshot.env.is_empty() {
        print_app_event(stage, "env: {}");
    } else {
        for (key, value) in &snapshot.env {
            print_app_event(stage, format!("env.{key}: {value}"));
        }
    }
    if snapshot.env_vars.is_empty() {
        print_app_event(stage, "env_vars: []");
    } else {
        print_app_event(
            stage,
            format!("env_vars: [{}]", snapshot.env_vars.join(", ")),
        );
    }
}

fn parse_env_assignments(
    assignments: &[String],
) -> Result<std::collections::BTreeMap<String, String>, Box<dyn Error>> {
    let mut env = std::collections::BTreeMap::new();

    for assignment in assignments {
        let Some((key, value)) = assignment.split_once('=') else {
            return Err(
                format!("invalid env assignment `{assignment}`; expected `KEY=VALUE`").into(),
            );
        };
        if key.is_empty() {
            return Err(
                format!("invalid env assignment `{assignment}`; key must not be empty").into(),
            );
        }
        env.insert(key.to_string(), value.to_string());
    }

    Ok(env)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

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
    fn parses_env_assignments_into_sorted_map() {
        let env = parse_env_assignments(&[
            "B=two".to_string(),
            "A=one".to_string(),
            "B=override".to_string(),
        ])
        .unwrap();

        assert_eq!(
            env,
            BTreeMap::from([
                ("A".to_string(), "one".to_string()),
                ("B".to_string(), "override".to_string()),
            ])
        );
    }

    #[test]
    fn rejects_invalid_env_assignment() {
        let error = parse_env_assignments(&["INVALID".to_string()]).unwrap_err();

        assert_eq!(
            error.to_string(),
            "invalid env assignment `INVALID`; expected `KEY=VALUE`"
        );
    }
}
