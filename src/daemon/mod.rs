mod runtime;

use std::error::Error;
use std::fs;
use std::fs::File;
use std::io;
use std::io::Write;
use std::os::unix::fs::FileTypeExt;
use std::os::unix::net::UnixStream as StdUnixStream;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader as AsyncBufReader};
use tokio::net::UnixStream;
use tokio::time::sleep;

use crate::paths::{daemon_socket_path, validate_unix_socket_path};
use crate::types::{CachedToolsetRecord, DaemonRequest, DaemonResponse, DaemonStatus};

const DAEMON_READY_RETRIES: usize = 100;
const DAEMON_RETRY_DELAY: Duration = Duration::from_millis(50);

pub async fn run_daemon(
    config_path: &Path,
    socket_override: Option<&Path>,
) -> Result<(), Box<dyn Error>> {
    let socket_path = resolve_socket_path(config_path, socket_override)?;
    let pid_path = daemon_pid_path(&socket_path)?;
    let _pid_guard = claim_daemon_pid(&socket_path, &pid_path)?;

    prepare_socket_path(&socket_path)?;
    let listener = tokio::net::UnixListener::bind(&socket_path)?;
    let _socket_guard = CleanupGuard::new(socket_path.clone());

    runtime::serve_daemon(listener, config_path.to_path_buf(), socket_path).await
}

pub async fn ensure_daemon_running(
    config_path: &Path,
    socket_override: Option<&Path>,
) -> Result<DaemonStatus, Box<dyn Error>> {
    match probe_status(config_path, socket_override).await? {
        Some(status) if status.version == env!("CARGO_PKG_VERSION") => Ok(status),
        Some(_) | None => restart_daemon(config_path, socket_override).await,
    }
}

pub async fn request_status(
    config_path: &Path,
    socket_override: Option<&Path>,
) -> Result<DaemonStatus, Box<dyn Error>> {
    probe_status(config_path, socket_override)
        .await?
        .ok_or_else(|| daemon_not_running_error(config_path, socket_override).into())
}

pub async fn request_exit(
    config_path: &Path,
    socket_override: Option<&Path>,
) -> Result<(), Box<dyn Error>> {
    let socket_path = resolve_socket_path(config_path, socket_override)?;
    let response = send_request(
        &socket_path,
        &DaemonRequest::Exit,
        Some(config_path),
        socket_override,
    )
    .await?;
    match response {
        Some(DaemonResponse::ExitAck) => wait_until_stopped(&socket_path).await,
        Some(DaemonResponse::Error { message }) => Err(message.into()),
        Some(other) => Err(format!("unexpected daemon exit response: {other:?}").into()),
        None => Ok(()),
    }
}

pub async fn stop_daemon(
    config_path: &Path,
    socket_override: Option<&Path>,
) -> Result<bool, Box<dyn Error>> {
    let socket_path = resolve_socket_path(config_path, socket_override)?;

    match probe_status(config_path, socket_override).await? {
        Some(_) => {
            request_exit(config_path, socket_override).await?;
            Ok(true)
        }
        None => {
            cleanup_runtime_state(&socket_path)?;
            Ok(false)
        }
    }
}

pub async fn restart_daemon(
    config_path: &Path,
    socket_override: Option<&Path>,
) -> Result<DaemonStatus, Box<dyn Error>> {
    let _ = stop_daemon(config_path, socket_override).await?;
    spawn_detached_daemon(config_path, socket_override)?;
    request_status(config_path, socket_override).await
}

pub async fn load_toolsets(
    config_path: &Path,
    socket_override: Option<&Path>,
    provider_name: &str,
) -> Result<Vec<CachedToolsetRecord>, Box<dyn Error>> {
    let socket_path = resolve_socket_path(config_path, socket_override)?;
    let response = send_request(
        &socket_path,
        &DaemonRequest::LoadToolsets {
            provider: provider_name.to_string(),
        },
        Some(config_path),
        socket_override,
    )
    .await?
    .ok_or_else(|| daemon_not_running_error(config_path, socket_override))?;

    match response {
        DaemonResponse::Toolsets { toolsets } => Ok(toolsets),
        DaemonResponse::Error { message } => Err(message.into()),
        other => Err(format!("unexpected daemon load_toolsets response: {other:?}").into()),
    }
}

pub async fn call_tool(
    config_path: &Path,
    socket_override: Option<&Path>,
    toolset_name: &str,
    tool_name: &str,
    arguments: Option<serde_json::Map<String, serde_json::Value>>,
) -> Result<rmcp::model::CallToolResult, Box<dyn Error>> {
    let socket_path = resolve_socket_path(config_path, socket_override)?;
    let response = send_request(
        &socket_path,
        &DaemonRequest::CallTool {
            toolset_name: toolset_name.to_string(),
            tool_name: tool_name.to_string(),
            arguments,
        },
        Some(config_path),
        socket_override,
    )
    .await?
    .ok_or_else(|| daemon_not_running_error(config_path, socket_override))?;

    match response {
        DaemonResponse::ToolResult { result } => Ok(result),
        DaemonResponse::Error { message } => Err(message.into()),
        other => Err(format!("unexpected daemon call_tool response: {other:?}").into()),
    }
}

pub fn resolve_socket_path(
    config_path: &Path,
    socket_override: Option<&Path>,
) -> Result<PathBuf, Box<dyn Error>> {
    let path = match socket_override {
        Some(path) => Ok(path.to_path_buf()),
        None => daemon_socket_path(config_path),
    }?;
    validate_unix_socket_path(&path)?;
    Ok(path)
}

async fn probe_status(
    config_path: &Path,
    socket_override: Option<&Path>,
) -> Result<Option<DaemonStatus>, Box<dyn Error>> {
    let socket_path = resolve_socket_path(config_path, socket_override)?;
    let response = send_request(
        &socket_path,
        &DaemonRequest::Status,
        Some(config_path),
        socket_override,
    )
    .await?;
    match response {
        Some(DaemonResponse::Status { status }) => Ok(Some(status)),
        Some(DaemonResponse::Error { message }) => Err(message.into()),
        Some(other) => Err(format!("unexpected daemon status response: {other:?}").into()),
        None => Ok(None),
    }
}

async fn send_request(
    socket_path: &Path,
    request: &DaemonRequest,
    config_path: Option<&Path>,
    socket_override: Option<&Path>,
) -> Result<Option<DaemonResponse>, Box<dyn Error>> {
    let mut stream = match UnixStream::connect(socket_path).await {
        Ok(stream) => stream,
        Err(error) if is_stale_socket_error(error.kind()) => return Ok(None),
        Err(error) => {
            return Err(format!(
                "failed to connect to daemon socket {}: {error}",
                socket_path.display()
            )
            .into());
        }
    };

    let payload = serde_json::to_string(request)?;
    stream.write_all(payload.as_bytes()).await?;
    stream.write_all(b"\n").await?;
    stream.shutdown().await?;

    let mut reader = AsyncBufReader::new(stream);
    let mut response = String::new();
    let bytes = reader.read_line(&mut response).await?;
    if bytes == 0 {
        return Err(
            daemon_not_running_error_path(config_path, socket_override, socket_path).into(),
        );
    }

    Ok(Some(serde_json::from_str(response.trim())?))
}

fn spawn_detached_daemon(
    config_path: &Path,
    socket_override: Option<&Path>,
) -> Result<(), Box<dyn Error>> {
    let executable = std::env::current_exe()?;
    let socket_path = resolve_socket_path(config_path, socket_override)?;
    let startup_log_path = startup_log_path(&socket_path)?;
    let mut command = std::process::Command::new(executable);

    cleanup_runtime_state(&socket_path)?;
    command.arg("--config").arg(config_path);
    command.arg("daemon");
    if let Some(path) = socket_override {
        command.arg("--socket").arg(path);
    }
    command.arg("run");
    command.stdin(Stdio::null());
    command.stdout(Stdio::null());
    command.stderr(Stdio::from(File::create(&startup_log_path)?));

    let mut child = command.spawn()?;
    if let Err(error) = wait_until_ready(&socket_path, &mut child, &startup_log_path) {
        remove_file_if_present(&startup_log_path)?;
        return Err(error);
    }
    remove_file_if_present(&startup_log_path)?;
    Ok(())
}

fn wait_until_ready(
    socket_path: &Path,
    child: &mut std::process::Child,
    startup_log_path: &Path,
) -> Result<(), Box<dyn Error>> {
    for _ in 0..DAEMON_READY_RETRIES {
        match StdUnixStream::connect(socket_path) {
            Ok(_) => return Ok(()),
            Err(error) if is_stale_socket_error(error.kind()) => {}
            Err(error) => {
                return Err(format!(
                    "failed to connect to daemon socket {}: {error}",
                    socket_path.display()
                )
                .into());
            }
        }

        if let Some(status) = child.try_wait()? {
            let startup_log = read_file_if_present(startup_log_path)?;
            return Err(format!(
                "daemon exited before becoming ready: {status}\nstartup log:\n{startup_log}"
            )
            .into());
        }

        std::thread::sleep(DAEMON_RETRY_DELAY);
    }

    Err(format!(
        "timed out waiting for daemon socket {}",
        socket_path.display()
    )
    .into())
}

async fn wait_until_stopped(socket_path: &Path) -> Result<(), Box<dyn Error>> {
    for _ in 0..DAEMON_READY_RETRIES {
        if !socket_path.exists() {
            return Ok(());
        }
        sleep(DAEMON_RETRY_DELAY).await;
    }

    Err(format!(
        "timed out waiting for daemon socket {} to be removed",
        socket_path.display()
    )
    .into())
}

fn daemon_pid_path(socket_path: &Path) -> Result<PathBuf, Box<dyn Error>> {
    socket_sibling_path(socket_path, "pid")
}

fn startup_log_path(socket_path: &Path) -> Result<PathBuf, Box<dyn Error>> {
    socket_sibling_path(socket_path, "startup.log")
}

fn socket_sibling_path(socket_path: &Path, suffix: &str) -> Result<PathBuf, Box<dyn Error>> {
    let file_name = socket_path
        .file_name()
        .and_then(|value| value.to_str())
        .ok_or("failed to determine daemon socket file name")?;
    let parent = socket_path
        .parent()
        .ok_or("failed to determine daemon socket directory")?;
    Ok(parent.join(format!("{file_name}.{suffix}")))
}

fn cleanup_runtime_state(socket_path: &Path) -> Result<(), Box<dyn Error>> {
    remove_socket_if_present(socket_path)?;
    let pid_path = daemon_pid_path(socket_path)?;
    remove_file_if_present(&pid_path)?;
    Ok(())
}

fn prepare_socket_path(path: &Path) -> Result<(), Box<dyn Error>> {
    let parent = path
        .parent()
        .ok_or("failed to determine daemon socket directory")?;
    fs::create_dir_all(parent)?;

    match fs::symlink_metadata(path) {
        Ok(metadata) => {
            if !metadata.file_type().is_socket() {
                return Err(format!(
                    "refusing to overwrite non-socket file at {}",
                    path.display()
                )
                .into());
            }
            match StdUnixStream::connect(path) {
                Ok(_) => Err(format!("daemon socket already in use: {}", path.display()).into()),
                Err(error) if is_stale_socket_error(error.kind()) => {
                    fs::remove_file(path)?;
                    Ok(())
                }
                Err(error) => Err(format!(
                    "failed to inspect daemon socket {}: {error}",
                    path.display()
                )
                .into()),
            }
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(format!(
            "failed to inspect daemon socket {}: {error}",
            path.display()
        )
        .into()),
    }
}

fn claim_daemon_pid(socket_path: &Path, pid_path: &Path) -> Result<CleanupGuard, Box<dyn Error>> {
    let current_pid = std::process::id();
    if let Some(existing_pid) = read_pid(pid_path)? {
        if existing_pid != current_pid && process_is_alive(existing_pid)? {
            return Err(format!("daemon already running: pid {existing_pid}").into());
        }
        cleanup_runtime_state(socket_path)?;
    }

    if let Some(parent) = pid_path.parent() {
        fs::create_dir_all(parent)?;
    }
    let mut file = File::create(pid_path)?;
    writeln!(file, "{current_pid}")?;
    Ok(CleanupGuard::new(pid_path.to_path_buf()))
}

fn read_pid(path: &Path) -> Result<Option<u32>, Box<dyn Error>> {
    match fs::read_to_string(path) {
        Ok(contents) => Ok(contents.trim().parse::<u32>().ok()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => {
            Err(format!("failed to read daemon pid file {}: {error}", path.display()).into())
        }
    }
}

fn process_is_alive(pid: u32) -> Result<bool, Box<dyn Error>> {
    let result = unsafe { libc::kill(pid as libc::pid_t, 0) };
    if result == 0 {
        return Ok(true);
    }

    let error = io::Error::last_os_error();
    match error.raw_os_error() {
        Some(libc::EPERM) => Ok(true),
        Some(libc::ESRCH) => Ok(false),
        _ => Err(format!("failed to inspect daemon pid {pid}: {error}").into()),
    }
}

fn remove_socket_if_present(path: &Path) -> Result<(), Box<dyn Error>> {
    match fs::symlink_metadata(path) {
        Ok(metadata) => {
            if !metadata.file_type().is_socket() {
                return Err(format!(
                    "refusing to overwrite non-socket file at {}",
                    path.display()
                )
                .into());
            }
            fs::remove_file(path)?;
            Ok(())
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(format!(
            "failed to inspect daemon socket {}: {error}",
            path.display()
        )
        .into()),
    }
}

fn remove_file_if_present(path: &Path) -> Result<(), Box<dyn Error>> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(format!("failed to remove file {}: {error}", path.display()).into()),
    }
}

fn read_file_if_present(path: &Path) -> Result<String, Box<dyn Error>> {
    match fs::read_to_string(path) {
        Ok(contents) => Ok(contents),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(String::new()),
        Err(error) => Err(format!("failed to read file {}: {error}", path.display()).into()),
    }
}

fn is_stale_socket_error(kind: io::ErrorKind) -> bool {
    matches!(
        kind,
        io::ErrorKind::ConnectionRefused
            | io::ErrorKind::ConnectionAborted
            | io::ErrorKind::NotFound
    )
}

fn daemon_not_running_error(config_path: &Path, socket_override: Option<&Path>) -> String {
    match resolve_socket_path(config_path, socket_override) {
        Ok(socket_path) => {
            daemon_not_running_error_path(Some(config_path), socket_override, &socket_path)
        }
        Err(_) => "daemon is not running".to_string(),
    }
}

fn daemon_not_running_error_path(
    config_path: Option<&Path>,
    socket_override: Option<&Path>,
    socket_path: &Path,
) -> String {
    match (config_path, socket_override) {
        (Some(config_path), _) => format!(
            "daemon is not running for config {} at {}",
            config_path.display(),
            socket_path.display()
        ),
        (None, Some(_)) | (None, None) => {
            format!("daemon is not running at {}", socket_path.display())
        }
    }
}

struct CleanupGuard {
    path: PathBuf,
}

impl CleanupGuard {
    fn new(path: PathBuf) -> Self {
        Self { path }
    }
}

impl Drop for CleanupGuard {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn daemon_pid_path_is_resolved_next_to_socket() {
        let path = daemon_pid_path(Path::new("/tmp/msp.sock")).unwrap();
        assert_eq!(path, PathBuf::from("/tmp/msp.sock.pid"));
    }

    #[test]
    fn socket_override_is_returned_verbatim() {
        let socket_path = resolve_socket_path(
            Path::new("/tmp/config.toml"),
            Some(Path::new("/tmp/custom.sock")),
        )
        .unwrap();

        assert_eq!(socket_path, PathBuf::from("/tmp/custom.sock"));
    }

    #[test]
    fn socket_override_is_rejected_when_too_long() {
        let long_name = "a".repeat(120);
        let socket_path = PathBuf::from(format!("/tmp/{long_name}.sock"));

        let error =
            resolve_socket_path(Path::new("/tmp/config.toml"), Some(&socket_path)).unwrap_err();

        assert!(error.to_string().contains("unix socket path is too long"));
    }
}
