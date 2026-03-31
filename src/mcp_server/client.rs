use std::collections::HashMap;
use std::sync::Arc;

use rmcp::{ErrorData as McpError, RoleClient, service::RunningService};
use tokio::sync::Mutex;

use crate::console::ExternalOutputRouter;
use crate::downstream_client::connect_stdio_client;
use crate::remote::connect_remote_client;
use crate::types::{ConfiguredServer, ConfiguredTransport};

use super::cache::CachedToolsetRecord;

#[derive(Clone)]
pub(super) struct ToolsetClient {
    pub(super) service: Arc<RunningService<RoleClient, ()>>,
    pub(super) stderr: ExternalOutputRouter,
    pub(super) command_line: String,
    pub(super) label: String,
}

pub(super) struct ClientRegistry {
    slots: HashMap<String, Arc<Mutex<Option<ToolsetClient>>>>,
}

impl ClientRegistry {
    pub(super) fn new(toolsets: &[CachedToolsetRecord]) -> Self {
        let slots = toolsets
            .iter()
            .map(|toolset| (toolset.name.clone(), Arc::new(Mutex::new(None))))
            .collect();
        Self { slots }
    }

    pub(super) async fn get_or_connect(
        &self,
        toolset: &CachedToolsetRecord,
    ) -> Result<ToolsetClient, McpError> {
        let slot = self
            .slots
            .get(&toolset.name)
            .cloned()
            .ok_or_else(|| McpError::internal_error("missing client slot for toolset", None))?;
        let mut client_guard = slot.lock().await;

        if let Some(client) = client_guard.as_ref() {
            if !client.service.is_closed() {
                return Ok(client.clone());
            }
            *client_guard = None;
        }

        let client = connect_toolset_client(&toolset.name, &toolset.server).await?;
        *client_guard = Some(client.clone());
        Ok(client)
    }
}

async fn connect_toolset_client(
    server_name: &str,
    server: &ConfiguredServer,
) -> Result<ToolsetClient, McpError> {
    match &server.transport {
        ConfiguredTransport::Stdio { command, args } => {
            let label = command.clone();
            let client = connect_stdio_client(
                "mcp.connect_toolset_client",
                "mcp.connect_toolset_client.spawn",
                "mcp.connect_toolset_client.connect",
                label.clone(),
                command,
                args,
                server.resolved_env(),
            )
            .await
            .map_err(|error| McpError::internal_error(error.to_string(), None))?;

            Ok(ToolsetClient {
                service: Arc::new(client.service),
                stderr: client.stderr,
                command_line: client.command_line,
                label,
            })
        }
        ConfiguredTransport::Remote { url, .. } => {
            let label = format!("remote:{url}");
            let client = connect_remote_client(server_name, server)
                .await
                .map_err(|error| McpError::internal_error(error.to_string(), None))?;
            Ok(ToolsetClient {
                service: Arc::new(client),
                stderr: ExternalOutputRouter::new(),
                command_line: url.clone(),
                label,
            })
        }
    }
}
