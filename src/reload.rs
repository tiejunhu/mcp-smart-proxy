use std::env;
use std::error::Error;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Stdio;

use aisdk::{
    core::{DynamicModel, LanguageModelRequest},
    providers::OpenAI,
};
use rmcp::{
    ServiceExt,
    model::Tool,
    transport::{ConfigureCommandExt, TokioChildProcess},
};
use tokio::io::AsyncWriteExt;

use crate::config::{configured_server, load_config_table, load_default_model_provider_config};
use crate::console::{
    describe_command, message_error, operation_error, print_external_command_end,
    print_external_command_start, print_external_output_block, spawn_stderr_logger,
};
use crate::paths::{cache_file_path, unix_epoch_ms};
use crate::types::{
    CachedTools, CodexRuntimeConfig, ConfiguredServer, ModelProviderConfig, OpenAiRuntimeConfig,
    tool_snapshot,
};

pub async fn reload_server(config_path: &Path, name: &str) -> Result<PathBuf, Box<dyn Error>> {
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
    let summary = summarize_tools(&provider, &resolved_name, &tools)
        .await
        .map_err(|error| {
            operation_error(
                "reload.summarize_tools",
                format!("failed to summarize tools for MCP server `{resolved_name}`"),
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
        tools: tools.iter().map(tool_snapshot).collect(),
    };

    write_cache(&cache_path, &payload).map_err(|error| {
        operation_error(
            "reload.write_cache",
            format!("failed to write cached tools into {}", cache_path.display()),
            error,
        )
    })?;
    Ok(cache_path)
}

async fn fetch_tools(
    server_name: &str,
    server: &ConfiguredServer,
) -> Result<Vec<Tool>, Box<dyn Error>> {
    let command_line = describe_command(&server.command, &server.args);
    print_external_command_start("reload.fetch_tools", server_name, &command_line);
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
        spawn_stderr_logger(
            "reload.fetch_tools".to_string(),
            server_name.to_string(),
            command_line.clone(),
            stderr,
        );
    }

    let client = ().serve(transport).await.map_err(|error| {
        operation_error(
            "reload.fetch_tools.connect",
            format!("failed to initialize an MCP client against external command `{command_line}`"),
            Box::new(error),
        )
    })?;
    let tools = client.list_all_tools().await.map_err(|error| {
        operation_error(
            "reload.fetch_tools.list_tools",
            format!("failed to list tools from external command `{command_line}`"),
            Box::new(error),
        )
    })?;
    client.cancel().await.map_err(|error| {
        operation_error(
            "reload.fetch_tools.shutdown",
            format!("failed to shut down MCP client for `{command_line}`"),
            Box::new(error),
        )
    })?;
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
        ModelProviderConfig::OpenAi(openai) => summarize_tools_with_openai(openai, &prompt).await,
        ModelProviderConfig::Codex(codex) => summarize_tools_with_codex(codex, &prompt).await,
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

async fn summarize_tools_with_openai(
    openai: &OpenAiRuntimeConfig,
    prompt: &str,
) -> Result<String, Box<dyn Error>> {
    let mut model_builder = OpenAI::<DynamicModel>::builder()
        .model_name(openai.model.clone())
        .api_key(openai.key.clone());
    if let Some(baseurl) = &openai.baseurl {
        model_builder = model_builder.base_url(baseurl.clone());
    }
    let model = model_builder.build().map_err(|error| {
        operation_error(
            "reload.summarize_tools.openai.build_model",
            "failed to build the OpenAI client",
            Box::new(error),
        )
    })?;

    let mut request = LanguageModelRequest::builder()
        .model(model)
        .prompt(prompt.to_string())
        .build();
    let response = request.generate_text().await.map_err(|error| {
        operation_error(
            "reload.summarize_tools.openai.generate",
            "OpenAI request failed while generating the tool summary",
            Box::new(error),
        )
    })?;
    non_empty_summary(
        Some(response.text().as_deref().ok_or_else(|| {
            message_error(
                "reload.summarize_tools.openai.response",
                "OpenAI returned no text field in the summary response",
            )
        })?),
        "OpenAI returned an empty summary",
    )
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
    print_external_command_start("reload.summarize_tools.codex", "codex", &command_line);

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
        operation_error(
            "reload.summarize_tools.codex.wait",
            format!("failed while waiting for external command `{command_line}`"),
            Box::new(error),
        )
    })?;
    print_external_output_block(
        "reload.summarize_tools.codex",
        "codex",
        &command_line,
        "stderr",
        &String::from_utf8_lossy(&output.stderr),
    );
    print_external_command_end(
        "reload.summarize_tools.codex",
        "codex",
        &command_line,
        output.status,
    );
    if !output.status.success() {
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
    fn rejects_empty_summary_text() {
        let error = non_empty_summary(Some("   "), "empty").unwrap_err();

        assert_eq!(
            error.to_string(),
            "reload.summarize_tools.empty_summary: empty"
        );
    }
}
