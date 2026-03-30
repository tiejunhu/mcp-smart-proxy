use std::env;
use std::error::Error;
use std::fs;
use std::path::PathBuf;
use std::process::Stdio;

use rmcp::model::Tool;
use tokio::io::AsyncWriteExt;

use crate::console::{
    describe_command, message_error, operation_error, print_external_command_failure,
    print_external_command_failure_with_output,
};
use crate::paths::{format_path_for_display, unix_epoch_ms};
use crate::types::{
    ClaudeRuntimeConfig, CodexRuntimeConfig, ModelProviderConfig, OpencodeRuntimeConfig,
};

pub(crate) async fn summarize_tools(
    provider: &ModelProviderConfig,
    server_name: &str,
    tools: &[Tool],
) -> Result<String, Box<dyn Error>> {
    let prompt = build_summary_prompt(server_name, tools)?;

    match provider {
        ModelProviderConfig::Codex(codex) => summarize_tools_with_codex(codex, &prompt).await,
        ModelProviderConfig::Opencode(opencode) => {
            summarize_tools_with_opencode(opencode, &prompt).await
        }
        ModelProviderConfig::Claude(claude) => summarize_tools_with_claude(claude, &prompt).await,
    }
}

fn build_summary_prompt(server_name: &str, tools: &[Tool]) -> Result<String, Box<dyn Error>> {
    let mut lines = vec![
        format!(
            "Write exactly one sentence describing when an AI should use the MCP server `{server_name}`."
        ),
        "Do not mention any provider names, brands, or model names.".to_string(),
        "Do not mention implementation details such as JSON schemas.".to_string(),
        "Be concise, direct, and specific about the server's practical purpose.".to_string(),
        "Return only the sentence.".to_string(),
        String::new(),
        format!("Server name: {server_name}"),
        "Tools:".to_string(),
    ];

    for tool in tools {
        let description = tool.description.as_deref().unwrap_or_default();
        lines.push(format!("- {}: {}", tool.name, description));
    }

    Ok(lines.join("\n"))
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
            format!(
                "failed to create temporary workdir {}",
                format_path_for_display(&workdir)
            ),
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
        print_external_command_failure_with_output(
            "reload.summarize_tools.codex",
            "codex",
            &command_line,
            &output.status.to_string(),
            &[("stderr", String::from_utf8_lossy(&output.stderr).as_ref())],
        );
        cleanup_summary_paths(Some(&output_path), Some(&workdir));
        return Err(message_error(
            "reload.summarize_tools.codex.exit_status",
            format!(
                "`codex exec` exited unsuccessfully while summarizing tools; status={}, output_path={}",
                output.status,
                format_path_for_display(&output_path)
            ),
        ));
    }

    let output = fs::read_to_string(&output_path).map_err(|error| {
        operation_error(
            "reload.summarize_tools.codex.read_output",
            format!(
                "failed to read summary output from {}",
                format_path_for_display(&output_path)
            ),
            Box::new(error),
        )
    })?;
    cleanup_summary_paths(Some(&output_path), Some(&workdir));
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
            format!(
                "failed to create temporary workdir {}",
                format_path_for_display(&workdir)
            ),
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
            cleanup_summary_paths(None, Some(&workdir));
            operation_error(
                "reload.summarize_tools.opencode.wait",
                format!("failed while waiting for external command `{command_line}`"),
                Box::new(error),
            )
        })?;

    if !output.status.success() {
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        print_external_command_failure_with_output(
            "reload.summarize_tools.opencode",
            "opencode",
            &command_line,
            &output.status.to_string(),
            &[("stdout", stdout.as_ref()), ("stderr", stderr.as_ref())],
        );
        cleanup_summary_paths(None, Some(&workdir));
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
    cleanup_summary_paths(None, Some(&workdir));
    summary
}

async fn summarize_tools_with_claude(
    claude: &ClaudeRuntimeConfig,
    prompt: &str,
) -> Result<String, Box<dyn Error>> {
    let workdir = claude_workdir_path().map_err(|error| {
        operation_error(
            "reload.summarize_tools.claude.workdir_path",
            "failed to compute a temporary workdir path for `claude -p`",
            error,
        )
    })?;
    fs::create_dir(&workdir).map_err(|error| {
        operation_error(
            "reload.summarize_tools.claude.create_workdir",
            format!(
                "failed to create temporary workdir {}",
                format_path_for_display(&workdir)
            ),
            Box::new(error),
        )
    })?;

    let command_args = vec![
        "-p".to_string(),
        "--model".to_string(),
        claude.model.clone(),
        "--output-format".to_string(),
        "text".to_string(),
        prompt.to_string(),
    ];
    let command_line = describe_command("claude", &command_args);

    let output = tokio::process::Command::new("claude")
        .current_dir(&workdir)
        .env("NO_COLOR", "1")
        .args(&command_args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .await
        .map_err(|error| {
            print_external_command_failure(
                "reload.summarize_tools.claude",
                "claude",
                &command_line,
                "wait-failed",
            );
            cleanup_summary_paths(None, Some(&workdir));
            operation_error(
                "reload.summarize_tools.claude.wait",
                format!("failed while waiting for external command `{command_line}`"),
                Box::new(error),
            )
        })?;

    if !output.status.success() {
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        print_external_command_failure_with_output(
            "reload.summarize_tools.claude",
            "claude",
            &command_line,
            &output.status.to_string(),
            &[("stdout", stdout.as_ref()), ("stderr", stderr.as_ref())],
        );
        cleanup_summary_paths(None, Some(&workdir));
        return Err(message_error(
            "reload.summarize_tools.claude.exit_status",
            format!(
                "`claude -p` exited unsuccessfully while summarizing tools; status={}",
                output.status
            ),
        ));
    }

    let summary = non_empty_summary(
        Some(String::from_utf8_lossy(&output.stdout).as_ref()),
        "Claude Code returned an empty summary",
    );
    cleanup_summary_paths(None, Some(&workdir));
    summary
}

fn cleanup_summary_paths(output_path: Option<&PathBuf>, workdir: Option<&PathBuf>) {
    if let Some(output_path) = output_path {
        let _ = fs::remove_file(output_path);
    }
    if let Some(workdir) = workdir {
        let _ = fs::remove_dir(workdir);
    }
}

pub(crate) fn codex_output_path() -> Result<PathBuf, Box<dyn Error>> {
    Ok(env::temp_dir().join(format!(
        "mcp-smart-proxy-codex-summary-{}-{}.txt",
        std::process::id(),
        unix_epoch_ms()?
    )))
}

pub(crate) fn codex_workdir_path() -> Result<PathBuf, Box<dyn Error>> {
    Ok(env::temp_dir().join(format!(
        "mcp-smart-proxy-codex-workdir-{}-{}",
        std::process::id(),
        unix_epoch_ms()?
    )))
}

pub(crate) fn opencode_workdir_path() -> Result<PathBuf, Box<dyn Error>> {
    Ok(env::temp_dir().join(format!(
        "mcp-smart-proxy-opencode-workdir-{}-{}",
        std::process::id(),
        unix_epoch_ms()?
    )))
}

pub(crate) fn claude_workdir_path() -> Result<PathBuf, Box<dyn Error>> {
    Ok(env::temp_dir().join(format!(
        "mcp-smart-proxy-claude-workdir-{}-{}",
        std::process::id(),
        unix_epoch_ms()?
    )))
}

pub(crate) fn non_empty_summary(
    value: Option<&str>,
    empty_message: &str,
) -> Result<String, Box<dyn Error>> {
    value
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
        .ok_or_else(|| {
            message_error(
                "reload.summarize_tools.empty_summary",
                empty_message.to_string(),
            )
        })
}
