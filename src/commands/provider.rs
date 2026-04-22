use std::error::Error;
use std::path::PathBuf;

use crate::cli::{ImportSource, InstallTarget, ProviderName};
use crate::config::{
    ImportPlan, InstallMcpServerResult, ReplaceMcpServersResult, RestoreMcpServersResult,
    install_claude_mcp_server, install_codex_mcp_server, install_copilot_mcp_server,
    install_opencode_mcp_server, load_claude_servers_for_import, load_codex_servers_for_import,
    load_copilot_servers_for_import, load_model_provider_config, load_opencode_servers_for_import,
    replace_claude_mcp_servers, replace_codex_mcp_servers, replace_copilot_mcp_servers,
    replace_opencode_mcp_servers, restore_claude_mcp_servers, restore_codex_mcp_servers,
    restore_copilot_mcp_servers, restore_opencode_mcp_servers,
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
    import_load_provider_stage: &'static str,
    import_load_source_stage: &'static str,
    import_run_stage: &'static str,
    install_stage: &'static str,
    restore_stage: &'static str,
}

pub(crate) fn provider_hooks_for_import_source(source: ImportSource) -> ProviderHooks {
    match source {
        ImportSource::Codex => provider_hooks("codex"),
        ImportSource::Opencode => provider_hooks("opencode"),
        ImportSource::Claude => provider_hooks("claude"),
        ImportSource::Copilot => provider_hooks("copilot"),
    }
}

pub(crate) fn provider_hooks_for_install_target(target: InstallTarget) -> ProviderHooks {
    match target {
        InstallTarget::Codex => provider_hooks("codex"),
        InstallTarget::Opencode => provider_hooks("opencode"),
        InstallTarget::Claude => provider_hooks("claude"),
        InstallTarget::Copilot => provider_hooks("copilot"),
    }
}

pub(crate) fn import_stage(provider_name: &'static str, suffix: &'static str) -> &'static str {
    let hooks = provider_hooks(provider_name);
    match suffix {
        "load_provider" => hooks.import_load_provider_stage,
        "load_source" => hooks.import_load_source_stage,
        "run" => hooks.import_run_stage,
        _ => unreachable!(),
    }
}

pub(crate) fn install_stage(provider_name: &'static str) -> &'static str {
    provider_hooks(provider_name).install_stage
}

pub(crate) fn restore_stage(provider_name: &'static str) -> &'static str {
    provider_hooks(provider_name).restore_stage
}

pub(crate) fn resolve_default_command_provider(
    provider_override: Option<ProviderName>,
) -> Result<Option<ModelProviderConfig>, Box<dyn Error>> {
    provider_override
        .map(|provider| load_model_provider_config(provider.as_str()))
        .transpose()
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
    provider_hooks_for_import_source(source).provider_name
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
            import_load_provider_stage: "cli.import.codex.load_provider",
            import_load_source_stage: "cli.import.codex.load_source",
            import_run_stage: "cli.import.codex",
            install_stage: "cli.install.codex",
            restore_stage: "cli.restore.codex",
        },
        "opencode" => ProviderHooks {
            provider_name,
            import_source: ImportSource::Opencode,
            load_import_plan: load_opencode_servers_for_import,
            install_server: install_opencode_mcp_server,
            replace_servers: replace_opencode_mcp_servers,
            restore_servers: restore_opencode_mcp_servers,
            import_load_provider_stage: "cli.import.opencode.load_provider",
            import_load_source_stage: "cli.import.opencode.load_source",
            import_run_stage: "cli.import.opencode",
            install_stage: "cli.install.opencode",
            restore_stage: "cli.restore.opencode",
        },
        "claude" => ProviderHooks {
            provider_name,
            import_source: ImportSource::Claude,
            load_import_plan: load_claude_servers_for_import,
            install_server: install_claude_mcp_server,
            replace_servers: replace_claude_mcp_servers,
            restore_servers: restore_claude_mcp_servers,
            import_load_provider_stage: "cli.import.claude.load_provider",
            import_load_source_stage: "cli.import.claude.load_source",
            import_run_stage: "cli.import.claude",
            install_stage: "cli.install.claude",
            restore_stage: "cli.restore.claude",
        },
        "copilot" => ProviderHooks {
            provider_name,
            import_source: ImportSource::Copilot,
            load_import_plan: load_copilot_servers_for_import,
            install_server: install_copilot_mcp_server,
            replace_servers: replace_copilot_mcp_servers,
            restore_servers: restore_copilot_mcp_servers,
            import_load_provider_stage: "cli.import.copilot.load_provider",
            import_load_source_stage: "cli.import.copilot.load_source",
            import_run_stage: "cli.import.copilot",
            install_stage: "cli.install.copilot",
            restore_stage: "cli.restore.copilot",
        },
        _ => unreachable!(),
    }
}
