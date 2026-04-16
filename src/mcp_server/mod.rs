pub(crate) mod cache;
mod server;
#[cfg(test)]
mod tests;
mod tools;

use std::error::Error;
use std::io::IsTerminal;
use std::path::Path;

use rmcp::{ServiceExt, transport::stdio};

use self::server::SmartProxyMcpServer;
use crate::console::operation_error;
use crate::daemon;
use crate::paths::format_path_for_display;
use crate::types::ModelProviderConfig;

pub async fn serve_cached_toolsets(
    config_path: &Path,
    provider: ModelProviderConfig,
    enable_input: bool,
    output_toon: bool,
) -> Result<(), Box<dyn Error>> {
    ensure_proxy_stdio_host_connection()?;
    daemon::ensure_daemon_running(config_path, None)
        .await
        .map_err(|error| {
            operation_error(
                "mcp.ensure_daemon",
                format!(
                    "failed to start or connect to the daemon for config {}",
                    format_path_for_display(config_path)
                ),
                error,
            )
        })?;
    let toolsets = daemon::load_toolsets(config_path, None, Some(provider.provider_name()))
        .await
        .map_err(|error| {
            operation_error(
                "mcp.load_toolsets",
                "failed to load cached toolsets from the daemon",
                error,
            )
        })?;
    let service = SmartProxyMcpServer::new(
        config_path.to_path_buf(),
        toolsets,
        enable_input,
        output_toon,
    )
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
