use std::error::Error;
use std::fs;
use std::path::{Path, PathBuf};

use rmcp::model::Tool;
mod summarizer;

use crate::config::{configured_server, load_config_table, load_model_provider_config};
use crate::console::{
    ExternalOutputCapture, operation_error, print_external_command_failure_with_captured_stderr,
};
use crate::downstream_client::connect_stdio_client;
use crate::fs_util::{FileLockGuard, acquire_sibling_lock, write_file_atomically};
use crate::paths::{
    cache_file_path, format_path_for_display, sanitize_name, sibling_lock_path, unix_epoch_ms,
};
use crate::reload::summarizer::summarize_tools;
use crate::remote::connect_remote_client;
use crate::types::{
    CachedTools, ConfiguredServer, ConfiguredTransport, ModelProviderConfig, ToolSnapshot,
    tool_snapshot,
};

pub struct ReloadResult {
    pub cache_path: PathBuf,
    pub updated: bool,
}

pub async fn reload_server(config_path: &Path, name: &str) -> Result<ReloadResult, Box<dyn Error>> {
    let provider = load_model_provider_config("codex").map_err(|error| {
        operation_error(
            "reload.load_provider",
            "failed to resolve the default summary provider",
            error,
        )
    })?;
    reload_server_with_resolved_provider(config_path, name, &provider).await
}

pub async fn reload_server_with_provider(
    config_path: &Path,
    name: &str,
    provider: &ModelProviderConfig,
) -> Result<ReloadResult, Box<dyn Error>> {
    reload_server_with_resolved_provider(config_path, name, provider).await
}

async fn reload_server_with_resolved_provider(
    config_path: &Path,
    name: &str,
    provider: &ModelProviderConfig,
) -> Result<ReloadResult, Box<dyn Error>> {
    let normalized_name = sanitize_name(name);
    let lock_cache_path = if normalized_name.is_empty() {
        None
    } else {
        Some(cache_file_path(&normalized_name).map_err(|error| {
            operation_error(
                "reload.cache_path",
                format!("failed to compute cache path for `{normalized_name}`"),
                error,
            )
        })?)
    };
    let _reload_lock = match &lock_cache_path {
        Some(cache_path) => Some(acquire_reload_lock(cache_path).map_err(|error| {
            operation_error(
                "reload.lock",
                format!("failed to acquire refresh lock for `{normalized_name}`"),
                error,
            )
        })?),
        None => None,
    };
    let config = load_config_table(config_path).map_err(|error| {
        operation_error(
            "reload.load_config",
            format!(
                "failed to load config from {}",
                format_path_for_display(config_path)
            ),
            error,
        )
    })?;
    let (resolved_name, server) = configured_server(&config, name).map_err(|error| {
        operation_error(
            "reload.resolve_server",
            format!("failed to resolve configured server `{name}`"),
            error,
        )
    })?;
    let cache_path = match lock_cache_path {
        Some(cache_path) => cache_path,
        None => cache_file_path(&resolved_name).map_err(|error| {
            operation_error(
                "reload.cache_path",
                format!("failed to compute cache path for `{resolved_name}`"),
                error,
            )
        })?,
    };
    let tools = fetch_tools(&resolved_name, &server)
        .await
        .map_err(|error| {
            operation_error(
                "reload.fetch_tools",
                format!("failed to fetch tools from MCP server `{resolved_name}`"),
                error,
            )
        })?;
    let tool_snapshots = tools.iter().map(tool_snapshot).collect::<Vec<_>>();
    if cached_tools_match(&cache_path, &tool_snapshots).map_err(|error| {
        operation_error(
            "reload.compare_cached_tools",
            format!("failed to compare fetched tools with cached tools for `{resolved_name}`"),
            error,
        )
    })? {
        return Ok(ReloadResult {
            cache_path,
            updated: false,
        });
    }

    let summary = summarize_tools(provider, &resolved_name, &tools)
        .await
        .map_err(|error| {
            operation_error(
                "reload.summarize_tools",
                format!("failed to summarize tools for MCP server `{resolved_name}`"),
                error,
            )
        })?;
    let payload = CachedTools {
        server: resolved_name,
        summary,
        fetched_at_epoch_ms: unix_epoch_ms().map_err(|error| {
            operation_error(
                "reload.timestamp",
                "failed to compute cache timestamp",
                error,
            )
        })?,
        tools: tool_snapshots,
    };

    write_cache(&cache_path, &payload).map_err(|error| {
        operation_error(
            "reload.write_cache",
            format!(
                "failed to write cached tools into {}",
                format_path_for_display(&cache_path)
            ),
            error,
        )
    })?;
    Ok(ReloadResult {
        cache_path,
        updated: true,
    })
}

async fn fetch_tools(
    server_name: &str,
    server: &ConfiguredServer,
) -> Result<Vec<Tool>, Box<dyn Error>> {
    match &server.transport {
        ConfiguredTransport::Stdio { command, args } => {
            let client = connect_stdio_client(
                "reload.fetch_tools",
                "reload.fetch_tools.spawn",
                "reload.fetch_tools.connect",
                server_name.to_string(),
                command,
                args,
                server.resolved_env(),
            )
            .await?;
            let stderr_capture = client.stderr.start_capture().await;
            let tools = match client.service.list_all_tools().await {
                Ok(tools) => tools,
                Err(error) => {
                    print_external_command_failure_async(
                        "reload.fetch_tools",
                        &client.label,
                        &client.command_line,
                        "list-tools-failed",
                        stderr_capture,
                    )
                    .await;
                    return Err(operation_error(
                        "reload.fetch_tools.list_tools",
                        format!(
                            "failed to list tools from external command `{}`",
                            client.command_line
                        ),
                        Box::new(error),
                    ));
                }
            };
            if let Err(error) = client.service.cancel().await {
                print_external_command_failure_async(
                    "reload.fetch_tools",
                    &client.label,
                    &client.command_line,
                    "shutdown-failed",
                    stderr_capture,
                )
                .await;
                return Err(operation_error(
                    "reload.fetch_tools.shutdown",
                    format!(
                        "failed to shut down MCP client for `{}`",
                        client.command_line
                    ),
                    Box::new(error),
                ));
            }
            let _ = stderr_capture.finish().await;
            Ok(tools)
        }
        ConfiguredTransport::Remote { url, .. } => {
            let client = connect_remote_client(server_name, server)
                .await
                .map_err(|error| {
                    operation_error(
                        "reload.fetch_tools.connect_remote",
                        format!(
                            "failed to initialize an MCP client against remote endpoint `{url}`"
                        ),
                        error,
                    )
                })?;
            let tools = client.list_all_tools().await.map_err(|error| {
                operation_error(
                    "reload.fetch_tools.list_remote_tools",
                    format!("failed to list tools from remote endpoint `{url}`"),
                    Box::new(error),
                )
            })?;
            client.cancel().await.map_err(|error| {
                operation_error(
                    "reload.fetch_tools.shutdown_remote",
                    format!("failed to shut down MCP client for remote endpoint `{url}`"),
                    Box::new(error),
                )
            })?;
            Ok(tools)
        }
    }
}

async fn print_external_command_failure_async(
    stage: &str,
    label: &str,
    command_line: &str,
    status: &str,
    stderr_capture: ExternalOutputCapture,
) {
    let content = stderr_capture.finish().await;
    print_external_command_failure_with_captured_stderr(
        stage,
        label,
        command_line,
        status,
        &content,
    )
    .await;
}

fn cached_tools_match(path: &Path, tools: &[ToolSnapshot]) -> Result<bool, Box<dyn Error>> {
    if !path.exists() {
        return Ok(false);
    }

    let cached = read_cached_tools(path)?;
    Ok(serialize_tool_snapshots(&cached.tools)? == serialize_tool_snapshots(tools)?)
}

fn read_cached_tools(path: &Path) -> Result<CachedTools, Box<dyn Error>> {
    let contents = fs::read_to_string(path).map_err(|error| {
        operation_error(
            "reload.read_cache.read_file",
            format!(
                "failed to read cache file {}",
                format_path_for_display(path)
            ),
            Box::new(error),
        )
    })?;

    serde_json::from_str(&contents).map_err(|error| {
        operation_error(
            "reload.read_cache.deserialize",
            format!(
                "failed to deserialize cache file {}",
                format_path_for_display(path)
            ),
            Box::new(error),
        )
    })
}

fn serialize_tool_snapshots(tools: &[ToolSnapshot]) -> Result<String, Box<dyn Error>> {
    serde_json::to_string_pretty(tools).map_err(|error| {
        operation_error(
            "reload.compare_cached_tools.serialize",
            "failed to serialize tool snapshots for comparison",
            Box::new(error),
        )
    })
}

fn acquire_reload_lock(cache_path: &Path) -> Result<FileLockGuard, Box<dyn Error>> {
    acquire_sibling_lock(cache_path).map_err(|error| {
        let lock_path = sibling_lock_path(cache_path);
        operation_error(
            "reload.lock",
            format!("failed to lock {}", format_path_for_display(&lock_path)),
            Box::new(error),
        )
    })
}

fn write_cache(path: &Path, payload: &CachedTools) -> Result<(), Box<dyn Error>> {
    let contents = serde_json::to_string_pretty(payload).map_err(|error| {
        operation_error(
            "reload.write_cache.serialize",
            "failed to serialize cached tool metadata to JSON",
            Box::new(error),
        )
    })?;
    write_file_atomically(path, contents.as_bytes()).map_err(|error| {
        operation_error(
            "reload.write_cache.write_file",
            format!(
                "failed to write cache file {}",
                format_path_for_display(path)
            ),
            Box::new(error),
        )
    })?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::env;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, mpsc};
    use std::thread;
    use std::time::Duration;

    use crate::reload::summarizer::{
        claude_workdir_path, codex_output_path, codex_workdir_path, non_empty_summary,
        opencode_workdir_path,
    };

    #[test]
    fn codex_output_path_is_created_in_temp_dir() {
        let path = codex_output_path().unwrap();

        assert!(path.starts_with(env::temp_dir()));
        assert!(
            path.file_name()
                .and_then(|name| name.to_str())
                .unwrap()
                .starts_with("mcp-smart-proxy-codex-summary-")
        );
    }

    #[test]
    fn codex_workdir_path_is_created_in_temp_dir() {
        let path = codex_workdir_path().unwrap();

        assert!(path.starts_with(env::temp_dir()));
        assert!(
            path.file_name()
                .and_then(|name| name.to_str())
                .unwrap()
                .starts_with("mcp-smart-proxy-codex-workdir-")
        );
    }

    #[test]
    fn opencode_workdir_path_is_created_in_temp_dir() {
        let path = opencode_workdir_path().unwrap();

        assert!(path.starts_with(env::temp_dir()));
        assert!(
            path.file_name()
                .and_then(|name| name.to_str())
                .unwrap()
                .starts_with("mcp-smart-proxy-opencode-workdir-")
        );
    }

    #[test]
    fn claude_workdir_path_is_created_in_temp_dir() {
        let path = claude_workdir_path().unwrap();

        assert!(path.starts_with(env::temp_dir()));
        assert!(
            path.file_name()
                .and_then(|name| name.to_str())
                .unwrap()
                .starts_with("mcp-smart-proxy-claude-workdir-")
        );
    }

    #[test]
    fn rejects_empty_summary_text() {
        let error = non_empty_summary(Some("   "), "empty").unwrap_err();

        assert_eq!(
            error.to_string(),
            "reload.summarize_tools.empty_summary: empty"
        );
    }

    #[test]
    fn reload_lock_path_uses_sibling_lock_file() {
        let cache_path = Path::new("/tmp/github.json");

        assert_eq!(
            sibling_lock_path(cache_path),
            PathBuf::from("/tmp/github.json.lock")
        );
    }

    #[test]
    fn reload_lock_serializes_concurrent_refreshes_for_the_same_cache() {
        let cache_path = env::temp_dir().join(format!(
            "mcp-smart-proxy-reload-lock-{}.json",
            unix_epoch_ms().unwrap()
        ));
        let lock_path = sibling_lock_path(&cache_path);
        let (locked_tx, locked_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let acquired = Arc::new(AtomicBool::new(false));

        let first_cache_path = cache_path.clone();
        let first = thread::spawn(move || {
            let _guard = acquire_reload_lock(&first_cache_path).unwrap();
            locked_tx.send(()).unwrap();
            release_rx.recv().unwrap();
        });

        locked_rx.recv().unwrap();

        let second_cache_path = cache_path.clone();
        let acquired_clone = Arc::clone(&acquired);
        let second = thread::spawn(move || {
            let _guard = acquire_reload_lock(&second_cache_path).unwrap();
            acquired_clone.store(true, Ordering::SeqCst);
        });

        thread::sleep(Duration::from_millis(150));
        assert!(
            !acquired.load(Ordering::SeqCst),
            "second refresh acquired the cache lock before the first one released it"
        );

        release_tx.send(()).unwrap();
        first.join().unwrap();
        second.join().unwrap();

        assert!(acquired.load(Ordering::SeqCst));

        if lock_path.exists() {
            fs::remove_file(lock_path).unwrap();
        }
    }

    #[test]
    fn matches_cached_tools_by_serialized_tool_string() {
        let cache_path = env::temp_dir().join(format!(
            "mcp-smart-proxy-reload-cache-{}.json",
            unix_epoch_ms().unwrap()
        ));
        let tools = vec![ToolSnapshot {
            name: "search".to_string(),
            title: Some("Search".to_string()),
            description: Some("Find items".to_string()),
            input_schema: json!({"type":"object"}),
            output_schema: None,
            annotations: None,
            execution: None,
            icons: None,
            meta: None,
        }];
        let payload = CachedTools {
            server: "demo".to_string(),
            summary: "old summary".to_string(),
            fetched_at_epoch_ms: 1,
            tools: tools.clone(),
        };

        write_cache(&cache_path, &payload).unwrap();

        assert!(cached_tools_match(&cache_path, &tools).unwrap());

        fs::remove_file(cache_path).unwrap();
    }

    #[test]
    fn detects_when_cached_tools_differ() {
        let cache_path = env::temp_dir().join(format!(
            "mcp-smart-proxy-reload-cache-diff-{}.json",
            unix_epoch_ms().unwrap()
        ));
        let payload = CachedTools {
            server: "demo".to_string(),
            summary: "old summary".to_string(),
            fetched_at_epoch_ms: 1,
            tools: vec![ToolSnapshot {
                name: "search".to_string(),
                title: Some("Search".to_string()),
                description: Some("Find items".to_string()),
                input_schema: json!({"type":"object"}),
                output_schema: None,
                annotations: None,
                execution: None,
                icons: None,
                meta: None,
            }],
        };
        let updated_tools = vec![ToolSnapshot {
            name: "lookup".to_string(),
            title: Some("Lookup".to_string()),
            description: Some("Lookup items".to_string()),
            input_schema: json!({"type":"object"}),
            output_schema: None,
            annotations: None,
            execution: None,
            icons: None,
            meta: None,
        }];

        write_cache(&cache_path, &payload).unwrap();

        assert!(!cached_tools_match(&cache_path, &updated_tools).unwrap());

        fs::remove_file(cache_path).unwrap();
    }
}
