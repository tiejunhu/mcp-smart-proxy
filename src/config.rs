use std::collections::BTreeMap;
use std::error::Error;
use std::path::PathBuf;

mod import_export;
mod local;
mod provider;
mod self_server;

pub(crate) use self_server::is_self_server_command;

const DEFAULT_MODEL: &str = "gpt-5.2";
const DEFAULT_OPENCODE_MODEL: &str = "openai/gpt-5.2";
const DEFAULT_CLAUDE_MODEL: &str = "sonnet";
const DEFAULT_COPILOT_MODEL: &str = "gpt-5.2";
const DEFAULT_CODEX_CONFIG_PATH: &str = "~/.codex/config.toml";
const DEFAULT_OPENCODE_CONFIG_PATH: &str = "~/.config/opencode/opencode.json";
const DEFAULT_CLAUDE_CONFIG_PATH: &str = "~/.claude.json";
const DEFAULT_COPILOT_CONFIG_PATH: &str = "~/.copilot/mcp-config.json";
const CODEX_HOME_ENV: &str = "CODEX_HOME";
const COPILOT_HOME_ENV: &str = "COPILOT_HOME";
const CODEX_PROVIDER_NAME: &str = "codex";
const OPENCODE_PROVIDER_NAME: &str = "opencode";
const CLAUDE_PROVIDER_NAME: &str = "claude";
const COPILOT_PROVIDER_NAME: &str = "copilot";

#[derive(Debug, Clone, PartialEq, Eq)]
struct StdioServer {
    command: String,
    args: Vec<String>,
}

impl StdioServer {
    fn from_command(command: Vec<String>) -> Result<Self, Box<dyn Error>> {
        let mut parts = command.into_iter();
        let executable = parts
            .next()
            .ok_or_else(|| "missing stdio server command".to_string())?;

        Ok(Self {
            command: executable,
            args: parts.collect(),
        })
    }

    fn raw_command(&self) -> Vec<String> {
        let mut command = vec![self.command.clone()];
        command.extend(self.args.iter().cloned());
        command
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ImportedServerDefinition {
    command: Vec<String>,
    url: Option<String>,
    headers: BTreeMap<String, String>,
    env: BTreeMap<String, String>,
    env_vars: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImportableServer {
    pub name: String,
    pub command: Vec<String>,
    pub url: Option<String>,
    pub headers: BTreeMap<String, String>,
    pub enabled: bool,
    pub env: BTreeMap<String, String>,
    pub env_vars: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AddServerConfig {
    pub command: Vec<String>,
    pub description: Option<String>,
    pub url: Option<String>,
    pub headers: BTreeMap<String, String>,
    pub enabled: bool,
    pub env: BTreeMap<String, String>,
    pub env_vars: Vec<String>,
}

impl Default for AddServerConfig {
    fn default() -> Self {
        Self {
            command: Vec::new(),
            description: None,
            url: None,
            headers: BTreeMap::new(),
            enabled: true,
            env: BTreeMap::new(),
            env_vars: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ListedServer {
    pub name: String,
    pub command: String,
    pub args: Vec<String>,
    pub enabled: bool,
    pub last_updated_at: Option<u128>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImportPlan {
    pub servers: Vec<ImportableServer>,
    pub skipped_self_servers: Vec<String>,
    pub skipped_unsupported_servers: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemovedServer {
    pub name: String,
    pub cache_path: PathBuf,
    pub cache_deleted: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SetServerEnabledResult {
    pub name: String,
    pub enabled: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServerConfigSnapshot {
    pub name: String,
    pub transport: String,
    pub enabled: bool,
    pub description: Option<String>,
    pub command: Option<String>,
    pub args: Vec<String>,
    pub url: Option<String>,
    pub headers: BTreeMap<String, String>,
    pub env: BTreeMap<String, String>,
    pub env_vars: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct UpdateServerConfig {
    pub transport: Option<String>,
    pub command: Option<String>,
    pub clear_args: bool,
    pub add_args: Vec<String>,
    pub url: Option<String>,
    pub enabled: Option<bool>,
    pub clear_headers: bool,
    pub set_headers: BTreeMap<String, String>,
    pub unset_headers: Vec<String>,
    pub clear_env: bool,
    pub set_env: BTreeMap<String, String>,
    pub unset_env: Vec<String>,
    pub clear_env_vars: bool,
    pub add_env_vars: Vec<String>,
    pub unset_env_vars: Vec<String>,
}

impl UpdateServerConfig {
    pub fn has_changes(&self) -> bool {
        self.transport.is_some()
            || self.command.is_some()
            || self.clear_args
            || !self.add_args.is_empty()
            || self.url.is_some()
            || self.enabled.is_some()
            || self.clear_headers
            || !self.set_headers.is_empty()
            || !self.unset_headers.is_empty()
            || self.clear_env
            || !self.set_env.is_empty()
            || !self.unset_env.is_empty()
            || self.clear_env_vars
            || !self.add_env_vars.is_empty()
            || !self.unset_env_vars.is_empty()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InstallMcpServerStatus {
    AlreadyInstalled,
    Updated,
    Installed,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstallMcpServerResult {
    pub name: String,
    pub config_path: PathBuf,
    pub status: InstallMcpServerStatus,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReplaceMcpServersResult {
    pub config_path: PathBuf,
    pub backup_path: PathBuf,
    pub backed_up_server_count: usize,
    pub removed_server_count: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RestoreMcpServersResult {
    pub config_path: PathBuf,
    pub backup_path: PathBuf,
    pub removed_self_server_count: usize,
    pub restored_server_count: usize,
}

pub use import_export::{
    install_claude_mcp_server, install_codex_mcp_server, install_copilot_mcp_server,
    install_opencode_mcp_server, load_claude_servers_for_import, load_codex_servers_for_import,
    load_copilot_servers_for_import, load_opencode_servers_for_import, replace_claude_mcp_servers,
    replace_codex_mcp_servers, replace_copilot_mcp_servers, replace_opencode_mcp_servers,
    restore_claude_mcp_servers, restore_codex_mcp_servers, restore_copilot_mcp_servers,
    restore_opencode_mcp_servers,
};
pub use local::{
    add_server_with_config, configured_server, contains_server_name, import_server, list_servers,
    load_config_table, load_server_config, remove_server, server_is_enabled, set_server_enabled,
    update_server_config,
};
pub use provider::load_model_provider_config;

#[cfg(test)]
pub(crate) use import_export::{
    collect_remote_header_value_env_vars, load_claude_config,
    load_claude_servers_for_import_from_path, load_codex_servers_for_import_from_path,
    load_copilot_config, load_copilot_servers_for_import_from_path, load_opencode_config,
    load_opencode_servers_for_import_from_path, replace_claude_mcp_servers_from_path,
    replace_codex_mcp_servers_from_path, replace_copilot_mcp_servers_from_path,
    replace_opencode_mcp_servers_from_path, restore_claude_mcp_servers_from_path,
    restore_codex_mcp_servers_from_path, restore_copilot_mcp_servers_from_path,
    restore_opencode_mcp_servers_from_path,
};
#[cfg(test)]
pub(crate) use provider::{codex_config_path, copilot_config_path};

#[cfg(test)]
mod tests;
