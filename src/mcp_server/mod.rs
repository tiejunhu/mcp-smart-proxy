mod cache;
mod client;
mod server;
#[cfg(test)]
mod tests;
mod tools;

use std::error::Error;
use std::io::IsTerminal;
use std::path::Path;

use rmcp::{ServiceExt, transport::stdio};

use crate::config::{list_servers, load_config_table};
use crate::console::operation_error;
use crate::paths::format_path_for_display;
use crate::reload::reload_server_with_provider;
use crate::types::ModelProviderConfig;

use self::cache::load_cached_toolsets;
use self::server::SmartProxyMcpServer;

pub async fn serve_cached_toolsets(
    config_path: &Path,
    provider: ModelProviderConfig,
) -> Result<(), Box<dyn Error>> {
    ensure_proxy_stdio_host_connection()?;

    reload_all_toolsets(config_path, &provider)
        .await
        .map_err(|error| {
            operation_error(
                "mcp.reload_all_toolsets",
                format!(
                    "failed to reload configured MCP servers before starting proxy with config {}",
                    format_path_for_display(config_path)
                ),
                error,
            )
        })?;

    let config = load_config_table(config_path).map_err(|error| {
        operation_error(
            "mcp.load_config",
            format!(
                "failed to load config from {}",
                format_path_for_display(config_path)
            ),
            error,
        )
    })?;
    let toolsets = load_cached_toolsets(&config).map_err(|error| {
        operation_error(
            "mcp.load_toolsets",
            "failed to load cached toolsets from config",
            error,
        )
    })?;
    let service = SmartProxyMcpServer::new(toolsets)
        .serve(stdio())
        .await
        .map_err(map_proxy_serve_error)?;
    service.waiting().await.map_err(|error| {
        operation_error(
            "mcp.wait",
            "proxy stdio MCP server exited with an error",
            Box::new(error),
        )
    })?;
    Ok(())
}

fn ensure_proxy_stdio_host_connection() -> Result<(), Box<dyn Error>> {
    validate_proxy_stdio_launch(
        std::io::stdin().is_terminal(),
        std::io::stdout().is_terminal(),
    )
}

fn validate_proxy_stdio_launch(
    stdin_is_terminal: bool,
    stdout_is_terminal: bool,
) -> Result<(), Box<dyn Error>> {
    if stdin_is_terminal || stdout_is_terminal {
        return Err(operation_error(
            "mcp.serve.stdio_host",
            tools::STDIO_HOST_REQUIRED_MESSAGE,
            "stdio MCP servers require an upstream host connection over stdin/stdout".into(),
        ));
    }

    Ok(())
}

fn map_proxy_serve_error(error: impl Error + 'static) -> Box<dyn Error> {
    if error.to_string() == "connection closed: initialize request" {
        return operation_error(
            "mcp.serve.initialize",
            tools::STDIO_HOST_REQUIRED_MESSAGE,
            Box::new(error),
        );
    }

    operation_error(
        "mcp.serve",
        "failed to start the proxy stdio MCP server",
        Box::new(error),
    )
}

async fn reload_all_toolsets(
    config_path: &Path,
    provider: &ModelProviderConfig,
) -> Result<(), Box<dyn Error>> {
    let servers = list_servers(config_path).map_err(|error| {
        operation_error(
            "mcp.reload_all_toolsets.list_servers",
            format!(
                "failed to list configured MCP servers from {} before startup reload",
                format_path_for_display(config_path)
            ),
            error,
        )
    })?;

    for server in servers.into_iter().filter(|server| server.enabled) {
        let server_name = server.name;
        reload_server_with_provider(config_path, &server_name, provider)
            .await
            .map_err(|error| {
                operation_error(
                    "mcp.reload_all_toolsets.reload_server",
                    format!("failed to reload MCP server `{server_name}` before proxy startup"),
                    error,
                )
            })?;
    }

    Ok(())
}
