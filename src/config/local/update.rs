use std::collections::BTreeMap;
use std::error::Error;
use std::path::Path;

use toml::{Table, Value};

use super::{
    UpdateServerConfig, load_config_table, load_server_config, looks_like_url, merge_env_vars,
    parse_remote_server_url, parse_toml_string_array, parse_toml_string_table,
    resolved_server_table_mut, resolved_server_transport, save_config_table, upsert_string_array,
    upsert_string_table,
};

struct ServerTransportUpdate {
    current: String,
    next: String,
}

pub fn update_server_config(
    config_path: &Path,
    requested_name: &str,
    update: &UpdateServerConfig,
) -> Result<super::ServerConfigSnapshot, Box<dyn Error>> {
    let mut config = load_config_table(config_path)?;
    let resolved_name = {
        let (resolved_name, server) = resolved_server_table_mut(&mut config, requested_name)?;
        let transport = resolve_transport_update(server, &resolved_name, update)?;

        apply_transport_setting(server, &transport, update);
        apply_transport_specific_changes(server, &resolved_name, update, &transport)?;
        apply_enabled_update(server, update);
        apply_env_update(server, &resolved_name, update)?;
        apply_env_vars_update(server, &resolved_name, update)?;

        resolved_name
    };

    save_config_table(config_path, &config)?;
    load_server_config(config_path, &resolved_name)
}

fn resolve_transport_update(
    server: &Table,
    resolved_name: &str,
    update: &UpdateServerConfig,
) -> Result<ServerTransportUpdate, Box<dyn Error>> {
    let current = resolved_server_transport(server, resolved_name)?.to_string();
    let next = determine_next_transport(resolved_name, &current, update)?;

    validate_transport_transition(resolved_name, &current, &next, update)?;

    Ok(ServerTransportUpdate { current, next })
}

fn determine_next_transport(
    resolved_name: &str,
    current_transport: &str,
    update: &UpdateServerConfig,
) -> Result<String, Box<dyn Error>> {
    let next_transport = if let Some(url) = &update.url {
        if !looks_like_url(url) {
            return Err(format!(
                "server `{resolved_name}` has an invalid remote `url` value `{url}`"
            )
            .into());
        }
        "remote".to_string()
    } else if current_transport == "remote" && update.command.is_some() {
        "stdio".to_string()
    } else if let Some(transport) = &update.transport {
        transport.clone()
    } else {
        current_transport.to_string()
    };

    match next_transport.as_str() {
        "stdio" | "remote" => Ok(next_transport),
        other => Err(format!(
            "server `{resolved_name}` uses unsupported transport `{other}`, only `stdio` and `remote` are supported"
        )
        .into()),
    }
}

fn validate_transport_transition(
    resolved_name: &str,
    current_transport: &str,
    next_transport: &str,
    update: &UpdateServerConfig,
) -> Result<(), Box<dyn Error>> {
    if next_transport == "remote"
        && (update.command.is_some() || update.clear_args || !update.add_args.is_empty())
    {
        return Err(format!(
            "server `{resolved_name}` uses remote transport; update it with `--url` and header flags instead of `--cmd` or `--arg`"
        )
        .into());
    }

    if next_transport == "stdio"
        && current_transport == "remote"
        && update.command.is_none()
        && update.transport.as_deref() == Some("stdio")
    {
        return Err(format!(
            "server `{resolved_name}` uses remote transport; pass `--cmd` when converting it to stdio"
        )
        .into());
    }

    Ok(())
}

fn apply_transport_setting(
    server: &mut Table,
    transport: &ServerTransportUpdate,
    update: &UpdateServerConfig,
) {
    match update.transport.as_deref() {
        Some("stdio") | Some("remote") => {
            server.insert(
                "transport".to_string(),
                Value::String(transport.next.clone()),
            );
        }
        Some(_) => unreachable!("unsupported transport already rejected"),
        None if transport.next != transport.current => {
            server.remove("transport");
        }
        None => {}
    }
}

fn apply_transport_specific_changes(
    server: &mut Table,
    resolved_name: &str,
    update: &UpdateServerConfig,
    transport: &ServerTransportUpdate,
) -> Result<(), Box<dyn Error>> {
    match transport.next.as_str() {
        "stdio" => apply_stdio_changes(server, resolved_name, update, &transport.current),
        "remote" => apply_remote_changes(server, resolved_name, update, &transport.current),
        _ => unreachable!("unsupported transport already validated"),
    }
}

fn apply_stdio_changes(
    server: &mut Table,
    resolved_name: &str,
    update: &UpdateServerConfig,
    current_transport: &str,
) -> Result<(), Box<dyn Error>> {
    server.remove("url");
    server.remove("headers");

    if let Some(command) = &update.command {
        server.insert("command".to_string(), Value::String(command.clone()));
    } else if current_transport == "remote" {
        return Err(format!(
            "server `{resolved_name}` uses remote transport; pass `--cmd` when converting it to stdio"
        )
        .into());
    }

    if current_transport == "remote" || update.clear_args || !update.add_args.is_empty() {
        let mut args = if update.clear_args || current_transport == "remote" {
            Vec::new()
        } else {
            parse_toml_string_array(server.get("args"), "args", "server", resolved_name)?
        };
        args.extend(update.add_args.iter().cloned());
        upsert_string_array(server, "args", args);
    }

    Ok(())
}

fn apply_remote_changes(
    server: &mut Table,
    resolved_name: &str,
    update: &UpdateServerConfig,
    current_transport: &str,
) -> Result<(), Box<dyn Error>> {
    server.remove("command");
    server.remove("args");

    let url = match &update.url {
        Some(url) => url.clone(),
        None => parse_remote_server_url(server, resolved_name)?.to_string(),
    };
    server.insert("url".to_string(), Value::String(url));

    if update.clear_headers || !update.set_headers.is_empty() || !update.unset_headers.is_empty() {
        let mut headers = if update.clear_headers || current_transport != "remote" {
            BTreeMap::new()
        } else {
            parse_toml_string_table(server.get("headers"), "headers", "server", resolved_name)?
        };
        for key in &update.unset_headers {
            headers.remove(key);
        }
        for (key, value) in &update.set_headers {
            headers.insert(key.clone(), value.clone());
        }
        upsert_string_table(server, "headers", headers);
    } else if current_transport != "remote" {
        server.remove("headers");
    }

    Ok(())
}

fn apply_enabled_update(server: &mut Table, update: &UpdateServerConfig) {
    if let Some(enabled) = update.enabled {
        server.insert("enabled".to_string(), Value::Boolean(enabled));
    }
}

fn apply_env_update(
    server: &mut Table,
    resolved_name: &str,
    update: &UpdateServerConfig,
) -> Result<(), Box<dyn Error>> {
    if !(update.clear_env || !update.set_env.is_empty() || !update.unset_env.is_empty()) {
        return Ok(());
    }

    let mut env = if update.clear_env {
        BTreeMap::new()
    } else {
        parse_toml_string_table(server.get("env"), "env", "server", resolved_name)?
    };
    for key in &update.unset_env {
        env.remove(key);
    }
    for (key, value) in &update.set_env {
        env.insert(key.clone(), value.clone());
    }
    upsert_string_table(server, "env", env);

    Ok(())
}

fn apply_env_vars_update(
    server: &mut Table,
    resolved_name: &str,
    update: &UpdateServerConfig,
) -> Result<(), Box<dyn Error>> {
    if !(update.clear_env_vars
        || !update.add_env_vars.is_empty()
        || !update.unset_env_vars.is_empty())
    {
        return Ok(());
    }

    let mut env_vars = if update.clear_env_vars {
        Vec::new()
    } else {
        parse_toml_string_array(server.get("env_vars"), "env_vars", "server", resolved_name)?
    };
    env_vars.retain(|name| !update.unset_env_vars.contains(name));
    merge_env_vars(&mut env_vars, update.add_env_vars.clone());
    upsert_string_array(server, "env_vars", env_vars);

    Ok(())
}
