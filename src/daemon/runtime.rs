use std::collections::HashMap;
use std::error::Error;
use std::future::Future;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::Mutex as StdMutex;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use rmcp::{RoleClient, model::CallToolRequestParams, service::RunningService};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::{Mutex, watch};

use super::logging::DaemonLogger;
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
    logger: DaemonLogger,
) -> Result<(), Box<dyn Error>> {
    let (shutdown_tx, mut shutdown_rx) = watch::channel(false);
    let state = Arc::new(DaemonState {
        config_path,
        socket_path,
        logger,
        registry: ClientRegistry::default(),
        load_toolsets: LoadToolsetsCoordinator::default(),
        next_request_id: AtomicU64::new(1),
        active_requests: AtomicUsize::new(0),
        last_activity: StdMutex::new(Instant::now()),
        shutdown_tx,
    });
    let mut idle_interval = tokio::time::interval(IDLE_POLL_INTERVAL);
    state.logger.info(
        "daemon.listen",
        format!(
            "daemon listening on {} for {}",
            state.socket_path.display(),
            state.config_path.display()
        ),
    );

    loop {
        tokio::select! {
            result = listener.accept() => {
                let (stream, _) = match result {
                    Ok(result) => result,
                    Err(error) => {
                        state.logger.error("daemon.accept_failed", error.to_string());
                        return Err(Box::new(error));
                    }
                };
                let state = Arc::clone(&state);
                let request_id = state.next_request_id.fetch_add(1, Ordering::SeqCst);
                state.logger.info(
                    "daemon.accepted",
                    format!("request_id={request_id} active_requests={}", state.active_requests.load(Ordering::SeqCst)),
                );
                tokio::spawn(async move {
                    let _ = handle_connection(stream, state, request_id).await;
                });
            }
            _ = idle_interval.tick() => {
                if state.should_exit_for_idle() {
                    state.logger.info("daemon.idle_exit", "daemon exited after idle timeout");
                    return Ok(());
                }
            }
            result = shutdown_rx.changed() => {
                result.map_err(|error| {
                    state.logger.error("daemon.shutdown_watch_failed", error.to_string());
                    format!("failed to observe daemon shutdown: {error}")
                })?;
                if *shutdown_rx.borrow() {
                    state.logger.info("daemon.shutdown_requested", "shutdown requested by daemon client");
                    return Ok(());
                }
            }
            result = shutdown_signal() => {
                result.inspect_err(|error| {
                    state.logger.error("daemon.signal_failed", error.to_string());
                })?;
                state.logger.info("daemon.signal_exit", "shutdown signal received");
                let _ = state.shutdown_tx.send(true);
                return Ok(());
            }
        }
    }
}

struct DaemonState {
    config_path: PathBuf,
    socket_path: PathBuf,
    logger: DaemonLogger,
    registry: ClientRegistry,
    load_toolsets: LoadToolsetsCoordinator,
    next_request_id: AtomicU64,
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
    request_id: u64,
) -> Result<(), Box<dyn Error>> {
    state.active_requests.fetch_add(1, Ordering::SeqCst);
    state.touch();
    let started_at = Instant::now();

    let result = handle_connection_inner(stream, Arc::clone(&state), request_id).await;

    state.touch();
    let active_requests = state.active_requests.fetch_sub(1, Ordering::SeqCst) - 1;
    match &result {
        Ok(()) => state.logger.info(
            "daemon.request_finished",
            format!(
                "request_id={request_id} elapsed_ms={} active_requests={active_requests}",
                started_at.elapsed().as_millis()
            ),
        ),
        Err(error) => state.logger.error(
            "daemon.request_failed",
            format!(
                "request_id={request_id} elapsed_ms={} active_requests={active_requests} error={error}",
                started_at.elapsed().as_millis()
            ),
        ),
    }
    result
}

async fn handle_connection_inner(
    stream: UnixStream,
    state: Arc<DaemonState>,
    request_id: u64,
) -> Result<(), Box<dyn Error>> {
    let (reader, mut writer) = stream.into_split();
    let mut reader = BufReader::new(reader);
    let mut line = String::new();
    let bytes = reader.read_line(&mut line).await?;
    if bytes == 0 {
        state
            .logger
            .info("daemon.empty_request", format!("request_id={request_id}"));
        return Ok(());
    }

    let request: DaemonRequest = serde_json::from_str(line.trim())?;
    let request_name = daemon_request_name(&request);
    state.logger.info(
        "daemon.request_received",
        format!("request_id={request_id} type={request_name}"),
    );
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
            match state
                .load_toolsets
                .run(&provider, || {
                    let config_path = state.config_path.clone();
                    let provider = provider.clone();
                    async move { load_toolsets_for_provider(&config_path, &provider).await }
                })
                .await
            {
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
    let response_name = daemon_response_name(&response);

    let payload = serde_json::to_string(&response)?;
    writer.write_all(payload.as_bytes()).await?;
    writer.write_all(b"\n").await?;
    writer.shutdown().await?;
    state.logger.info(
        "daemon.response_sent",
        format!(
            "request_id={request_id} request_type={request_name} response_type={response_name}"
        ),
    );
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

type SharedLoadToolsetsResult = Result<Vec<CachedToolsetRecord>, String>;

#[derive(Default)]
struct LoadToolsetsCoordinator {
    slots: Mutex<HashMap<String, watch::Sender<Option<SharedLoadToolsetsResult>>>>,
}

impl LoadToolsetsCoordinator {
    async fn run<F, Fut>(
        &self,
        provider_name: &str,
        operation: F,
    ) -> Result<Vec<CachedToolsetRecord>, Box<dyn Error>>
    where
        F: FnOnce() -> Fut,
        Fut: Future<Output = Result<Vec<CachedToolsetRecord>, Box<dyn Error>>>,
    {
        let (mut receiver, is_leader) = {
            let mut slots = self.slots.lock().await;
            if let Some(sender) = slots.get(provider_name) {
                (sender.subscribe(), false)
            } else {
                let (sender, receiver) = watch::channel(None);
                slots.insert(provider_name.to_string(), sender);
                (receiver, true)
            }
        };

        if is_leader {
            let result = operation().await.map_err(|error| error.to_string());
            if let Some(sender) = self.slots.lock().await.remove(provider_name) {
                let _ = sender.send(Some(result.clone()));
            }
            return result.map_err(Into::into);
        }

        loop {
            if let Some(result) = receiver.borrow().clone() {
                return result.map_err(Into::into);
            }

            receiver.changed().await.map_err(|_| {
                format!(
                    "shared load_toolsets refresh ended unexpectedly for provider `{provider_name}`"
                )
            })?;
        }
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
    let mut hangup = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::hangup())?;
    let mut terminate = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;
    tokio::select! {
        result = tokio::signal::ctrl_c() => result,
        _ = hangup.recv() => Ok(()),
        _ = terminate.recv() => Ok(()),
    }
}

fn daemon_request_name(request: &DaemonRequest) -> &'static str {
    match request {
        DaemonRequest::Status => "status",
        DaemonRequest::Exit => "exit",
        DaemonRequest::LoadToolsets { .. } => "load_toolsets",
        DaemonRequest::CallTool { .. } => "call_tool",
    }
}

fn daemon_response_name(response: &DaemonResponse) -> &'static str {
    match response {
        DaemonResponse::Status { .. } => "status",
        DaemonResponse::ExitAck => "exit_ack",
        DaemonResponse::Toolsets { .. } => "toolsets",
        DaemonResponse::ToolResult { .. } => "tool_result",
        DaemonResponse::Error { .. } => "error",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use tokio::time::sleep;

    fn sample_toolsets() -> Vec<CachedToolsetRecord> {
        vec![CachedToolsetRecord {
            name: "demo".to_string(),
            summary: "Use demo.".to_string(),
            tools: Vec::new(),
        }]
    }

    #[tokio::test]
    async fn load_toolsets_coordinator_deduplicates_concurrent_refreshes() {
        let coordinator = LoadToolsetsCoordinator::default();
        let runs = Arc::new(AtomicUsize::new(0));

        let first_runs = Arc::clone(&runs);
        let first = coordinator.run("codex", move || async move {
            first_runs.fetch_add(1, Ordering::SeqCst);
            sleep(Duration::from_millis(50)).await;
            Ok(sample_toolsets())
        });

        let second_runs = Arc::clone(&runs);
        let second = coordinator.run("codex", move || async move {
            second_runs.fetch_add(1, Ordering::SeqCst);
            Ok(sample_toolsets())
        });

        let (first_result, second_result) = tokio::join!(first, second);

        assert_eq!(first_result.unwrap()[0].name, "demo");
        assert_eq!(second_result.unwrap()[0].name, "demo");
        assert_eq!(runs.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn load_toolsets_coordinator_shares_refresh_errors() {
        let coordinator = LoadToolsetsCoordinator::default();
        let first = coordinator.run("codex", || async {
            sleep(Duration::from_millis(50)).await;
            Err("refresh failed".into())
        });
        let second = coordinator.run("codex", || async { Ok(sample_toolsets()) });

        let (first_error, second_error) = tokio::join!(first, second);

        assert_eq!(first_error.unwrap_err().to_string(), "refresh failed");
        assert_eq!(second_error.unwrap_err().to_string(), "refresh failed");
    }
}
