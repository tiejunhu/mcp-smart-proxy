use std::env;
use std::error::Error;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Stdio;

use rmcp::{
    ServiceExt,
    model::Tool,
    transport::{ConfigureCommandExt, TokioChildProcess},
};
use tokio::io::AsyncWriteExt;

use crate::config::{configured_server, load_config_table, load_default_model_provider_config};
use crate::console::{
    ExternalOutputCapture, ExternalOutputRouter, describe_command, message_error, operation_error,
    print_external_command_failure, print_external_output_block, print_external_output_if_present,
    spawn_stderr_collector,
};
use crate::paths::{cache_file_path, unix_epoch_ms};
use crate::types::{
    CachedTools, CodexRuntimeConfig, ConfiguredServer, ModelProviderConfig, OpencodeRuntimeConfig,
    ToolSnapshot, tool_snapshot,
};

pub struct ReloadResult {
    pub cache_path: PathBuf,
    pub updated: bool,
}

pub async fn reload_server(config_path: &Path, name: &str) -> Result<ReloadResult, Box<dyn Error>> {
    let config = load_config_table(config_path).map_err(|error| {
        operation_error(
            "reload.load_config",
            format!("failed to load config from {}", config_path.display()),
            error,
        )
    })?;
    let provider = load_default_model_provider_config(&config).map_err(|error| {
        operation_error(
            "reload.load_provider",
            "failed to load the default model provider configuration",
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
    let config = load_config_table(config_path).map_err(|error| {
        operation_error(
            "reload.load_config",
            format!("failed to load config from {}", config_path.display()),
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
    let tools = fetch_tools(&resolved_name, &server)
        .await
        .map_err(|error| {
            operation_error(
                "reload.fetch_tools",
                format!("failed to fetch tools from MCP server `{resolved_name}`"),
                error,
            )
        })?;
    let cache_path = cache_file_path(&resolved_name).map_err(|error| {
        operation_error(
            "reload.cache_path",
            format!("failed to compute cache path for `{resolved_name}`"),
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

    let summary = summarize_tools(&provider, &resolved_name, &tools)
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
            format!("failed to write cached tools into {}", cache_path.display()),
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
    let command_line = describe_command(&server.command, &server.args);
    let stderr_router = ExternalOutputRouter::new();
    let stderr_capture = stderr_router.start_capture().await;
    let (transport, stderr) = TokioChildProcess::builder(
        tokio::process::Command::new(&server.command).configure(|cmd| {
            cmd.args(&server.args);
        }),
    )
    .stderr(Stdio::piped())
    .spawn()
    .map_err(|error| {
        operation_error(
            "reload.fetch_tools.spawn",
            format!("failed to start external command `{command_line}`"),
            Box::new(error),
        )
    })?;

    if let Some(stderr) = stderr {
        spawn_stderr_collector(
            "reload.fetch_tools".to_string(),
            server_name.to_string(),
            command_line.clone(),
            stderr,
            stderr_router.clone(),
        );
    }

    let client = match ().serve(transport).await {
        Ok(client) => client,
        Err(error) => {
            print_external_command_failure_async(
                "reload.fetch_tools",
                server_name,
                &command_line,
                "connect-failed",
                stderr_capture,
            )
            .await;
            return Err(operation_error(
                "reload.fetch_tools.connect",
                format!(
                    "failed to initialize an MCP client against external command `{command_line}`"
                ),
                Box::new(error),
            ));
        }
    };
    let tools = match client.list_all_tools().await {
        Ok(tools) => tools,
        Err(error) => {
            print_external_command_failure_async(
                "reload.fetch_tools",
                server_name,
                &command_line,
                "list-tools-failed",
                stderr_capture,
            )
            .await;
            return Err(operation_error(
                "reload.fetch_tools.list_tools",
                format!("failed to list tools from external command `{command_line}`"),
                Box::new(error),
            ));
        }
    };
    if let Err(error) = client.cancel().await {
        print_external_command_failure_async(
            "reload.fetch_tools",
            server_name,
            &command_line,
            "shutdown-failed",
            stderr_capture,
        )
        .await;
        return Err(operation_error(
            "reload.fetch_tools.shutdown",
            format!("failed to shut down MCP client for `{command_line}`"),
            Box::new(error),
        ));
    }
    let _ = stderr_capture.finish().await;
    Ok(tools)
}

async fn summarize_tools(
    provider: &ModelProviderConfig,
    server_name: &str,
    tools: &[Tool],
) -> Result<String, Box<dyn Error>> {
    let prompt = build_summary_prompt(server_name, tools).map_err(|error| {
        operation_error(
            "reload.summarize_tools.build_prompt",
            format!("failed to build a summary prompt for `{server_name}`"),
            error,
        )
    })?;

    match provider {
        ModelProviderConfig::Codex(codex) => summarize_tools_with_codex(codex, &prompt).await,
        ModelProviderConfig::Opencode(opencode) => {
            summarize_tools_with_opencode(opencode, &prompt).await
        }
    }
}

fn build_summary_prompt(server_name: &str, tools: &[Tool]) -> Result<String, Box<dyn Error>> {
    let tools_json = serde_json::to_string_pretty(&tools).map_err(|error| {
        operation_error(
            "reload.summarize_tools.serialize_tools",
            format!("failed to serialize tool metadata for `{server_name}`"),
            Box::new(error),
        )
    })?;

    Ok(format!(
        "You are summarizing an MCP toolset for another AI.\n\
Server name: {server_name}\n\
Return exactly one concise English sentence.\n\
The sentence must explain when this toolset should be activated, based on the available tools.\n\
Do not mention implementation details like MCP, JSON, schema, or caching unless essential.\n\
Do not run shell commands or inspect the workspace. Use only the tool data provided below.\n\
If the tools cover multiple related workflows, summarize the common decision boundary.\n\n\
Tools:\n{tools_json}"
    ))
}

async fn summarize_tools_with_codex(
    codex: &CodexRuntimeConfig,
    prompt: &str,
) -> Result<String, Box<dyn Error>> {
    let workdir = codex_workdir_path().map_err(|error| {
        operation_error(
            "reload.summarize_tools.codex.workdir_path",
            "failed to compute a temporary workdir path for `codex exec`",
            error,
        )
    })?;
    fs::create_dir(&workdir).map_err(|error| {
        operation_error(
            "reload.summarize_tools.codex.create_workdir",
            format!("failed to create temporary workdir {}", workdir.display()),
            Box::new(error),
        )
    })?;
    let output_path = codex_output_path().map_err(|error| {
        operation_error(
            "reload.summarize_tools.codex.output_path",
            "failed to compute a temporary output path for `codex exec`",
            error,
        )
    })?;

    let command_args = vec![
        "exec".to_string(),
        "--model".to_string(),
        codex.model.clone(),
        "--skip-git-repo-check".to_string(),
        "--sandbox".to_string(),
        "read-only".to_string(),
        "--output-last-message".to_string(),
        output_path.display().to_string(),
        "-".to_string(),
    ];
    let command_line = describe_command("codex", &command_args);

    let mut child = tokio::process::Command::new("codex");
    child
        .current_dir(&workdir)
        .args(&command_args)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped());

    let mut child = child.spawn().map_err(|error| {
        operation_error(
            "reload.summarize_tools.codex.spawn",
            format!("failed to start external command `{command_line}`"),
            Box::new(error),
        )
    })?;
    let mut stdin = child.stdin.take().ok_or_else(|| {
        message_error(
            "reload.summarize_tools.codex.stdin",
            "failed to open stdin for `codex exec`",
        )
    })?;
    stdin.write_all(prompt.as_bytes()).await.map_err(|error| {
        operation_error(
            "reload.summarize_tools.codex.write_prompt",
            "failed to send the tool-summary prompt to `codex exec`",
            Box::new(error),
        )
    })?;
    drop(stdin);

    let output = child.wait_with_output().await.map_err(|error| {
        print_external_command_failure(
            "reload.summarize_tools.codex",
            "codex",
            &command_line,
            "wait-failed",
        );
        operation_error(
            "reload.summarize_tools.codex.wait",
            format!("failed while waiting for external command `{command_line}`"),
            Box::new(error),
        )
    })?;
    if !output.status.success() {
        print_external_command_failure(
            "reload.summarize_tools.codex",
            "codex",
            &command_line,
            &output.status.to_string(),
        );
        print_external_output_block(
            "reload.summarize_tools.codex",
            "codex",
            &command_line,
            "stderr",
            &String::from_utf8_lossy(&output.stderr),
        );
        let _ = fs::remove_file(&output_path);
        let _ = fs::remove_dir(&workdir);
        return Err(message_error(
            "reload.summarize_tools.codex.exit_status",
            format!(
                "`codex exec` exited unsuccessfully while summarizing tools; status={}, output_path={}",
                output.status,
                output_path.display()
            ),
        ));
    }

    let output = fs::read_to_string(&output_path).map_err(|error| {
        operation_error(
            "reload.summarize_tools.codex.read_output",
            format!(
                "failed to read summary output from {}",
                output_path.display()
            ),
            Box::new(error),
        )
    })?;
    let _ = fs::remove_file(&output_path);
    let _ = fs::remove_dir(&workdir);
    non_empty_summary(Some(output.as_str()), "Codex returned an empty summary")
}

async fn summarize_tools_with_opencode(
    opencode: &OpencodeRuntimeConfig,
    prompt: &str,
) -> Result<String, Box<dyn Error>> {
    let workdir = opencode_workdir_path().map_err(|error| {
        operation_error(
            "reload.summarize_tools.opencode.workdir_path",
            "failed to compute a temporary workdir path for `opencode run`",
            error,
        )
    })?;
    fs::create_dir(&workdir).map_err(|error| {
        operation_error(
            "reload.summarize_tools.opencode.create_workdir",
            format!("failed to create temporary workdir {}", workdir.display()),
            Box::new(error),
        )
    })?;

    let command_args = vec![
        "run".to_string(),
        "--model".to_string(),
        opencode.model.clone(),
        "--dir".to_string(),
        workdir.display().to_string(),
        "--format".to_string(),
        "default".to_string(),
        prompt.to_string(),
    ];
    let command_line = describe_command("opencode", &command_args);

    let output = tokio::process::Command::new("opencode")
        .current_dir(&workdir)
        .env("NO_COLOR", "1")
        .args(&command_args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .await
        .map_err(|error| {
            print_external_command_failure(
                "reload.summarize_tools.opencode",
                "opencode",
                &command_line,
                "wait-failed",
            );
            let _ = fs::remove_dir(&workdir);
            operation_error(
                "reload.summarize_tools.opencode.wait",
                format!("failed while waiting for external command `{command_line}`"),
                Box::new(error),
            )
        })?;

    if !output.status.success() {
        print_external_command_failure(
            "reload.summarize_tools.opencode",
            "opencode",
            &command_line,
            &output.status.to_string(),
        );
        print_external_output_block(
            "reload.summarize_tools.opencode",
            "opencode",
            &command_line,
            "stdout",
            &String::from_utf8_lossy(&output.stdout),
        );
        print_external_output_block(
            "reload.summarize_tools.opencode",
            "opencode",
            &command_line,
            "stderr",
            &String::from_utf8_lossy(&output.stderr),
        );
        let _ = fs::remove_dir(&workdir);
        return Err(message_error(
            "reload.summarize_tools.opencode.exit_status",
            format!(
                "`opencode run` exited unsuccessfully while summarizing tools; status={}",
                output.status
            ),
        ));
    }

    let summary = non_empty_summary(
        Some(String::from_utf8_lossy(&output.stdout).as_ref()),
        "OpenCode returned an empty summary",
    );
    let _ = fs::remove_dir(&workdir);
    summary
}

async fn print_external_command_failure_async(
    stage: &str,
    label: &str,
    command_line: &str,
    status: &str,
    stderr_capture: ExternalOutputCapture,
) {
    print_external_command_failure(stage, label, command_line, status);
    let content = stderr_capture.finish().await;
    print_external_output_if_present(stage, label, command_line, "stderr", &content).await;
}

fn codex_output_path() -> Result<PathBuf, Box<dyn Error>> {
    Ok(env::temp_dir().join(format!(
        "mcp-smart-proxy-codex-summary-{}-{}.txt",
        std::process::id(),
        unix_epoch_ms()?
    )))
}

fn codex_workdir_path() -> Result<PathBuf, Box<dyn Error>> {
    Ok(env::temp_dir().join(format!(
        "mcp-smart-proxy-codex-workdir-{}-{}",
        std::process::id(),
        unix_epoch_ms()?
    )))
}

fn opencode_workdir_path() -> Result<PathBuf, Box<dyn Error>> {
    Ok(env::temp_dir().join(format!(
        "mcp-smart-proxy-opencode-workdir-{}-{}",
        std::process::id(),
        unix_epoch_ms()?
    )))
}

fn non_empty_summary(value: Option<&str>, empty_message: &str) -> Result<String, Box<dyn Error>> {
    value
        .map(str::trim)
        .filter(|text| !text.is_empty())
        .map(ToOwned::to_owned)
        .ok_or_else(|| {
            message_error(
                "reload.summarize_tools.empty_summary",
                empty_message.to_string(),
            )
        })
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
            format!("failed to read cache file {}", path.display()),
            Box::new(error),
        )
    })?;

    serde_json::from_str(&contents).map_err(|error| {
        operation_error(
            "reload.read_cache.deserialize",
            format!("failed to deserialize cache file {}", path.display()),
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

fn write_cache(path: &Path, payload: &CachedTools) -> Result<(), Box<dyn Error>> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|error| {
            operation_error(
                "reload.write_cache.create_parent",
                format!("failed to create cache directory {}", parent.display()),
                Box::new(error),
            )
        })?;
    }

    let contents = serde_json::to_string_pretty(payload).map_err(|error| {
        operation_error(
            "reload.write_cache.serialize",
            "failed to serialize cached tool metadata to JSON",
            Box::new(error),
        )
    })?;
    fs::write(path, contents).map_err(|error| {
        operation_error(
            "reload.write_cache.write_file",
            format!("failed to write cache file {}", path.display()),
            Box::new(error),
        )
    })?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

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
    fn rejects_empty_summary_text() {
        let error = non_empty_summary(Some("   "), "empty").unwrap_err();

        assert_eq!(
            error.to_string(),
            "reload.summarize_tools.empty_summary: empty"
        );
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
