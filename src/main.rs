use std::error::Error;

use clap::Parser;

mod cli;
mod config;
mod console;
mod mcp_server;
mod paths;
mod reload;
mod types;

use cli::{Cli, Command, ConfigCommand};
use config::{
    CodexConfigUpdate, OpenAiConfigUpdate, add_server, load_config_table, update_codex_config,
    update_openai_config,
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
