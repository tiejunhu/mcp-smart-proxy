use std::collections::BTreeMap;
use std::env;
use std::ffi::OsString;

use rmcp::model::{CallToolResult, Tool};
use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConfiguredTransport {
    Stdio {
        command: String,
        args: Vec<String>,
    },
    Remote {
        url: String,
        headers: BTreeMap<String, String>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfiguredServer {
    pub transport: ConfiguredTransport,
    pub description: Option<String>,
    pub env: BTreeMap<String, String>,
    pub env_vars: Vec<String>,
}

impl ConfiguredServer {
    #[cfg(test)]
    pub fn stdio_transport(&self) -> Option<(&str, &[String])> {
        match &self.transport {
            ConfiguredTransport::Stdio { command, args } => Some((command, args)),
            ConfiguredTransport::Remote { .. } => None,
        }
    }

    #[cfg(test)]
    pub fn remote_transport(&self) -> Option<(&str, &BTreeMap<String, String>)> {
        match &self.transport {
            ConfiguredTransport::Remote { url, headers } => Some((url, headers)),
            ConfiguredTransport::Stdio { .. } => None,
        }
    }

    pub fn resolved_env(&self) -> Vec<(String, OsString)> {
        let mut resolved = BTreeMap::new();

        for name in &self.env_vars {
            if let Some(value) = env::var_os(name) {
                resolved.insert(name.clone(), value);
            }
        }

        for (name, value) in &self.env {
            resolved.insert(name.clone(), OsString::from(value));
        }

        resolved.into_iter().collect()
    }

    pub fn resolved_env_map(&self) -> BTreeMap<String, OsString> {
        self.resolved_env().into_iter().collect()
    }
}

impl Default for ConfiguredServer {
    fn default() -> Self {
        Self {
            transport: ConfiguredTransport::Stdio {
                command: String::new(),
                args: Vec::new(),
            },
            description: None,
            env: BTreeMap::new(),
            env_vars: Vec::new(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct CodexRuntimeConfig {
    pub model: String,
}

#[derive(Debug, Clone)]
pub struct OpencodeRuntimeConfig {
    pub model: String,
}

#[derive(Debug, Clone)]
pub struct ClaudeRuntimeConfig {
    pub model: String,
}

#[derive(Debug, Clone)]
pub struct CopilotRuntimeConfig {
    pub model: String,
}

#[derive(Debug, Clone)]
pub enum ModelProviderConfig {
    Codex(CodexRuntimeConfig),
    Opencode(OpencodeRuntimeConfig),
    Claude(ClaudeRuntimeConfig),
    Copilot(CopilotRuntimeConfig),
}

impl ModelProviderConfig {
    pub fn provider_name(&self) -> &'static str {
        match self {
            Self::Codex(_) => "codex",
            Self::Opencode(_) => "opencode",
            Self::Claude(_) => "claude",
            Self::Copilot(_) => "copilot",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CachedTools {
    pub server: String,
    pub summary: String,
    pub fetched_at_epoch_ms: u128,
    pub tools: Vec<ToolSnapshot>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolSnapshot {
    pub name: String,
    pub title: Option<String>,
    pub description: Option<String>,
    pub input_schema: JsonValue,
    pub output_schema: Option<JsonValue>,
    pub annotations: Option<JsonValue>,
    pub execution: Option<JsonValue>,
    pub icons: Option<JsonValue>,
    pub meta: Option<JsonValue>,
}

pub fn tool_snapshot(tool: &Tool) -> ToolSnapshot {
    ToolSnapshot {
        name: tool.name.to_string(),
        title: tool.title.clone(),
        description: tool.description.as_ref().map(ToString::to_string),
        input_schema: JsonValue::Object((*(tool.input_schema.clone())).clone()),
        output_schema: tool
            .output_schema
            .as_ref()
            .map(|schema| JsonValue::Object((**schema).clone())),
        annotations: tool.annotations.as_ref().map(json_value_or_null),
        execution: tool.execution.as_ref().map(json_value_or_null),
        icons: tool.icons.as_ref().map(json_value_or_null),
        meta: tool.meta.as_ref().map(json_value_or_null),
    }
}

fn json_value_or_null<T: Serialize>(value: &T) -> JsonValue {
    serde_json::to_value(value).unwrap_or(JsonValue::Null)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CachedToolsetRecord {
    pub name: String,
    pub summary: String,
    pub tools: Vec<ToolSnapshot>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DaemonStatus {
    pub version: String,
    pub pid: u32,
    pub socket_path: String,
    pub config_path: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum DaemonRequest {
    Status,
    Exit,
    LoadToolsets {
        provider: Option<String>,
    },
    CallTool {
        toolset_name: String,
        tool_name: String,
        arguments: Option<serde_json::Map<String, JsonValue>>,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum DaemonResponse {
    Status { status: DaemonStatus },
    ExitAck,
    Toolsets { toolsets: Vec<CachedToolsetRecord> },
    ToolResult { result: CallToolResult },
    Error { message: String },
}

#[cfg(test)]
mod tests {
    use super::*;
    use rmcp::model::ToolAnnotations;
    use serde_json::json;
    use std::sync::{Mutex, OnceLock};

    fn env_lock() -> &'static Mutex<()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(()))
    }

    #[test]
    fn resolves_forwarded_and_static_env_vars() {
        let _guard = env_lock().lock().unwrap();
        let previous_forwarded = env::var("MSP_TEST_FORWARDED").ok();
        let previous_overridden = env::var("MSP_TEST_OVERRIDDEN").ok();

        unsafe {
            env::set_var("MSP_TEST_FORWARDED", "forwarded");
            env::set_var("MSP_TEST_OVERRIDDEN", "from-process");
        }

        let server = ConfiguredServer {
            transport: ConfiguredTransport::Stdio {
                command: "demo".to_string(),
                args: Vec::new(),
            },
            description: None,
            env: BTreeMap::from([("MSP_TEST_OVERRIDDEN".to_string(), "from-config".to_string())]),
            env_vars: vec![
                "MSP_TEST_FORWARDED".to_string(),
                "MSP_TEST_OVERRIDDEN".to_string(),
                "MSP_TEST_MISSING".to_string(),
            ],
        };

        let resolved = server.resolved_env();

        assert_eq!(
            resolved,
            vec![
                ("MSP_TEST_FORWARDED".to_string(), "forwarded".into()),
                ("MSP_TEST_OVERRIDDEN".to_string(), "from-config".into()),
            ]
        );

        match previous_forwarded {
            Some(value) => unsafe { env::set_var("MSP_TEST_FORWARDED", value) },
            None => unsafe { env::remove_var("MSP_TEST_FORWARDED") },
        }
        match previous_overridden {
            Some(value) => unsafe { env::set_var("MSP_TEST_OVERRIDDEN", value) },
            None => unsafe { env::remove_var("MSP_TEST_OVERRIDDEN") },
        }
    }

    #[test]
    fn default_configured_server_has_no_env() {
        let server = ConfiguredServer::default();

        assert!(matches!(
            server.transport,
            ConfiguredTransport::Stdio {
                command: _,
                args: _
            }
        ));
        assert!(server.env.is_empty());
        assert!(server.env_vars.is_empty());
    }

    #[test]
    fn tool_snapshot_preserves_missing_annotations() {
        let tool = Tool::new("search", "Search things", serde_json::Map::new());

        let snapshot = tool_snapshot(&tool);

        assert_eq!(snapshot.annotations, None);
    }

    #[test]
    fn tool_snapshot_preserves_all_annotation_hints() {
        let tool = Tool::new("search", "Search things", serde_json::Map::new()).annotate(
            ToolAnnotations::new()
                .read_only(false)
                .destructive(true)
                .idempotent(true)
                .open_world(false),
        );

        let snapshot = tool_snapshot(&tool);

        assert_eq!(
            snapshot.annotations,
            Some(json!({
                "readOnlyHint": false,
                "destructiveHint": true,
                "idempotentHint": true,
                "openWorldHint": false
            }))
        );
    }
}
