use std::collections::HashMap;
use std::collections::HashSet;
use std::error::Error;
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
        refresh_toolsets: RefreshToolsetsCoordinator::default(),
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
    refresh_toolsets: RefreshToolsetsCoordinator,
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
            match load_cached_toolsets_snapshot(&state.config_path)
                .map_err(|error| error.to_string())
            {
                Ok(toolsets) => {
                    state
                        .refresh_toolsets
                        .spawn(&provider, state.logger.clone(), {
                            let config_path = state.config_path.clone();
                            let provider = provider.clone();
                            move || async move {
                                refresh_toolsets_for_provider(&config_path, &provider).await
                            }
                        })
                        .await;
                    DaemonResponse::Toolsets { toolsets }
                }
                Err(message) => DaemonResponse::Error { message },
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

fn load_cached_toolsets_snapshot(
    config_path: &Path,
) -> Result<Vec<CachedToolsetRecord>, Box<dyn Error>> {
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

async fn refresh_toolsets_for_provider(
    config_path: &Path,
    provider_name: &str,
) -> Result<(), Box<dyn Error>> {
    let provider = load_model_provider_config(provider_name)?;
    let servers = list_servers(config_path)?;
    for server in servers.into_iter().filter(|server| server.enabled) {
        reload_server_with_provider(config_path, &server.name, &provider).await?;
    }

    Ok(())
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

#[derive(Clone, Default)]
struct RefreshToolsetsCoordinator {
    slots: Arc<Mutex<HashSet<String>>>,
}

impl RefreshToolsetsCoordinator {
    async fn spawn<F, Fut>(&self, provider_name: &str, logger: DaemonLogger, operation: F)
    where
        F: FnOnce() -> Fut + Send + 'static,
        Fut: Future<Output = Result<(), Box<dyn Error>>> + Send + 'static,
    {
        let provider_name = provider_name.to_string();
        {
            let mut slots = self.slots.lock().await;
            if !slots.insert(provider_name.clone()) {
                logger.info(
                    "daemon.refresh_skipped",
                    format!("provider={provider_name} reason=already_running"),
                );
                return;
            }
        }

        logger.info(
            "daemon.refresh_started",
            format!("provider={provider_name}"),
        );
        let slots = Arc::clone(&self.slots);
        tokio::spawn(async move {
            let failure = operation().await.err().map(|error| error.to_string());
            match failure {
                None => logger.info(
                    "daemon.refresh_finished",
                    format!("provider={provider_name} status=ok"),
                ),
                Some(error) => logger.error(
                    "daemon.refresh_failed",
                    format!("provider={provider_name} error={error}"),
                ),
            }
            slots.lock().await.remove(&provider_name);
        });
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
    use std::time::{SystemTime, UNIX_EPOCH};
    use tokio::time::sleep;

    #[tokio::test]
    async fn refresh_toolsets_coordinator_deduplicates_concurrent_refreshes() {
        let coordinator = RefreshToolsetsCoordinator::default();
        let runs = Arc::new(AtomicUsize::new(0));
        let release = Arc::new(tokio::sync::Notify::new());
        let logger = test_logger("refresh-dedup");

        let first_runs = Arc::clone(&runs);
        let first_release = Arc::clone(&release);
        coordinator
            .spawn("codex", logger.clone(), move || async move {
                first_runs.fetch_add(1, Ordering::SeqCst);
                first_release.notified().await;
                Ok(())
            })
            .await;

        let second_runs = Arc::clone(&runs);
        coordinator
            .spawn("codex", logger, move || async move {
                second_runs.fetch_add(1, Ordering::SeqCst);
                Ok(())
            })
            .await;

        sleep(Duration::from_millis(50)).await;
        assert_eq!(runs.load(Ordering::SeqCst), 1);
        release.notify_waiters();
        sleep(Duration::from_millis(50)).await;
    }

    #[tokio::test]
    async fn refresh_toolsets_coordinator_allows_retry_after_failure() {
        let coordinator = RefreshToolsetsCoordinator::default();
        let runs = Arc::new(AtomicUsize::new(0));
        let logger = test_logger("refresh-retry");

        let first_runs = Arc::clone(&runs);
        coordinator
            .spawn("codex", logger.clone(), move || async move {
                first_runs.fetch_add(1, Ordering::SeqCst);
                Err("refresh failed".into())
            })
            .await;
        sleep(Duration::from_millis(50)).await;

        let second_runs = Arc::clone(&runs);
        coordinator
            .spawn("codex", logger, move || async move {
                second_runs.fetch_add(1, Ordering::SeqCst);
                Ok(())
            })
            .await;
        sleep(Duration::from_millis(50)).await;

        assert_eq!(runs.load(Ordering::SeqCst), 2);
    }

    fn test_logger(label: &str) -> DaemonLogger {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "msp-daemon-runtime-{label}-{}-{nonce}.log",
            std::process::id()
        ));
        DaemonLogger::open(path).unwrap()
    }
}
