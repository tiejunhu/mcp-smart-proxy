use std::error::Error;
use std::ffi::OsString;
use std::path::Path;

use chrono::{Local, TimeZone};
use clap::Parser;

#[cfg(test)]
use crate::cli::ImportSource;
use crate::cli::{Cli, Command, DaemonCommand, ProviderName};
use crate::config::{
    add_server, list_servers, load_config_table, load_server_config, remove_server,
    set_server_enabled, update_server_config,
};
use crate::console::{describe_command, operation_error, print_app_event};
use crate::daemon;
use crate::mcp_server;
use crate::paths::{expand_tilde, format_path_for_display};
use crate::reload::reload_server_with_provider;
#[cfg(test)]
use crate::types::ModelProviderConfig;
use crate::version_check;

mod auth_cmd;
mod config_cmd;
mod import_cmd;
mod provider;

use auth_cmd::{run_login_command, run_logout_command};
use config_cmd::{ConfigCommandArgs, print_server_config};
use import_cmd::{run_import_command, run_install_command, run_restore_command};
use provider::resolve_default_command_provider;
#[cfg(test)]
use provider::{resolve_import_provider, resolve_install_import_provider};

pub async fn run() -> Result<(), Box<dyn Error>> {
    let raw_args = std::env::args_os().collect::<Vec<OsString>>();
    let cli = Cli::parse();
    if matches!(
        &cli.command,
        Some(Command::Daemon {
            command: DaemonCommand::Run,
            ..
        })
    ) {
        version_check::prepare_executable_for_background_update(&raw_args);
        version_check::spawn_periodic_self_update(raw_args.clone());
    } else if !matches!(
        &cli.command,
        Some(Command::Update) | Some(Command::Daemon { .. })
    ) {
        version_check::print_cached_update_notice();
    }
    let config_path = expand_tilde(&cli.config).map_err(|error| {
        operation_error("cli.config_path", "failed to resolve config path", error)
    })?;

    match cli.command {
        Some(Command::Add { name, command }) => run_add_command(&config_path, &name, command)?,
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
        Some(Command::Daemon { socket, command }) => {
            run_daemon_command(&config_path, socket.as_deref(), command).await?
        }
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

async fn run_daemon_command(
    config_path: &Path,
    socket_override: Option<&Path>,
    command: DaemonCommand,
) -> Result<(), Box<dyn Error>> {
    match command {
        DaemonCommand::Run => daemon::run_daemon(config_path, socket_override).await,
        DaemonCommand::Status => {
            let status = daemon::request_status(config_path, socket_override).await?;
            print_app_event(
                "cli.daemon.status",
                format!(
                    "Daemon v{} pid {} on {} for {}",
                    status.version, status.pid, status.socket_path, status.config_path
                ),
            );
            Ok(())
        }
        DaemonCommand::Exit => {
            daemon::request_exit(config_path, socket_override).await?;
            print_app_event("cli.daemon.exit", "Daemon exit requested");
            Ok(())
        }
    }
}

fn run_add_command(
    config_path: &Path,
    name: &str,
    command: Vec<String>,
) -> Result<(), Box<dyn Error>> {
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
    print_app_event(
        "cli.add",
        format!(
            "Added MCP server `{server_name}` to {}; cached tools will refresh on `msp reload --provider ...` or `msp mcp --provider ...`",
            format_path_for_display(config_path),
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
    mcp_server::serve_cached_toolsets(config_path, resolved_provider)
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
    use super::provider::missing_provider_error;
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
