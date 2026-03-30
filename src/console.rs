use std::error::Error;
use std::fmt::{self, Display, Formatter};
use std::sync::Arc;

use tokio::{
    io::{AsyncBufReadExt, BufReader},
    process::ChildStderr,
    sync::Mutex,
};

const MAX_EXTERNAL_OUTPUT_LINES: usize = 1000;

#[derive(Debug, Default)]
struct ExternalCaptureState {
    next_id: u64,
    captures: Vec<(u64, Vec<String>)>,
}

#[derive(Clone, Debug, Default)]
pub struct ExternalOutputRouter {
    state: Arc<Mutex<ExternalCaptureState>>,
}

pub struct ExternalOutputCapture {
    router: ExternalOutputRouter,
    id: u64,
}

impl ExternalOutputRouter {
    pub fn new() -> Self {
        Self::default()
    }

    pub async fn start_capture(&self) -> ExternalOutputCapture {
        let mut state = self.state.lock().await;
        let id = state.next_id;
        state.next_id += 1;
        state.captures.push((id, Vec::new()));
        ExternalOutputCapture {
            router: self.clone(),
            id,
        }
    }

    pub async fn push(&self, line: String) {
        let mut state = self.state.lock().await;
        for (_, lines) in &mut state.captures {
            if lines.len() >= MAX_EXTERNAL_OUTPUT_LINES {
                lines.remove(0);
            }
            lines.push(line.clone());
        }
    }
}

impl ExternalOutputCapture {
    pub async fn finish(self) -> String {
        let mut state = self.router.state.lock().await;
        let Some(index) = state
            .captures
            .iter()
            .position(|(capture_id, _)| *capture_id == self.id)
        else {
            return String::new();
        };
        let (_, lines) = state.captures.swap_remove(index);
        lines.join("\n")
    }
}

#[derive(Debug)]
pub struct OperationError {
    stage: &'static str,
    summary: String,
    source: Option<Box<dyn Error>>,
}

impl OperationError {
    pub fn new(
        stage: &'static str,
        summary: impl Into<String>,
        source: Option<Box<dyn Error>>,
    ) -> Self {
        Self {
            stage,
            summary: summary.into(),
            source,
        }
    }

    pub fn stage(&self) -> &'static str {
        self.stage
    }

    pub fn summary(&self) -> &str {
        &self.summary
    }
}

impl Display for OperationError {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.stage, self.summary)
    }
}

impl Error for OperationError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        self.source.as_deref()
    }
}

pub fn operation_error(
    stage: &'static str,
    summary: impl Into<String>,
    source: Box<dyn Error>,
) -> Box<dyn Error> {
    Box::new(OperationError::new(stage, summary, Some(source)))
}

pub fn message_error(stage: &'static str, summary: impl Into<String>) -> Box<dyn Error> {
    Box::new(OperationError::new(stage, summary, None))
}

pub fn print_app_event(_stage: &str, message: impl AsRef<str>) {
    println!("{}", format_app_event_line(message.as_ref()));
}

pub fn print_app_warning(_stage: &str, message: impl AsRef<str>) {
    eprintln!("{}", format_app_warning_line(message.as_ref()));
}

pub fn print_app_error(error: &(dyn Error + 'static)) {
    let context = deepest_operation_error(error);
    let stage = context.map(OperationError::stage).unwrap_or("unknown");
    let summary = context
        .map(|item| item.summary().to_string())
        .unwrap_or_else(|| error.to_string());

    eprintln!(
        "{}",
        format_app_error_line(stage, &summary, &error_chain(error))
    );
}

pub fn describe_command(command: &str, args: &[String]) -> String {
    let mut rendered = Vec::with_capacity(args.len() + 1);
    rendered.push(render_token(command));
    rendered.extend(args.iter().map(|arg| render_token(arg)));
    rendered.join(" ")
}

pub fn print_external_command_failure(stage: &str, label: &str, command_line: &str, status: &str) {
    print_external_command_failure_with_output(stage, label, command_line, status, &[]);
}

pub fn print_external_command_failure_with_output(
    stage: &str,
    label: &str,
    command_line: &str,
    status: &str,
    streams: &[(&str, &str)],
) {
    eprintln!(
        "{}",
        format_external_command_failure_block(stage, label, command_line, status, streams)
    );
}

pub fn spawn_stderr_collector(
    stage: String,
    label: String,
    command_line: String,
    stderr: ChildStderr,
    output: ExternalOutputRouter,
) {
    tokio::spawn(async move {
        let mut lines = BufReader::new(stderr).lines();
        loop {
            match lines.next_line().await {
                Ok(Some(line)) => {
                    output.push(line).await;
                }
                Ok(None) => break,
                Err(error) => {
                    output
                        .push(format!(
                            "stderr read error (stage={stage}, target={label}, command={command_line}): {error}"
                        ))
                        .await;
                    break;
                }
            }
        }
    });
}

pub async fn print_external_command_failure_with_captured_stderr(
    stage: &str,
    label: &str,
    command_line: &str,
    status: &str,
    content: &str,
) {
    print_external_command_failure_with_output(
        stage,
        label,
        command_line,
        status,
        &[("stderr", content)],
    );
}

fn deepest_operation_error<'a>(mut error: &'a (dyn Error + 'static)) -> Option<&'a OperationError> {
    let mut deepest = None;
    loop {
        if let Some(operation_error) = error.downcast_ref::<OperationError>() {
            deepest = Some(operation_error);
        }
        match error.source() {
            Some(source) => error = source,
            None => return deepest,
        }
    }
}

fn error_chain(error: &(dyn Error + 'static)) -> Vec<String> {
    let mut items = Vec::new();
    let mut current = Some(error);
    while let Some(item) = current {
        items.push(item.to_string());
        current = item.source();
    }
    items
}

fn format_app_event_line(message: &str) -> String {
    render_inline_value(message)
}

fn format_app_warning_line(message: &str) -> String {
    format!("warning: {}", render_inline_value(message))
}

fn format_app_error_line(stage: &str, summary: &str, chain: &[String]) -> String {
    let mut lines = vec![
        "application error:".to_string(),
        format!("stage: {}", render_inline_value(stage)),
        format!("summary: {}", render_inline_value(summary)),
    ];

    if !chain.is_empty() {
        lines.push("causes:".to_string());
        lines.extend(
            chain
                .iter()
                .enumerate()
                .map(|(index, item)| format!("  {}. {}", index + 1, render_inline_value(item))),
        );
    }

    lines.join("\n")
}

fn format_external_command_failure_block(
    stage: &str,
    label: &str,
    command_line: &str,
    status: &str,
    streams: &[(&str, &str)],
) -> String {
    let mut lines = vec![
        "external command failure:".to_string(),
        format!("stage: {}", render_inline_value(stage)),
        format!("target: {}", render_inline_value(label)),
        format!("command: {}", render_inline_value(command_line)),
        format!("status: {}", render_inline_value(status)),
    ];

    for (stream, content) in streams {
        let trimmed = content.trim();
        if trimmed.is_empty() {
            continue;
        }
        lines.push(format!(
            "stream: external command {}",
            render_inline_value(stream)
        ));
        lines.push(format!("----- {} begin -----", render_inline_value(stream)));
        lines.extend(trimmed.lines().map(render_inline_value));
        lines.push(format!("----- {} end -----", render_inline_value(stream)));
    }

    lines.join("\n")
}

fn render_inline_value(value: &str) -> String {
    let mut rendered = String::with_capacity(value.len());
    for ch in value.chars() {
        match ch {
            '\n' => rendered.push_str("\\n"),
            '\r' => rendered.push_str("\\r"),
            '\t' => rendered.push_str("\\t"),
            _ => rendered.push(ch),
        }
    }
    rendered
}

fn render_token(value: &str) -> String {
    if value.chars().all(|ch| {
        ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-' | '/' | '.' | ':' | '=' | '@')
    }) {
        return value.to_string();
    }

    format!("{value:?}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_command_for_humans_and_logs() {
        let command_line = describe_command("npx", &["-y".into(), "server name".into()]);

        assert_eq!(command_line, "npx -y \"server name\"");
    }

    #[test]
    fn operation_error_exposes_stage_and_summary() {
        let error = OperationError::new("reload.fetch_tools", "failed to list tools", None);

        assert_eq!(error.stage(), "reload.fetch_tools");
        assert_eq!(error.summary(), "failed to list tools");
        assert_eq!(
            error.to_string(),
            "reload.fetch_tools: failed to list tools"
        );
    }

    #[test]
    fn formats_app_event_as_single_line() {
        assert_eq!(format_app_event_line("Updated config"), "Updated config");
    }

    #[test]
    fn formats_app_warning_as_single_line() {
        assert_eq!(
            format_app_warning_line("New version available"),
            "warning: New version available"
        );
    }

    #[test]
    fn escapes_control_characters_in_app_logs() {
        assert_eq!(
            format_app_event_line("line 1\nline 2\tvalue"),
            "line 1\\nline 2\\tvalue"
        );
    }

    #[test]
    fn formats_app_error_as_block_with_chain() {
        let line = format_app_error_line(
            "reload.fetch_tools.list_tools",
            "failed to list tools",
            &[
                "cli.reload: failed to reload MCP server `github`".into(),
                "reload.fetch_tools: failed to fetch tools from MCP server `github`".into(),
            ],
        );

        assert_eq!(
            line,
            "application error:\nstage: reload.fetch_tools.list_tools\nsummary: failed to list tools\ncauses:\n  1. cli.reload: failed to reload MCP server `github`\n  2. reload.fetch_tools: failed to fetch tools from MCP server `github`"
        );
    }

    #[test]
    fn formats_external_command_failure_as_labeled_block() {
        let block = format_external_command_failure_block(
            "reload.fetch_tools",
            "github",
            "npx -y @modelcontextprotocol/server-github",
            "list-tools-failed",
            &[],
        );

        assert_eq!(
            block,
            "external command failure:\nstage: reload.fetch_tools\ntarget: github\ncommand: npx -y @modelcontextprotocol/server-github\nstatus: list-tools-failed"
        );
    }

    #[test]
    fn formats_external_command_failure_with_stderr_block() {
        let block = format_external_command_failure_block(
            "reload.fetch_tools",
            "github",
            "npx -y @modelcontextprotocol/server-github",
            "list-tools-failed",
            &[("stderr", "GitHub token is missing")],
        );

        assert_eq!(
            block,
            "external command failure:\nstage: reload.fetch_tools\ntarget: github\ncommand: npx -y @modelcontextprotocol/server-github\nstatus: list-tools-failed\nstream: external command stderr\n----- stderr begin -----\nGitHub token is missing\n----- stderr end -----"
        );
    }

    #[test]
    fn skips_empty_external_output_streams_in_failure_block() {
        let block = format_external_command_failure_block(
            "reload.fetch_tools",
            "github",
            "npx -y @modelcontextprotocol/server-github",
            "list-tools-failed",
            &[("stderr", "   ")],
        );

        assert_eq!(
            block,
            "external command failure:\nstage: reload.fetch_tools\ntarget: github\ncommand: npx -y @modelcontextprotocol/server-github\nstatus: list-tools-failed"
        );
    }
}
