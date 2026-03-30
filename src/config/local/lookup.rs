use std::error::Error;

use toml::{Table, Value};

use crate::paths::sanitize_name;

pub(crate) fn has_server_name(config: &Table, name: &str) -> bool {
    config
        .get("servers")
        .and_then(Value::as_table)
        .map(|servers| servers.contains_key(name))
        .unwrap_or(false)
}

pub(crate) fn resolve_server_name(servers: &Table, requested_name: &str) -> Option<String> {
    if servers.contains_key(requested_name) {
        return Some(requested_name.to_string());
    }

    let normalized = sanitize_name(requested_name);
    if normalized.is_empty() {
        return None;
    }

    servers.contains_key(&normalized).then_some(normalized)
}

pub(crate) fn resolved_server_table<'a>(
    config: &'a Table,
    requested_name: &str,
) -> Result<(String, &'a Table), Box<dyn Error>> {
    let servers = config
        .get("servers")
        .and_then(Value::as_table)
        .ok_or_else(|| "no `servers` table found in config".to_string())?;

    let resolved_name = resolve_server_name(servers, requested_name)
        .ok_or_else(|| format!("server `{requested_name}` not found"))?;
    let server = servers
        .get(&resolved_name)
        .and_then(Value::as_table)
        .ok_or_else(|| format!("server `{resolved_name}` must be a table"))?;

    Ok((resolved_name, server))
}

pub(crate) fn resolved_server_table_mut<'a>(
    config: &'a mut Table,
    requested_name: &str,
) -> Result<(String, &'a mut Table), Box<dyn Error>> {
    let servers = config
        .get_mut("servers")
        .and_then(Value::as_table_mut)
        .ok_or_else(|| "no `servers` table found in config".to_string())?;

    let resolved_name = resolve_server_name(servers, requested_name)
        .ok_or_else(|| format!("server `{requested_name}` not found"))?;
    let server = servers
        .get_mut(&resolved_name)
        .and_then(Value::as_table_mut)
        .ok_or_else(|| format!("server `{resolved_name}` must be a table"))?;

    Ok((resolved_name, server))
}
