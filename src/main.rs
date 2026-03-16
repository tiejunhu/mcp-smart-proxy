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

use cli::{Cli, Command, ConfigCommand, ImportSource};
use config::{
    CodexConfigUpdate, OpenAiConfigUpdate, add_server, contains_server_name, list_servers,
    load_codex_servers_for_import, load_config_table, load_default_model_provider_config,
    remove_server, update_codex_config, update_openai_config,
};
use console::{describe_command, operation_error, print_app_error, print_app_event};
use paths::expand_tilde;
use reload::reload_server;

#[tokio::main]
async fn main() {
    if let Err(error) = run().await {
        print_app_error(error.as_ref());
        std::process::exit(1);
    }
}

async fn run() -> Result<(), Box<dyn Error>> {
    let cli = Cli::parse();
    let config_path = expand_tilde(&cli.config).map_err(|error| {
        operation_error("cli.config_path", "failed to resolve config path", error)
    })?;

    match cli.command {
        Some(Command::Add { name, command }) => {
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
            let reload_result =
                reload_server(&config_path, &server_name)
                    .await
                    .map_err(|error| {
                        operation_error(
                            "cli.add.reload",
                            format!("failed to reload newly added MCP server `{server_name}`"),
                            error,
                        )
                    })?;
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

            print_app_event(
                "cli.list",
                format!(
                    "Configured {} MCP server(s) in {}",
                    servers.len(),
                    config_path.display()
                ),
            );

            for server in servers {
                let command_line = describe_command(&server.command, &server.args);
                let last_updated = format_last_updated(server.last_updated_at);
                print_app_event(
                    "cli.list.server",
                    format!(
                        "{} -> {} | last_updated: {}",
                        server.name, command_line, last_updated
                    ),
                );
            }
        }
        Some(Command::Import {
            source: ImportSource::Codex,
        }) => {
            let mut config = load_config_table(&config_path).map_err(|error| {
                operation_error(
                    "cli.import.codex.validate_provider.load_config",
                    format!("failed to load config from {}", config_path.display()),
                    error,
                )
            })?;
            let _ = load_default_model_provider_config(&config).map_err(|error| {
                operation_error(
                    "cli.import.codex.validate_provider",
                    format!(
                        "failed to validate model provider before importing from Codex into {}",
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

                let server_name =
                    add_server(&config_path, &server.name, server.command).map_err(|error| {
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
                let reload_result =
                    reload_server(&config_path, &server_name)
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
                    "{server_name} -> {}",
                    reload_result.cache_path.display()
                ));
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
                    "Imported {} MCP server(s) from {} into {}{}{}",
                    imported_servers.len(),
                    codex_config_path.display(),
                    config_path.display(),
                    if imported_servers.is_empty() {
                        "".to_string()
                    } else {
                        format!(": {}", imported_servers.join(", "))
                    },
                    if skipped_existing_servers.is_empty()
                        && import_plan.skipped_self_servers.is_empty()
                    {
                        "".to_string()
                    } else {
                        let mut skipped_parts = Vec::new();
                        if !skipped_existing_servers.is_empty() {
                            skipped_parts.push(format!(
                                "existing server(s): {}",
                                skipped_existing_servers.join(", ")
                            ));
                        }
                        if !import_plan.skipped_self_servers.is_empty() {
                            skipped_parts.push(format!(
                                "self-referential server(s): {}",
                                import_plan.skipped_self_servers.join(", ")
                            ));
                        }

                        format!("; skipped {}", skipped_parts.join("; "))
                    }
                ),
            );
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
                    "Removed MCP server `{}` from {}; {}",
                    removed.name,
                    config_path.display(),
                    cache_message
                ),
            );
        }
        Some(Command::Reload { name: Some(name) }) => {
            let reload_result = reload_server(&config_path, &name).await.map_err(|error| {
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
                        "Reloaded MCP server `{name}` into {}",
                        reload_result.cache_path.display()
                    )
                } else {
                    format!(
                        "Skipped cache update for MCP server `{name}` because the fetched tools matched {}",
                        reload_result.cache_path.display()
                    )
                },
            );
        }
        Some(Command::Reload { name: None }) => {
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
                let mut results = Vec::new();
                for server in servers {
                    let server_name = server.name;
                    let reload_result =
                        reload_server(&config_path, &server_name)
                            .await
                            .map_err(|error| {
                                operation_error(
                                    "cli.reload.all",
                                    format!("failed to reload MCP server `{server_name}`"),
                                    error,
                                )
                            })?;
                    let status = if reload_result.updated {
                        "updated"
                    } else {
                        "unchanged"
                    };
                    results.push(format!(
                        "{server_name} -> {} ({status})",
                        reload_result.cache_path.display()
                    ));
                }

                print_app_event(
                    "cli.reload",
                    format!(
                        "Reloaded {} MCP server(s) from {}: {}",
                        results.len(),
                        config_path.display(),
                        results.join(", ")
                    ),
                );
            }
        }
        Some(Command::Mcp) => {
            mcp_server::serve_cached_toolsets(&config_path)
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
        Some(Command::Config {
            command:
                ConfigCommand::Openai {
                    baseurl,
                    key,
                    model,
                    make_default,
                },
        }) => {
            update_openai_config(
                &config_path,
                OpenAiConfigUpdate {
                    baseurl,
                    key,
                    model,
                    make_default,
                },
            )
            .map_err(|error| {
                operation_error(
                    "cli.config.openai",
                    format!(
                        "failed to update OpenAI config in {}",
                        config_path.display()
                    ),
                    error,
                )
            })?;
            print_app_event(
                "cli.config.openai",
                format!("Updated OpenAI config in {}", config_path.display()),
            );
        }
        Some(Command::Config {
            command:
                ConfigCommand::Codex {
                    model,
                    make_default,
                },
        }) => {
            update_codex_config(
                &config_path,
                CodexConfigUpdate {
                    model,
                    make_default,
                },
            )
            .map_err(|error| {
                operation_error(
                    "cli.config.codex",
                    format!("failed to update Codex config in {}", config_path.display()),
                    error,
                )
            })?;
            print_app_event(
                "cli.config.codex",
                format!("Updated Codex config in {}", config_path.display()),
            );
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
}
