use std::error::Error;
use std::path::PathBuf;

use crate::cli::{ImportSource, InstallTarget, ProviderName};
use crate::config::{
    ImportPlan, InstallMcpServerResult, ReplaceMcpServersResult, RestoreMcpServersResult,
    install_claude_mcp_server, install_codex_mcp_server, install_opencode_mcp_server,
    load_claude_servers_for_import, load_codex_servers_for_import, load_model_provider_config,
    load_opencode_servers_for_import, replace_claude_mcp_servers, replace_codex_mcp_servers,
    replace_opencode_mcp_servers, restore_claude_mcp_servers, restore_codex_mcp_servers,
    restore_opencode_mcp_servers,
};
use crate::types::ModelProviderConfig;

pub(crate) type ImportPlanLoader = fn() -> Result<(PathBuf, ImportPlan), Box<dyn Error>>;
type InstallFn = fn() -> Result<InstallMcpServerResult, Box<dyn Error>>;
type ReplaceFn = fn() -> Result<ReplaceMcpServersResult, Box<dyn Error>>;
type RestoreFn = fn() -> Result<RestoreMcpServersResult, Box<dyn Error>>;

pub(crate) struct ProviderHooks {
    pub(crate) provider_name: &'static str,
    pub(crate) import_source: ImportSource,
    pub(crate) load_import_plan: ImportPlanLoader,
    pub(crate) install_server: InstallFn,
    pub(crate) replace_servers: ReplaceFn,
    pub(crate) restore_servers: RestoreFn,
}

pub(crate) fn provider_hooks_for_import_source(source: ImportSource) -> ProviderHooks {
    match source {
        ImportSource::Codex => provider_hooks("codex"),
        ImportSource::Opencode => provider_hooks("opencode"),
        ImportSource::Claude => provider_hooks("claude"),
    }
}

pub(crate) fn provider_hooks_for_install_target(target: InstallTarget) -> ProviderHooks {
    match target {
        InstallTarget::Codex => provider_hooks("codex"),
        InstallTarget::Opencode => provider_hooks("opencode"),
        InstallTarget::Claude => provider_hooks("claude"),
    }
}

pub(crate) fn import_stage(provider_name: &'static str, suffix: &'static str) -> &'static str {
    match (provider_name, suffix) {
        ("codex", "load_provider") => "cli.import.codex.load_provider",
        ("codex", "load_source") => "cli.import.codex.load_source",
        ("codex", "run") => "cli.import.codex",
        ("opencode", "load_provider") => "cli.import.opencode.load_provider",
        ("opencode", "load_source") => "cli.import.opencode.load_source",
        ("opencode", "run") => "cli.import.opencode",
        ("claude", "load_provider") => "cli.import.claude.load_provider",
        ("claude", "load_source") => "cli.import.claude.load_source",
        ("claude", "run") => "cli.import.claude",
        _ => unreachable!(),
    }
}

pub(crate) fn install_stage(provider_name: &'static str) -> &'static str {
    match provider_name {
        "codex" => "cli.install.codex",
        "opencode" => "cli.install.opencode",
        "claude" => "cli.install.claude",
        _ => unreachable!(),
    }
}

pub(crate) fn restore_stage(provider_name: &'static str) -> &'static str {
    match provider_name {
        "codex" => "cli.restore.codex",
        "opencode" => "cli.restore.opencode",
        "claude" => "cli.restore.claude",
        _ => unreachable!(),
    }
}

pub(crate) fn resolve_default_command_provider(
    provider_override: Option<ProviderName>,
) -> Result<ModelProviderConfig, Box<dyn Error>> {
    let provider = provider_override.ok_or_else(|| {
        "missing required `--provider`; supported providers are `codex`, `opencode`, and `claude`"
            .to_string()
    })?;
    load_model_provider_config(provider.as_str())
}

pub(crate) fn resolve_import_provider(
    provider_override: Option<ProviderName>,
    source: ImportSource,
) -> Result<ModelProviderConfig, Box<dyn Error>> {
    match provider_override {
        Some(provider) => load_model_provider_config(provider.as_str()),
        None => load_model_provider_config(import_source_provider_name(source)),
    }
}

pub(crate) fn resolve_install_import_provider(
    source: ImportSource,
) -> Result<ModelProviderConfig, Box<dyn Error>> {
    load_model_provider_config(import_source_provider_name(source))
}

fn import_source_provider_name(source: ImportSource) -> &'static str {
    match source {
        ImportSource::Codex => "codex",
        ImportSource::Opencode => "opencode",
        ImportSource::Claude => "claude",
    }
}

fn provider_hooks(provider_name: &'static str) -> ProviderHooks {
    match provider_name {
        "codex" => ProviderHooks {
            provider_name,
            import_source: ImportSource::Codex,
            load_import_plan: load_codex_servers_for_import,
            install_server: install_codex_mcp_server,
            replace_servers: replace_codex_mcp_servers,
            restore_servers: restore_codex_mcp_servers,
        },
        "opencode" => ProviderHooks {
            provider_name,
            import_source: ImportSource::Opencode,
            load_import_plan: load_opencode_servers_for_import,
            install_server: install_opencode_mcp_server,
            replace_servers: replace_opencode_mcp_servers,
            restore_servers: restore_opencode_mcp_servers,
        },
        "claude" => ProviderHooks {
            provider_name,
            import_source: ImportSource::Claude,
            load_import_plan: load_claude_servers_for_import,
            install_server: install_claude_mcp_server,
            replace_servers: replace_claude_mcp_servers,
            restore_servers: restore_claude_mcp_servers,
        },
        _ => unreachable!(),
    }
}

#[cfg(test)]
pub(crate) fn missing_provider_error() -> &'static str {
    "missing required `--provider`; supported providers are `codex`, `opencode`, and `claude`"
}
