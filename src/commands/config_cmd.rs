use std::collections::BTreeMap;
use std::error::Error;
use std::path::Path;

use crate::cli::ServerTransport;
use crate::config::{ServerConfigSnapshot, UpdateServerConfig};
use crate::console::print_app_event;
use crate::paths::format_path_for_display;

pub(crate) struct ConfigCommandArgs {
    pub(crate) transport: Option<ServerTransport>,
    pub(crate) command: Option<String>,
    pub(crate) args: Vec<String>,
    pub(crate) clear_args: bool,
    pub(crate) url: Option<String>,
    pub(crate) enabled: Option<bool>,
    pub(crate) headers: Vec<String>,
    pub(crate) unset_headers: Vec<String>,
    pub(crate) clear_headers: bool,
    pub(crate) env: Vec<String>,
    pub(crate) unset_env: Vec<String>,
    pub(crate) clear_env: bool,
    pub(crate) env_vars: Vec<String>,
    pub(crate) unset_env_vars: Vec<String>,
    pub(crate) clear_env_vars: bool,
}

impl ConfigCommandArgs {
    pub(crate) fn into_update_config(
        self,
        server_name: &str,
    ) -> Result<UpdateServerConfig, Box<dyn Error>> {
        let set_headers =
            parse_key_value_assignments(&self.headers, "header").map_err(|error| {
                format!("failed to parse `--header` values for server `{server_name}`: {error}")
            })?;
        let set_env = parse_key_value_assignments(&self.env, "env").map_err(|error| {
            format!("failed to parse `--env` values for server `{server_name}`: {error}")
        })?;

        Ok(UpdateServerConfig {
            transport: self.transport.map(|value| value.as_str().to_string()),
            command: self.command,
            clear_args: self.clear_args,
            add_args: self.args,
            url: self.url,
            enabled: self.enabled,
            clear_headers: self.clear_headers,
            set_headers,
            unset_headers: self.unset_headers,
            clear_env: self.clear_env,
            set_env,
            unset_env: self.unset_env,
            clear_env_vars: self.clear_env_vars,
            add_env_vars: self.env_vars,
            unset_env_vars: self.unset_env_vars,
        })
    }
}

pub(crate) fn print_server_config(
    stage: &str,
    config_path: &Path,
    snapshot: &ServerConfigSnapshot,
) {
    print_app_event(
        stage,
        format!(
            "Server `{}` in {}",
            snapshot.name,
            format_path_for_display(config_path)
        ),
    );
    print_app_event(stage, format!("transport: {}", snapshot.transport));
    print_app_event(stage, format!("enabled: {}", snapshot.enabled));
    if let Some(command) = &snapshot.command {
        print_app_event(stage, format!("command: {command}"));
        if snapshot.args.is_empty() {
            print_app_event(stage, "args: []");
        } else {
            print_app_event(stage, format!("args: [{}]", snapshot.args.join(", ")));
        }
    }
    if let Some(url) = &snapshot.url {
        print_app_event(stage, format!("url: {url}"));
        if snapshot.headers.is_empty() {
            print_app_event(stage, "headers: {}");
        } else {
            for (key, value) in &snapshot.headers {
                print_app_event(stage, format!("headers.{key}: {value}"));
            }
        }
    }
    if snapshot.env.is_empty() {
        print_app_event(stage, "env: {}");
    } else {
        for (key, value) in &snapshot.env {
            print_app_event(stage, format!("env.{key}: {value}"));
        }
    }
    if snapshot.env_vars.is_empty() {
        print_app_event(stage, "env_vars: []");
    } else {
        print_app_event(
            stage,
            format!("env_vars: [{}]", snapshot.env_vars.join(", ")),
        );
    }
}

fn parse_key_value_assignments(
    assignments: &[String],
    flag_name: &str,
) -> Result<BTreeMap<String, String>, Box<dyn Error>> {
    let mut env = BTreeMap::new();

    for assignment in assignments {
        let Some((key, value)) = assignment.split_once('=') else {
            return Err(format!(
                "invalid {flag_name} assignment `{assignment}`; expected `KEY=VALUE`"
            )
            .into());
        };
        if key.is_empty() {
            return Err(format!(
                "invalid {flag_name} assignment `{assignment}`; key must not be empty"
            )
            .into());
        }
        env.insert(key.to_string(), value.to_string());
    }

    Ok(env)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_env_assignments_into_sorted_map() {
        let env = parse_key_value_assignments(
            &[
                "B=two".to_string(),
                "A=one".to_string(),
                "B=override".to_string(),
            ],
            "env",
        )
        .unwrap();

        assert_eq!(
            env,
            BTreeMap::from([
                ("A".to_string(), "one".to_string()),
                ("B".to_string(), "override".to_string()),
            ])
        );
    }

    #[test]
    fn rejects_invalid_env_assignment() {
        let error = parse_key_value_assignments(&["INVALID".to_string()], "env").unwrap_err();

        assert_eq!(
            error.to_string(),
            "invalid env assignment `INVALID`; expected `KEY=VALUE`"
        );
    }
}
