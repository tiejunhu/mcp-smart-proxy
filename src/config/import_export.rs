mod claude;
mod codex;
mod common;
mod copilot;
mod json_support;
mod opencode;
mod toml_support;

pub use claude::{
    install_claude_mcp_server, load_claude_servers_for_import, replace_claude_mcp_servers,
    restore_claude_mcp_servers,
};
pub use codex::{
    install_codex_mcp_server, load_codex_servers_for_import, replace_codex_mcp_servers,
    restore_codex_mcp_servers,
};
pub use copilot::{
    install_copilot_mcp_server, load_copilot_servers_for_import, replace_copilot_mcp_servers,
    restore_copilot_mcp_servers,
};
pub use opencode::{
    install_opencode_mcp_server, load_opencode_servers_for_import, replace_opencode_mcp_servers,
    restore_opencode_mcp_servers,
};

#[cfg(test)]
pub(crate) use claude::{
    load_claude_config, load_claude_servers_for_import_from_path,
    replace_claude_mcp_servers_from_path, restore_claude_mcp_servers_from_path,
};
#[cfg(test)]
pub(crate) use codex::{
    load_codex_servers_for_import_from_path, replace_codex_mcp_servers_from_path,
    restore_codex_mcp_servers_from_path,
};
#[cfg(test)]
pub(crate) use common::collect_remote_header_value_env_vars;
#[cfg(test)]
pub(crate) use copilot::{
    load_copilot_config, load_copilot_servers_for_import_from_path,
    replace_copilot_mcp_servers_from_path, restore_copilot_mcp_servers_from_path,
};
#[cfg(test)]
pub(crate) use opencode::{
    load_opencode_config, load_opencode_servers_for_import_from_path,
    replace_opencode_mcp_servers_from_path, restore_opencode_mcp_servers_from_path,
};
