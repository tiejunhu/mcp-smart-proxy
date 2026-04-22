use std::env;
use std::error::Error;
use std::path::{Path, PathBuf};

use crate::paths::expand_tilde;
use crate::types::{
    ClaudeRuntimeConfig, CodexRuntimeConfig, CopilotRuntimeConfig, ModelProviderConfig,
    OpencodeRuntimeConfig,
};

use super::{
    CLAUDE_PROVIDER_NAME, CODEX_HOME_ENV, CODEX_PROVIDER_NAME, COPILOT_HOME_ENV,
    COPILOT_PROVIDER_NAME, DEFAULT_CLAUDE_CONFIG_PATH, DEFAULT_CLAUDE_MODEL,
    DEFAULT_CODEX_CONFIG_PATH, DEFAULT_COPILOT_CONFIG_PATH, DEFAULT_COPILOT_MODEL, DEFAULT_MODEL,
    DEFAULT_OPENCODE_CONFIG_PATH, DEFAULT_OPENCODE_MODEL, OPENCODE_PROVIDER_NAME,
};

pub fn load_codex_runtime_config() -> CodexRuntimeConfig {
    CodexRuntimeConfig {
        model: DEFAULT_MODEL.to_string(),
    }
}

pub fn load_opencode_runtime_config() -> OpencodeRuntimeConfig {
    OpencodeRuntimeConfig {
        model: DEFAULT_OPENCODE_MODEL.to_string(),
    }
}

pub fn load_claude_runtime_config() -> ClaudeRuntimeConfig {
    ClaudeRuntimeConfig {
        model: DEFAULT_CLAUDE_MODEL.to_string(),
    }
}

pub fn load_copilot_runtime_config() -> CopilotRuntimeConfig {
    CopilotRuntimeConfig {
        model: DEFAULT_COPILOT_MODEL.to_string(),
    }
}

pub fn load_model_provider_config(provider: &str) -> Result<ModelProviderConfig, Box<dyn Error>> {
    match provider {
        CODEX_PROVIDER_NAME => Ok(ModelProviderConfig::Codex(load_codex_runtime_config())),
        OPENCODE_PROVIDER_NAME => Ok(ModelProviderConfig::Opencode(load_opencode_runtime_config())),
        CLAUDE_PROVIDER_NAME => Ok(ModelProviderConfig::Claude(load_claude_runtime_config())),
        COPILOT_PROVIDER_NAME => Ok(ModelProviderConfig::Copilot(load_copilot_runtime_config())),
        _ => Err(format!(
            "unsupported provider `{provider}`; supported providers are `codex`, `opencode`, `claude`, and `copilot`"
        )
        .into()),
    }
}

pub(crate) fn codex_config_path() -> Result<PathBuf, Box<dyn Error>> {
    if let Some(codex_home) = env::var_os(CODEX_HOME_ENV).filter(|value| !value.is_empty()) {
        return Ok(PathBuf::from(codex_home).join("config.toml"));
    }

    expand_tilde(Path::new(DEFAULT_CODEX_CONFIG_PATH))
}

pub(crate) fn opencode_config_path() -> Result<PathBuf, Box<dyn Error>> {
    expand_tilde(Path::new(DEFAULT_OPENCODE_CONFIG_PATH))
}

pub(crate) fn claude_config_path() -> Result<PathBuf, Box<dyn Error>> {
    expand_tilde(Path::new(DEFAULT_CLAUDE_CONFIG_PATH))
}

pub(crate) fn copilot_config_path() -> Result<PathBuf, Box<dyn Error>> {
    if let Some(copilot_home) = env::var_os(COPILOT_HOME_ENV).filter(|value| !value.is_empty()) {
        return Ok(PathBuf::from(copilot_home).join("mcp-config.json"));
    }

    expand_tilde(Path::new(DEFAULT_COPILOT_CONFIG_PATH))
}
