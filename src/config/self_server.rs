use std::path::Path;

use serde_json::{Map as JsonMap, Value as JsonValue};
use toml::{Table, Value};

use super::StdioServer;

const SELF_EXECUTABLE_NAME: &str = "msp";
const SELF_SUBCOMMAND_NAME: &str = "mcp";

pub(crate) fn is_self_server_command(raw_command: &[String]) -> bool {
    let Some(command) = raw_command.first() else {
        return false;
    };

    let executable = Path::new(command)
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or(command.as_str())
        .trim_end_matches(".exe");

    executable == SELF_EXECUTABLE_NAME
        && raw_command.get(1).map(String::as_str) == Some(SELF_SUBCOMMAND_NAME)
}

pub(crate) fn proxy_stdio_server(provider: &str) -> StdioServer {
    StdioServer {
        command: SELF_EXECUTABLE_NAME.to_string(),
        args: vec![
            SELF_SUBCOMMAND_NAME.to_string(),
            "--provider".to_string(),
            provider.to_string(),
        ],
    }
}

pub(crate) fn inspect_codex_self_server(servers: &Table, provider: &str) -> Option<(String, bool)> {
    inspect_self_server(
        servers.iter().filter_map(|(name, value)| {
            let server = value.as_table()?;
            let raw_command = codex_server_raw_command(server)?;
            Some((name.clone(), raw_command))
        }),
        provider,
    )
}

pub(crate) fn inspect_opencode_self_server(
    servers: &JsonMap<String, JsonValue>,
    provider: &str,
) -> Option<(String, bool)> {
    inspect_self_server(
        servers.iter().filter_map(|(name, value)| {
            let server = value.as_object()?;
            let raw_command = opencode_server_raw_command(server)?;
            Some((name.clone(), raw_command))
        }),
        provider,
    )
}

pub(crate) fn inspect_claude_self_server(
    servers: &JsonMap<String, JsonValue>,
    provider: &str,
) -> Option<(String, bool)> {
    inspect_self_server(
        servers.iter().filter_map(|(name, value)| {
            let server = value.as_object()?;
            let raw_command = claude_server_raw_command(server)?;
            Some((name.clone(), raw_command))
        }),
        provider,
    )
}

pub(crate) fn inspect_copilot_self_server(
    servers: &JsonMap<String, JsonValue>,
    provider: &str,
) -> Option<(String, bool)> {
    inspect_self_server(
        servers.iter().filter_map(|(name, value)| {
            let server = value.as_object()?;
            let raw_command = copilot_server_raw_command(server)?;
            Some((name.clone(), raw_command))
        }),
        provider,
    )
}

pub(crate) fn next_available_server_name<'a>(
    existing_names: impl Iterator<Item = &'a str>,
) -> String {
    let existing_names = existing_names.collect::<std::collections::BTreeSet<_>>();
    if !existing_names.contains("msp") {
        return "msp".to_string();
    }

    let mut index = 1usize;
    loop {
        let candidate = format!("msp{index}");
        if !existing_names.contains(candidate.as_str()) {
            return candidate;
        }
        index += 1;
    }
}

fn inspect_self_server(
    candidates: impl Iterator<Item = (String, Vec<String>)>,
    provider: &str,
) -> Option<(String, bool)> {
    let mut self_server_names = Vec::new();

    for (name, raw_command) in candidates {
        if !is_self_server_command(&raw_command) {
            continue;
        }
        if self_server_uses_provider(&raw_command, provider) {
            return Some((name, true));
        }
        self_server_names.push(name);
    }

    pick_existing_self_server_name(self_server_names).map(|name| (name, false))
}

pub(crate) fn codex_server_raw_command(server: &Table) -> Option<Vec<String>> {
    let command = server.get("command")?.as_str()?.to_string();
    let args = match server.get("args") {
        None => Vec::new(),
        Some(Value::Array(items)) => items
            .iter()
            .map(|value| value.as_str().map(ToOwned::to_owned))
            .collect::<Option<Vec<_>>>()?,
        Some(_) => return None,
    };

    let mut raw_command = vec![command];
    raw_command.extend(args);
    Some(raw_command)
}

pub(crate) fn opencode_server_raw_command(
    server: &JsonMap<String, JsonValue>,
) -> Option<Vec<String>> {
    server
        .get("command")?
        .as_array()?
        .iter()
        .map(|value| value.as_str().map(ToOwned::to_owned))
        .collect()
}

pub(crate) fn claude_server_raw_command(
    server: &JsonMap<String, JsonValue>,
) -> Option<Vec<String>> {
    match server
        .get("type")
        .and_then(JsonValue::as_str)
        .unwrap_or("stdio")
    {
        "stdio" => {
            let command = server.get("command")?.as_str()?.to_string();
            let args = match server.get("args") {
                None => Vec::new(),
                Some(JsonValue::Array(items)) => items
                    .iter()
                    .map(|value| value.as_str().map(ToOwned::to_owned))
                    .collect::<Option<Vec<_>>>()?,
                Some(_) => return None,
            };

            let mut raw_command = vec![command];
            raw_command.extend(args);
            Some(raw_command)
        }
        _ => None,
    }
}

pub(crate) fn copilot_server_raw_command(
    server: &JsonMap<String, JsonValue>,
) -> Option<Vec<String>> {
    match server
        .get("type")
        .and_then(JsonValue::as_str)
        .unwrap_or("stdio")
    {
        "local" | "stdio" => {
            let command = server.get("command")?.as_str()?.to_string();
            let args = match server.get("args") {
                None => Vec::new(),
                Some(JsonValue::Array(items)) => items
                    .iter()
                    .map(|value| value.as_str().map(ToOwned::to_owned))
                    .collect::<Option<Vec<_>>>()?,
                Some(_) => return None,
            };

            let mut raw_command = vec![command];
            raw_command.extend(args);
            Some(raw_command)
        }
        _ => None,
    }
}

fn self_server_uses_provider(raw_command: &[String], provider: &str) -> bool {
    is_self_server_command(raw_command)
        && raw_command.len() == 4
        && raw_command[1] == SELF_SUBCOMMAND_NAME
        && raw_command[2] == "--provider"
        && raw_command[3] == provider
}

fn pick_existing_self_server_name(mut names: Vec<String>) -> Option<String> {
    names.sort_by(|left, right| match (left == "msp", right == "msp") {
        (true, false) => std::cmp::Ordering::Less,
        (false, true) => std::cmp::Ordering::Greater,
        _ => left.cmp(right),
    });
    names.into_iter().next()
}
