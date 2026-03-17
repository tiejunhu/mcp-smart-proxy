use rmcp::model::Tool;
use serde::Serialize;
use serde_json::Value as JsonValue;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfiguredServer {
    pub command: String,
    pub args: Vec<String>,
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
pub enum ModelProviderConfig {
    Codex(CodexRuntimeConfig),
    Opencode(OpencodeRuntimeConfig),
}

#[derive(Debug, Clone, Serialize, serde::Deserialize)]
pub struct CachedTools {
    pub server: String,
    pub summary: String,
    pub fetched_at_epoch_ms: u128,
    pub tools: Vec<ToolSnapshot>,
}

#[derive(Debug, Clone, Serialize, serde::Deserialize)]
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
