use std::collections::HashMap;
use std::error::Error;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::Mutex as StdMutex;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use rmcp::{RoleClient, model::CallToolRequestParams, service::RunningService};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::{Mutex, watch};

use crate::config::{
    configured_server, list_servers, load_config_table, load_model_provider_config,
};
use crate::console::{ExternalOutputRouter, print_external_command_failure_with_captured_stderr};
use crate::downstream_client::connect_stdio_client;
use crate::mcp_server::cache::load_cached_toolsets;
use crate::reload::reload_server_with_provider;
use crate::remote::connect_remote_client;
use crate::types::{
    CachedToolsetRecord, ConfiguredServer, ConfiguredTransport, DaemonRequest, DaemonResponse,
    DaemonStatus,
};

const DAEMON_IDLE_TIMEOUT: Duration = Duration::from_secs(60 * 60);
const IDLE_POLL_INTERVAL: Duration = Duration::from_secs(15);

pub(crate) async fn serve_daemon(
    listener: UnixListener,
    config_path: PathBuf,
    socket_path: PathBuf,
) -> Result<(), Box<dyn Error>> {
    let (shutdown_tx, mut shutdown_rx) = watch::channel(false);
    let state = Arc::new(DaemonState {
        config_path,
        socket_path,
        registry: ClientRegistry::default(),
        active_requests: AtomicUsize::new(0),
        last_activity: StdMutex::new(Instant::now()),
        shutdown_tx,
    });
    let mut idle_interval = tokio::time::interval(IDLE_POLL_INTERVAL);

    loop {
        tokio::select! {
            result = listener.accept() => {
                let (stream, _) = result?;
                let state = Arc::clone(&state);
                tokio::spawn(async move {
                    if let Err(error) = handle_connection(stream, state).await {
                        eprintln!("warning: daemon request failed: {error}");
                    }
                });
            }
            _ = idle_interval.tick() => {
                if state.should_exit_for_idle() {
                    return Ok(());
                }
            }
            result = shutdown_rx.changed() => {
                result.map_err(|error| format!("failed to observe daemon shutdown: {error}"))?;
                if *shutdown_rx.borrow() {
                    return Ok(());
                }
            }
            result = shutdown_signal() => {
                result?;
                let _ = state.shutdown_tx.send(true);
                return Ok(());
            }
        }
    }
}

struct DaemonState {
    config_path: PathBuf,
    socket_path: PathBuf,
    registry: ClientRegistry,
    active_requests: AtomicUsize,
    last_activity: StdMutex<Instant>,
    shutdown_tx: watch::Sender<bool>,
}

impl DaemonState {
    fn touch(&self) {
        let mut last_activity = self.last_activity.lock().unwrap();
        *last_activity = Instant::now();
    }

    fn should_exit_for_idle(&self) -> bool {
        if self.active_requests.load(Ordering::SeqCst) != 0 {
            return false;
        }
        let last_activity = *self.last_activity.lock().unwrap();
        last_activity.elapsed() >= DAEMON_IDLE_TIMEOUT
    }
}

async fn handle_connection(
    stream: UnixStream,
    state: Arc<DaemonState>,
) -> Result<(), Box<dyn Error>> {
    state.active_requests.fetch_add(1, Ordering::SeqCst);
    state.touch();

    let result = handle_connection_inner(stream, Arc::clone(&state)).await;

    state.touch();
    state.active_requests.fetch_sub(1, Ordering::SeqCst);
    result
}

async fn handle_connection_inner(
    stream: UnixStream,
    state: Arc<DaemonState>,
) -> Result<(), Box<dyn Error>> {
    let (reader, mut writer) = stream.into_split();
    let mut reader = BufReader::new(reader);
    let mut line = String::new();
    let bytes = reader.read_line(&mut line).await?;
    if bytes == 0 {
        return Ok(());
    }

    let request: DaemonRequest = serde_json::from_str(line.trim())?;
    let response = match request {
        DaemonRequest::Status => DaemonResponse::Status {
            status: DaemonStatus {
                version: env!("CARGO_PKG_VERSION").to_string(),
                pid: std::process::id(),
                socket_path: state.socket_path.display().to_string(),
                config_path: state.config_path.display().to_string(),
            },
        },
        DaemonRequest::Exit => {
            let _ = state.shutdown_tx.send(true);
            DaemonResponse::ExitAck
        }
        DaemonRequest::LoadToolsets { provider } => {
            match load_toolsets_for_provider(&state.config_path, &provider).await {
                Ok(toolsets) => DaemonResponse::Toolsets { toolsets },
                Err(error) => DaemonResponse::Error {
                    message: error.to_string(),
                },
            }
        }
        DaemonRequest::CallTool {
            toolset_name,
            tool_name,
            arguments,
        } => match call_tool_with_registry(&state, &toolset_name, &tool_name, arguments).await {
            Ok(result) => DaemonResponse::ToolResult { result },
            Err(error) => DaemonResponse::Error {
                message: error.to_string(),
            },
        },
    };

    let payload = serde_json::to_string(&response)?;
    writer.write_all(payload.as_bytes()).await?;
    writer.write_all(b"\n").await?;
    writer.shutdown().await?;
    Ok(())
}

async fn load_toolsets_for_provider(
    config_path: &Path,
    provider_name: &str,
) -> Result<Vec<CachedToolsetRecord>, Box<dyn Error>> {
    let provider = load_model_provider_config(provider_name)?;
    let servers = list_servers(config_path)?;
    for server in servers.into_iter().filter(|server| server.enabled) {
        reload_server_with_provider(config_path, &server.name, &provider).await?;
    }

    let config = load_config_table(config_path)?;
    Ok(load_cached_toolsets(&config)?
        .into_iter()
        .map(|toolset| CachedToolsetRecord {
            name: toolset.name,
            summary: toolset.summary,
            tools: toolset.tools,
        })
        .collect())
}

async fn call_tool_with_registry(
    state: &DaemonState,
    toolset_name: &str,
    tool_name: &str,
    arguments: Option<serde_json::Map<String, serde_json::Value>>,
) -> Result<rmcp::model::CallToolResult, Box<dyn Error>> {
    let config = load_config_table(&state.config_path)?;
    let (resolved_name, server) = configured_server(&config, toolset_name)?;
    let client = state
        .registry
        .get_or_connect(&resolved_name, &server)
        .await?;
    let stderr_capture = client.stderr.start_capture().await;
    let request = match arguments {
        Some(arguments) => {
            CallToolRequestParams::new(tool_name.to_string()).with_arguments(arguments)
        }
        None => CallToolRequestParams::new(tool_name.to_string()),
    };

    match client.service.call_tool(request).await {
        Ok(result) => {
            let _ = stderr_capture.finish().await;
            Ok(result)
        }
        Err(error) => {
            let stderr_content = stderr_capture.finish().await;
            print_external_command_failure_with_captured_stderr(
                "daemon.call_tool",
                &client.label,
                &client.command_line,
                "tool-call-failed",
                &stderr_content,
            )
            .await;
            Err(Box::new(error))
        }
    }
}

#[derive(Clone)]
struct ToolsetClient {
    service: Arc<RunningService<RoleClient, ()>>,
    stderr: ExternalOutputRouter,
    command_line: String,
    label: String,
}

#[derive(Clone)]
struct ClientEntry {
    server: ConfiguredServer,
    client: ToolsetClient,
}

#[derive(Default)]
struct ClientRegistry {
    slots: Mutex<HashMap<String, ClientEntry>>,
}

impl ClientRegistry {
    async fn get_or_connect(
        &self,
        server_name: &str,
        server: &ConfiguredServer,
    ) -> Result<ToolsetClient, Box<dyn Error>> {
        let mut slots = self.slots.lock().await;

        if let Some(entry) = slots.get(server_name)
            && entry.server == *server
            && !entry.client.service.is_closed()
        {
            return Ok(entry.client.clone());
        }

        if let Some(entry) = slots.remove(server_name) {
            entry.client.service.cancellation_token().cancel();
        }

        let client = connect_toolset_client(server_name, server).await?;
        slots.insert(
            server_name.to_string(),
            ClientEntry {
                server: server.clone(),
                client: client.clone(),
            },
        );
        Ok(client)
    }
}

async fn connect_toolset_client(
    server_name: &str,
    server: &ConfiguredServer,
) -> Result<ToolsetClient, Box<dyn Error>> {
    match &server.transport {
        ConfiguredTransport::Stdio { command, args } => {
            let label = command.clone();
            let client = connect_stdio_client(
                "daemon.connect_toolset_client",
                "daemon.connect_toolset_client.spawn",
                "daemon.connect_toolset_client.connect",
                label.clone(),
                command,
                args,
                server.resolved_env(),
            )
            .await?;

            Ok(ToolsetClient {
                service: Arc::new(client.service),
                stderr: client.stderr,
                command_line: client.command_line,
                label,
            })
        }
        ConfiguredTransport::Remote { url, .. } => {
            let label = format!("remote:{url}");
            let client = connect_remote_client(server_name, server).await?;
            Ok(ToolsetClient {
                service: Arc::new(client),
                stderr: ExternalOutputRouter::new(),
                command_line: url.clone(),
                label,
            })
        }
    }
}

async fn shutdown_signal() -> io::Result<()> {
    let mut terminate = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;
    tokio::select! {
        result = tokio::signal::ctrl_c() => result,
        _ = terminate.recv() => Ok(()),
    }
}
