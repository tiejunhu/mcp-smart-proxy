use std::error::Error;
use std::fmt::{self, Display, Formatter};
use std::process::ExitStatus;

use tokio::{
    io::{AsyncBufReadExt, BufReader},
    process::ChildStderr,
};

const APP_EVENT_BEGIN: &str = "=== MSP APP EVENT BEGIN ===";
const APP_EVENT_END: &str = "=== MSP APP EVENT END ===";
const APP_ERROR_BEGIN: &str = "=== MSP APP ERROR BEGIN ===";
const APP_ERROR_END: &str = "=== MSP APP ERROR END ===";
const EXTERNAL_OUTPUT_BEGIN: &str = "=== MSP EXTERNAL OUTPUT BEGIN ===";
const EXTERNAL_OUTPUT_END: &str = "=== MSP EXTERNAL OUTPUT END ===";

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

pub fn print_app_event(stage: &str, message: impl AsRef<str>) {
    println!("{APP_EVENT_BEGIN}");
    println!("kind: app");
    println!("level: info");
    println!("stage: {stage}");
    println!("message: {}", message.as_ref());
    println!("{APP_EVENT_END}");
}

pub fn print_app_error(error: &(dyn Error + 'static)) {
    let context = deepest_operation_error(error);
    let stage = context.map(OperationError::stage).unwrap_or("unknown");
    let summary = context
        .map(|item| item.summary().to_string())
        .unwrap_or_else(|| error.to_string());

    eprintln!("{APP_ERROR_BEGIN}");
    eprintln!("kind: app");
    eprintln!("level: error");
    eprintln!("stage: {stage}");
    eprintln!("summary: {summary}");
    eprintln!("error_chain:");
    for item in error_chain(error) {
        eprintln!("- {item}");
    }
    eprintln!("{APP_ERROR_END}");
}

pub fn describe_command(command: &str, args: &[String]) -> String {
    let mut rendered = Vec::with_capacity(args.len() + 1);
    rendered.push(render_token(command));
    rendered.extend(args.iter().map(|arg| render_token(arg)));
    rendered.join(" ")
}

pub fn print_external_command_start(stage: &str, label: &str, command_line: &str) {
    eprintln!("[MSP][EXTERNAL][{stage}][{label}][start] command={command_line}");
}

pub fn print_external_command_end(
    stage: &str,
    label: &str,
    command_line: &str,
    status: ExitStatus,
) {
    eprintln!("[MSP][EXTERNAL][{stage}][{label}][end] status={status} command={command_line}");
}

pub fn print_external_output_block(
    stage: &str,
    label: &str,
    command_line: &str,
    stream: &str,
    content: &str,
) {
    let trimmed = content.trim();
    if trimmed.is_empty() {
        return;
    }

    eprintln!("{EXTERNAL_OUTPUT_BEGIN}");
    eprintln!("kind: external-command");
    eprintln!("stage: {stage}");
    eprintln!("label: {label}");
    eprintln!("command: {command_line}");
    eprintln!("stream: {stream}");
    eprintln!("content:");
    for line in trimmed.lines() {
        eprintln!("{line}");
    }
    eprintln!("{EXTERNAL_OUTPUT_END}");
}

pub fn spawn_stderr_logger(
    stage: String,
    label: String,
    command_line: String,
    stderr: ChildStderr,
) {
    tokio::spawn(async move {
        let mut lines = BufReader::new(stderr).lines();
        loop {
            match lines.next_line().await {
                Ok(Some(line)) => {
                    eprintln!("[MSP][EXTERNAL][{stage}][{label}][stderr] {line}");
                }
                Ok(None) => break,
                Err(error) => {
                    eprintln!(
                        "[MSP][EXTERNAL][{stage}][{label}][stderr-read-error] command={command_line} error={error}"
                    );
                    break;
                }
            }
        }
    });
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
}
