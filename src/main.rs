use std::error::Error;

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
    CodexConfigUpdate, OpenAiConfigUpdate, add_server, contains_server_name,
    load_codex_servers_for_import, load_config_table, load_default_model_provider_config,
    update_codex_config, update_openai_config,
};
use console::{operation_error, print_app_error, print_app_event};
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
            let cache_path = reload_server(&config_path, &server_name)
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
                    cache_path.display()
                ),
            );
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
            let (codex_config_path, servers) =
                load_codex_servers_for_import().map_err(|error| {
                    operation_error(
                        "cli.import.codex.load_source",
                        "failed to load importable MCP servers from Codex config",
                        error,
                    )
                })?;

            let mut imported_servers = Vec::new();
            let mut skipped_servers = Vec::new();
            for server in servers {
                if contains_server_name(&config, &server.name) {
                    skipped_servers.push(server.name);
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
                let cache_path =
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
                imported_servers.push(format!("{server_name} -> {}", cache_path.display()));
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
                    if skipped_servers.is_empty() {
                        "".to_string()
                    } else {
                        format!(
                            "; skipped existing server(s): {}",
                            skipped_servers.join(", ")
                        )
                    }
                ),
            );
        }
        Some(Command::Reload { name }) => {
            let cache_path = reload_server(&config_path, &name).await.map_err(|error| {
                operation_error(
                    "cli.reload",
                    format!("failed to reload MCP server `{name}`"),
                    error,
                )
            })?;
            print_app_event(
                "cli.reload",
                format!("Reloaded MCP server `{name}` into {}", cache_path.display()),
            );
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
