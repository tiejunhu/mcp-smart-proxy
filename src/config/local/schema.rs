use std::collections::BTreeMap;
use std::error::Error;

use toml::{Table, Value};

use super::{ServerConfigSnapshot, parse_toml_string_array, parse_toml_string_table};

pub(crate) enum ParsedServerTransport {
    Stdio {
        command: String,
        args: Vec<String>,
    },
    Remote {
        url: String,
        headers: BTreeMap<String, String>,
    },
}

pub(crate) struct ParsedServerEntry {
    pub(crate) transport_name: &'static str,
    pub(crate) transport: ParsedServerTransport,
    pub(crate) enabled: bool,
    pub(crate) env: BTreeMap<String, String>,
    pub(crate) env_vars: Vec<String>,
}

pub(crate) fn parse_server_enabled(server: &Table, name: &str) -> Result<bool, Box<dyn Error>> {
    match server.get("enabled") {
        Some(Value::Boolean(enabled)) => Ok(*enabled),
        Some(_) => Err(format!("server `{name}` has a non-boolean `enabled` field").into()),
        None => Ok(true),
    }
}

pub(crate) fn resolved_server_transport(
    server: &Table,
    name: &str,
) -> Result<&'static str, Box<dyn Error>> {
    if let Some(transport) = configured_server_transport(server, name)? {
        return Ok(transport);
    }

    infer_server_transport(server, name)
}

fn configured_server_transport(
    server: &Table,
    name: &str,
) -> Result<Option<&'static str>, Box<dyn Error>> {
    match server.get("transport") {
        Some(Value::String(transport)) => match transport.as_str() {
            "stdio" => Ok(Some("stdio")),
            "remote" => Ok(Some("remote")),
            other => Err(format!(
                "server `{name}` uses unsupported transport `{other}`, only `stdio` and `remote` are supported"
            )
            .into()),
        },
        Some(_) => Err(format!("server `{name}` has a non-string `transport` field").into()),
        None => Ok(None),
    }
}

fn infer_server_transport(server: &Table, name: &str) -> Result<&'static str, Box<dyn Error>> {
    let has_command = server.contains_key("command");
    let has_url = server.contains_key("url");

    match (has_command, has_url) {
        (true, _) => Ok("stdio"),
        (false, true) => Ok("remote"),
        (false, false) => Err(format!("server `{name}` must define `command` or `url`").into()),
    }
}

pub(crate) fn parse_remote_server_url<'a>(
    server: &'a Table,
    name: &str,
) -> Result<&'a str, Box<dyn Error>> {
    server
        .get("url")
        .and_then(Value::as_str)
        .ok_or_else(|| format!("server `{name}` is missing `url`").into())
}

pub(crate) fn server_config_snapshot(
    resolved_name: &str,
    server: &Table,
) -> Result<ServerConfigSnapshot, Box<dyn Error>> {
    let parsed = parse_server_entry(server, resolved_name)?;
    let (command, args, url, headers) = match parsed.transport {
        ParsedServerTransport::Stdio { command, args } => {
            (Some(command), args, None, BTreeMap::new())
        }
        ParsedServerTransport::Remote { url, headers } => (None, Vec::new(), Some(url), headers),
    };

    Ok(ServerConfigSnapshot {
        name: resolved_name.to_string(),
        transport: parsed.transport_name.to_string(),
        enabled: parsed.enabled,
        command,
        args,
        url,
        headers,
        env: parsed.env,
        env_vars: parsed.env_vars,
    })
}

pub(crate) fn parse_server_entry(
    server: &Table,
    name: &str,
) -> Result<ParsedServerEntry, Box<dyn Error>> {
    let transport_name = resolved_server_transport(server, name)?;
    let transport = match transport_name {
        "stdio" => {
            let (command, args) = parse_stdio_command(server, name)?;
            ParsedServerTransport::Stdio { command, args }
        }
        "remote" => ParsedServerTransport::Remote {
            url: parse_remote_server_url(server, name)?.to_string(),
            headers: parse_toml_string_table(server.get("headers"), "headers", "server", name)?,
        },
        other => {
            return Err(format!(
                "server `{name}` uses unsupported transport `{other}`, only `stdio` and `remote` are supported"
            )
            .into())
        }
    };

    Ok(ParsedServerEntry {
        transport_name,
        transport,
        enabled: parse_server_enabled(server, name)?,
        env: parse_toml_string_table(server.get("env"), "env", "server", name)?,
        env_vars: parse_toml_string_array(server.get("env_vars"), "env_vars", "server", name)?,
    })
}

fn parse_stdio_command(
    server: &Table,
    name: &str,
) -> Result<(String, Vec<String>), Box<dyn Error>> {
    let command = server
        .get("command")
        .and_then(Value::as_str)
        .ok_or_else(|| format!("server `{name}` is missing `command`"))?
        .to_string();
    let args = parse_toml_string_array(server.get("args"), "args", "server", name)?;

    Ok((command, args))
}
