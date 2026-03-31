use std::error::Error;
use std::path::Path;

use crate::config::{configured_server, load_config_table};
use crate::console::{operation_error, print_app_event};
use crate::paths::format_path_for_display;
use crate::remote::{login_remote_server, logout_remote_server};
use crate::types::{ConfiguredServer, ConfiguredTransport};

fn load_remote_server_for_auth(
    config_path: &Path,
    requested_name: &str,
    load_stage: &'static str,
    resolve_stage: &'static str,
    unsupported_stage: &'static str,
    unsupported_message: &'static str,
) -> Result<(String, ConfiguredServer), Box<dyn Error>> {
    let config = load_config_table(config_path).map_err(|error| {
        operation_error(
            load_stage,
            format!(
                "failed to load config from {}",
                format_path_for_display(config_path)
            ),
            error,
        )
    })?;
    let (resolved_name, server) = configured_server(&config, requested_name).map_err(|error| {
        operation_error(
            resolve_stage,
            format!("failed to resolve configured server `{requested_name}`"),
            error,
        )
    })?;
    if !matches!(server.transport, ConfiguredTransport::Remote { .. }) {
        return Err(operation_error(
            unsupported_stage,
            format!("MCP server `{resolved_name}` is not configured as `remote`"),
            unsupported_message.into(),
        ));
    }

    Ok((resolved_name, server))
}

pub(super) async fn run_login_command(
    config_path: &Path,
    name: &str,
) -> Result<(), Box<dyn Error>> {
    let (resolved_name, server) = load_remote_server_for_auth(
        config_path,
        name,
        "cli.login.load_config",
        "cli.login.resolve_server",
        "cli.login.unsupported_transport",
        "only remote servers support OAuth login",
    )?;

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

pub(super) fn run_logout_command(config_path: &Path, name: &str) -> Result<(), Box<dyn Error>> {
    let (resolved_name, _server) = load_remote_server_for_auth(
        config_path,
        name,
        "cli.logout.load_config",
        "cli.logout.resolve_server",
        "cli.logout.unsupported_transport",
        "only remote servers store OAuth credentials",
    )?;

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
