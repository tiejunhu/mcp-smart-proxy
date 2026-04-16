use std::collections::{BTreeMap, BTreeSet};
use std::error::Error;
use std::ffi::OsString;
use std::path::Path;

use rmcp::model::CallToolResult;
use serde_json::{Map as JsonMap, Number, Value as JsonValue};

use crate::console::operation_error;
use crate::daemon;
use crate::paths::{format_path_for_display, sanitize_name};
use crate::toon::encode_json_to_toon;
use crate::types::{CachedToolsetRecord, ToolSnapshot};

const TOOL_DESCRIPTION_PREVIEW_CHARS: usize = 80;
const CLI_USAGE_PREFIX: &str = "msp cli";
const CLI_USAGE_HELP_PREFIX: &str = "msp cli [--output-toon]";

pub(super) async fn run_mcp_cli_command(
    config_path: &Path,
    output_toon: bool,
    args: &[OsString],
) -> Result<(), Box<dyn Error>> {
    daemon::ensure_daemon_running(config_path, None)
        .await
        .map_err(|error| {
            operation_error(
                "cli.cli.daemon",
                format!(
                    "failed to ensure the shared daemon is running for {}",
                    format_path_for_display(config_path)
                ),
                error,
            )
        })?;

    let toolsets = daemon::load_toolsets(config_path, None, None)
        .await
        .map_err(|error| {
            operation_error(
                "cli.cli.load_toolsets",
                format!(
                    "failed to load cached MCP toolsets through the shared daemon for {}",
                    format_path_for_display(config_path)
                ),
                error,
            )
        })?;

    let output = match classify_command(args) {
        CliCommand::RootHelp => render_root_help(&toolsets),
        CliCommand::ToolsetHelp { toolset_name } => {
            let toolset = resolve_toolset(&toolsets, &toolset_name)?;
            render_toolset_help(toolset)
        }
        CliCommand::ToolHelp {
            toolset_name,
            tool_name,
        } => {
            let toolset = resolve_toolset(&toolsets, &toolset_name)?;
            let tool = resolve_tool(toolset, &tool_name)?;
            render_tool_help(toolset, tool)
        }
        CliCommand::ToolCall {
            toolset_name,
            tool_name,
            arguments,
        } => {
            let toolset = resolve_toolset(&toolsets, &toolset_name)?;
            let tool = resolve_tool(toolset, &tool_name)?;
            let arguments = parse_tool_arguments(toolset, tool, &arguments)?;
            let result = daemon::call_tool(
                config_path,
                None,
                &toolset.name,
                &tool.name,
                Some(arguments),
            )
            .await
            .map_err(|error| {
                operation_error(
                    "cli.cli.call_tool",
                    format!(
                        "failed to call tool `{}` in MCP server `{}` through the shared daemon",
                        tool.name, toolset.name
                    ),
                    error,
                )
            })?;
            format_tool_result(&result, output_toon)?
        }
    };

    print!("{output}");
    Ok(())
}

enum CliCommand {
    RootHelp,
    ToolsetHelp {
        toolset_name: String,
    },
    ToolHelp {
        toolset_name: String,
        tool_name: String,
    },
    ToolCall {
        toolset_name: String,
        tool_name: String,
        arguments: Vec<OsString>,
    },
}

fn classify_command(args: &[OsString]) -> CliCommand {
    if args.is_empty() || is_help_flag(&args[0]) {
        return CliCommand::RootHelp;
    }

    let toolset_name = args[0].to_string_lossy().into_owned();
    if args.len() == 1 || is_help_flag(&args[1]) {
        return CliCommand::ToolsetHelp { toolset_name };
    }

    let tool_name = args[1].to_string_lossy().into_owned();
    if args.len() == 2 || args[2..].iter().any(is_help_flag) {
        return CliCommand::ToolHelp {
            toolset_name,
            tool_name,
        };
    }

    CliCommand::ToolCall {
        toolset_name,
        tool_name,
        arguments: args[2..].to_vec(),
    }
}

fn is_help_flag(arg: &OsString) -> bool {
    matches!(arg.to_string_lossy().as_ref(), "-h" | "--help")
}

fn render_root_help(toolsets: &[CachedToolsetRecord]) -> String {
    let width = toolsets
        .iter()
        .map(|toolset| toolset.name.chars().count())
        .max()
        .unwrap_or(0);
    let mut output = String::from(
        "Usage: msp cli [--output-toon] <mcp-name> <tool-name> [--<parameter> <value>]\n\n",
    );

    if toolsets.is_empty() {
        output.push_str("MCP servers:\n  none  No cached MCP servers available yet.\n");
        return output;
    }

    output.push_str("MCP servers:\n");
    for toolset in toolsets {
        let summary = normalize_description(&toolset.summary);
        output.push_str(&format!(
            "  {:width$}  {}\n",
            toolset.name,
            summary,
            width = width
        ));
    }
    output.push_str("\nRun `msp cli <mcp-name>` to list that server's tools.\n");
    output
}

fn render_toolset_help(toolset: &CachedToolsetRecord) -> String {
    let width = toolset
        .tools
        .iter()
        .map(|tool| tool.name.chars().count())
        .max()
        .unwrap_or(0);
    let mut output = format!(
        "{}\n\nUsage: {} {} <tool-name> [--<parameter> <value>]\n\nTools:\n",
        normalize_description(&toolset.summary),
        CLI_USAGE_HELP_PREFIX,
        toolset.name
    );

    if toolset.tools.is_empty() {
        output.push_str("  none  No cached tools available.\n");
        return output;
    }

    for tool in &toolset.tools {
        let description = tool_description_preview(tool);
        if description.is_empty() {
            output.push_str(&format!("  {}\n", tool.name));
        } else {
            output.push_str(&format!(
                "  {:width$}  {}\n",
                tool.name,
                description,
                width = width
            ));
        }
    }
    output.push_str("\nRun `msp cli ");
    output.push_str(&toolset.name);
    output.push_str(" <tool-name> -h` to inspect one tool.\n");
    output
}

fn tool_description_preview(tool: &ToolSnapshot) -> String {
    truncate_description(
        tool.description.as_deref().unwrap_or_default(),
        TOOL_DESCRIPTION_PREVIEW_CHARS,
    )
}

fn truncate_description(description: &str, max_chars: usize) -> String {
    let normalized = normalize_description(description);
    let char_count = normalized.chars().count();
    if char_count <= max_chars {
        return normalized;
    }

    let preview_len = max_chars.saturating_sub(3);
    let mut preview = normalized.chars().take(preview_len).collect::<String>();
    preview.push_str("...");
    preview
}

fn normalize_description(description: &str) -> String {
    description.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn resolve_toolset<'a>(
    toolsets: &'a [CachedToolsetRecord],
    requested_name: &str,
) -> Result<&'a CachedToolsetRecord, Box<dyn Error>> {
    if let Some(toolset) = toolsets
        .iter()
        .find(|toolset| toolset.name == requested_name)
    {
        return Ok(toolset);
    }

    let sanitized_name = sanitize_name(requested_name);
    toolsets
        .iter()
        .find(|toolset| toolset.name == sanitized_name)
        .ok_or_else(|| {
            let available = toolsets
                .iter()
                .map(|toolset| toolset.name.as_str())
                .collect::<Vec<_>>()
                .join(", ");
            format!("unknown MCP server `{requested_name}`. Available MCP servers: {available}")
                .into()
        })
}

fn resolve_tool<'a>(
    toolset: &'a CachedToolsetRecord,
    requested_name: &str,
) -> Result<&'a ToolSnapshot, Box<dyn Error>> {
    toolset
        .tools
        .iter()
        .find(|tool| tool.name == requested_name)
        .ok_or_else(|| {
            let available = toolset
                .tools
                .iter()
                .map(|tool| tool.name.as_str())
                .collect::<Vec<_>>()
                .join(", ");
            format!(
                "unknown tool `{requested_name}` in MCP server `{}`. Available tools: {available}",
                toolset.name
            )
            .into()
        })
}

fn format_tool_result(
    result: &CallToolResult,
    output_toon: bool,
) -> Result<String, Box<dyn Error>> {
    if output_toon && let Some(structured_content) = result.structured_content.as_ref() {
        let toon = encode_json_to_toon(structured_content)
            .map_err(|error| format!("failed to encode structured tool result as TOON: {error}"))?;
        return Ok(format!("{toon}\n"));
    }

    let payload = serde_json::to_value(result)
        .map_err(|error| format!("failed to serialize tool result: {error}"))?;
    let rendered = display_tool_result(&payload);
    Ok(format!("{}\n", serde_json::to_string_pretty(rendered)?))
}

fn display_tool_result(result: &JsonValue) -> &JsonValue {
    result.get("structuredContent").unwrap_or(result)
}

fn parse_tool_arguments(
    toolset: &CachedToolsetRecord,
    tool: &ToolSnapshot,
    args: &[OsString],
) -> Result<JsonMap<String, JsonValue>, Box<dyn Error>> {
    let properties = tool_properties(tool);
    let required = required_properties(tool);
    let raw_values = collect_raw_parameter_values(toolset, tool, args, properties)?;
    let mut arguments = JsonMap::new();

    for (name, values) in &raw_values {
        let schema = properties
            .and_then(|properties| properties.get(name.as_str()))
            .ok_or_else(|| format!("unknown parameter `--{name}` for tool `{}`", tool.name))?;
        arguments.insert(name.clone(), parse_parameter_values(name, schema, values)?);
    }

    let missing = required
        .into_iter()
        .filter(|name| !arguments.contains_key(name))
        .collect::<Vec<_>>();
    if !missing.is_empty() {
        return Err(format!(
            "missing required parameters for `{}`: {}. Use `{} {} {} --help`.",
            tool.name,
            missing.join(", "),
            CLI_USAGE_PREFIX,
            toolset.name,
            tool.name
        )
        .into());
    }

    Ok(arguments)
}

fn collect_raw_parameter_values(
    toolset: &CachedToolsetRecord,
    tool: &ToolSnapshot,
    args: &[OsString],
    properties: Option<&JsonMap<String, JsonValue>>,
) -> Result<BTreeMap<String, Vec<String>>, Box<dyn Error>> {
    let mut values = BTreeMap::<String, Vec<String>>::new();
    let mut index = 0;

    while index < args.len() {
        let current = args[index].to_string_lossy();
        if current == "-h" || current == "--help" {
            index += 1;
            continue;
        }

        if !current.starts_with("--") || current.len() <= 2 {
            return Err(format!(
                "invalid argument `{current}` for tool `{}`. Expected `--<parameter> <value>`.",
                tool.name
            )
            .into());
        }

        let (name, value, consumed_next) = if let Some((name, value)) = current[2..].split_once('=')
        {
            (name.to_owned(), value.to_owned(), false)
        } else {
            let name = current[2..].to_owned();
            let value = args
                .get(index + 1)
                .ok_or_else(|| format!("missing value for parameter `--{name}`"))?
                .to_string_lossy()
                .into_owned();
            (name, value, true)
        };

        if !properties.is_some_and(|properties| properties.contains_key(name.as_str())) {
            return Err(format!(
                "unknown parameter `--{name}` for tool `{}`. Use `{} {} {} --help`.",
                tool.name, CLI_USAGE_PREFIX, toolset.name, tool.name
            )
            .into());
        }

        values.entry(name).or_default().push(value);
        index += if consumed_next { 2 } else { 1 };
    }

    Ok(values)
}

fn render_tool_help(toolset: &CachedToolsetRecord, tool: &ToolSnapshot) -> String {
    let properties = tool_properties(tool);
    let required = required_properties(tool);
    let mut parameters = properties
        .into_iter()
        .flat_map(|properties| properties.iter().enumerate())
        .map(|(position, (name, schema))| ToolParameterHelp {
            name: name.to_owned(),
            value_hint: parameter_value_hint(schema),
            description_lines: parameter_description_lines(schema, required.contains(name)),
            required: required.contains(name),
            position,
        })
        .collect::<Vec<_>>();
    parameters.sort_by_key(|parameter| (!parameter.required, parameter.position));
    let width = parameters
        .iter()
        .map(|parameter| {
            format!("--{} {}", parameter.name, parameter.value_hint)
                .chars()
                .count()
        })
        .max()
        .unwrap_or(0);
    let mut output = String::new();

    if let Some(description) = tool.description.as_deref() {
        output.push_str(&normalize_description(description));
        output.push_str("\n\n");
    }

    let usage_suffix = if parameters.is_empty() {
        String::new()
    } else {
        " [--<parameter> <value>]".to_owned()
    };
    output.push_str(&format!(
        "Usage: {} {} {}{}\n",
        CLI_USAGE_HELP_PREFIX, toolset.name, tool.name, usage_suffix
    ));

    if !parameters.is_empty() {
        output.push_str("\nParameters:\n");
        for parameter in parameters {
            let label = format!("--{} {}", parameter.name, parameter.value_hint);
            let mut description_lines = parameter.description_lines.into_iter();
            let first_line = description_lines
                .next()
                .unwrap_or_else(|| "No description.".to_owned());
            output.push_str(&format!(
                "  {:width$}  {}\n",
                label,
                first_line,
                width = width
            ));
            for line in description_lines {
                output.push_str(&format!("  {:width$}  {}\n", "", line, width = width));
            }
        }
    }

    output
}

fn tool_properties(tool: &ToolSnapshot) -> Option<&JsonMap<String, JsonValue>> {
    tool.input_schema
        .get("properties")
        .and_then(JsonValue::as_object)
}

fn required_properties(tool: &ToolSnapshot) -> BTreeSet<String> {
    tool.input_schema
        .get("required")
        .and_then(JsonValue::as_array)
        .map(|required| {
            required
                .iter()
                .filter_map(JsonValue::as_str)
                .map(ToOwned::to_owned)
                .collect::<BTreeSet<_>>()
        })
        .unwrap_or_default()
}

fn parse_parameter_values(
    name: &str,
    schema: &JsonValue,
    values: &[String],
) -> Result<JsonValue, Box<dyn Error>> {
    if is_array_schema(schema) {
        let item_schema = schema
            .get("items")
            .ok_or_else(|| format!("parameter `--{name}` is missing an array item schema"))?;
        let mut items = Vec::with_capacity(values.len());
        for value in values {
            items.push(parse_single_parameter_value(name, item_schema, value)?);
        }
        return Ok(JsonValue::Array(items));
    }

    if values.len() != 1 {
        return Err(format!("parameter `--{name}` only accepts one value").into());
    }

    parse_single_parameter_value(name, schema, &values[0])
}

fn parse_single_parameter_value(
    name: &str,
    schema: &JsonValue,
    raw: &str,
) -> Result<JsonValue, Box<dyn Error>> {
    if let Some(candidates) = schema_candidates(schema) {
        let mut string_error = None;

        for candidate in candidates {
            match parse_single_parameter_value(name, candidate, raw) {
                Ok(value) => return Ok(value),
                Err(error) if candidate_type(candidate) == Some("string") => {
                    string_error = Some(error);
                }
                Err(_) => {}
            }
        }

        if let Some(error) = string_error {
            return Err(error);
        }

        return Err(format!("invalid value `{raw}` for parameter `--{name}`").into());
    }

    if let Some(enum_values) = schema.get("enum").and_then(JsonValue::as_array) {
        if enum_values.iter().any(|item| item.as_str() == Some(raw)) {
            return Ok(JsonValue::String(raw.to_owned()));
        }

        return Err(format!(
            "invalid value `{raw}` for parameter `--{name}`. Expected one of: {}",
            enum_values
                .iter()
                .filter_map(JsonValue::as_str)
                .collect::<Vec<_>>()
                .join(", ")
        )
        .into());
    }

    match candidate_type(schema) {
        Some("string") | None => Ok(JsonValue::String(raw.to_owned())),
        Some("number") => parse_number_value(name, raw),
        Some("integer") => parse_integer_value(name, raw),
        Some("boolean") => parse_boolean_value(name, raw),
        Some("object") => Ok(serde_json::from_str(raw)
            .map_err(|error| format!("invalid JSON object for parameter `--{name}`: {error}"))?),
        Some("array") => Err(format!(
            "parameter `--{name}` must be provided multiple times instead of as a single JSON array"
        )
        .into()),
        Some(other) => {
            Err(format!("unsupported schema type `{other}` for parameter `--{name}`").into())
        }
    }
}

fn parse_number_value(name: &str, raw: &str) -> Result<JsonValue, Box<dyn Error>> {
    let number = raw
        .parse::<f64>()
        .map_err(|_| format!("invalid number `{raw}` for parameter `--{name}`"))?;
    let number = Number::from_f64(number)
        .ok_or_else(|| format!("invalid number `{raw}` for parameter `--{name}`"))?;
    Ok(JsonValue::Number(number))
}

fn parse_integer_value(name: &str, raw: &str) -> Result<JsonValue, Box<dyn Error>> {
    if let Ok(value) = raw.parse::<i64>() {
        return Ok(JsonValue::Number(Number::from(value)));
    }

    if let Ok(value) = raw.parse::<u64>() {
        return Ok(JsonValue::Number(Number::from(value)));
    }

    Err(format!("invalid integer `{raw}` for parameter `--{name}`").into())
}

fn parse_boolean_value(name: &str, raw: &str) -> Result<JsonValue, Box<dyn Error>> {
    match raw {
        "true" => Ok(JsonValue::Bool(true)),
        "false" => Ok(JsonValue::Bool(false)),
        _ => Err(format!(
            "invalid boolean `{raw}` for parameter `--{name}`. Use `true` or `false`."
        )
        .into()),
    }
}

fn schema_candidates(schema: &JsonValue) -> Option<Vec<&JsonValue>> {
    let mut candidates = schema
        .get("anyOf")
        .or_else(|| schema.get("oneOf"))
        .and_then(JsonValue::as_array)?
        .iter()
        .filter(|candidate| candidate_type(candidate) != Some("null"))
        .collect::<Vec<_>>();
    candidates.sort_by_key(candidate_priority);
    Some(candidates)
}

fn candidate_priority(schema: &&JsonValue) -> usize {
    match candidate_type(schema) {
        Some("boolean") => 0,
        Some("integer") => 1,
        Some("number") => 2,
        Some("object") => 3,
        Some("array") => 4,
        Some("string") => 5,
        Some(_) | None => 6,
    }
}

fn candidate_type(schema: &JsonValue) -> Option<&str> {
    schema.get("type").and_then(JsonValue::as_str)
}

fn is_array_schema(schema: &JsonValue) -> bool {
    if candidate_type(schema) == Some("array") {
        return true;
    }

    schema_candidates(schema)
        .map(|candidates| candidates.into_iter().any(is_array_schema))
        .unwrap_or(false)
}

fn parameter_value_hint(schema: &JsonValue) -> String {
    if is_array_schema(schema) {
        let item_schema = schema
            .get("items")
            .or_else(|| {
                schema_candidates(schema)
                    .and_then(|candidates| {
                        candidates
                            .into_iter()
                            .find(|candidate| candidate_type(candidate) == Some("array"))
                    })
                    .and_then(|array_schema| array_schema.get("items"))
            })
            .unwrap_or(&JsonValue::Null);
        return format!("<{}>...", scalar_value_hint(item_schema));
    }

    format!("<{}>", scalar_value_hint(schema))
}

fn scalar_value_hint(schema: &JsonValue) -> &'static str {
    if let Some(candidates) = schema_candidates(schema) {
        for candidate in candidates {
            let hint = scalar_value_hint(candidate);
            if hint != "VALUE" {
                return hint;
            }
        }
        return "VALUE";
    }

    match candidate_type(schema) {
        Some("string") => "STRING",
        Some("number") => "NUMBER",
        Some("integer") => "INTEGER",
        Some("boolean") => "BOOLEAN",
        Some("object") => "JSON",
        Some("array") => "VALUE",
        Some(_) | None => "VALUE",
    }
}

fn parameter_description_lines(schema: &JsonValue, required: bool) -> Vec<String> {
    let mut description = schema
        .get("description")
        .and_then(JsonValue::as_str)
        .map(normalize_description)
        .unwrap_or_else(|| "No description.".to_owned());

    if required {
        description.push_str(" [required]");
    }

    if let Some(enum_values) = schema.get("enum").and_then(JsonValue::as_array) {
        let values = enum_values
            .iter()
            .filter_map(JsonValue::as_str)
            .collect::<Vec<_>>();
        if !values.is_empty() {
            description.push_str(&format!(" Allowed values: {}.", values.join(", ")));
        }
    }

    let mut lines = vec![description];
    if let Some(extension_lines) = parameter_description_extension_lines(schema) {
        lines.extend(extension_lines);
    }
    lines
}

fn parameter_description_extension_lines(schema: &JsonValue) -> Option<Vec<String>> {
    if let Some(array_schema) = first_schema_of_type(schema, "array") {
        let item_schema = array_schema.get("items")?;
        let summary = summarize_object_schema(item_schema)?;
        let mut lines = vec![
            "Repeat this parameter with one JSON object per occurrence.".to_owned(),
            format!("Item object shape: {}", summary.shape),
        ];
        if let Some(required_keys) = summary.required_keys_line() {
            lines.push(required_keys);
        }
        lines.extend(summary.property_note_lines());
        return Some(lines);
    }

    let summary = summarize_object_schema(schema)?;
    let mut lines = vec![format!("JSON object shape: {}", summary.shape)];
    if let Some(required_keys) = summary.required_keys_line() {
        lines.push(required_keys);
    }
    lines.extend(summary.property_note_lines());
    Some(lines)
}

fn summarize_object_schema(schema: &JsonValue) -> Option<ObjectSchemaSummary> {
    let object_schema = first_schema_of_type(schema, "object")?;
    let properties = object_schema
        .get("properties")
        .and_then(JsonValue::as_object)?;
    if properties.is_empty() {
        return None;
    }

    let required = object_schema
        .get("required")
        .and_then(JsonValue::as_array)
        .map(|required| {
            required
                .iter()
                .filter_map(JsonValue::as_str)
                .map(ToOwned::to_owned)
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();

    let shape_fields = properties
        .iter()
        .map(|(name, property_schema)| {
            let optional_suffix = if required.iter().any(|item| item == name) {
                ""
            } else {
                "?"
            };
            format!(
                "{name}{optional_suffix}: {}",
                schema_value_placeholder(property_schema)
            )
        })
        .collect::<Vec<_>>();

    let property_notes = properties
        .iter()
        .filter_map(|(name, property_schema)| {
            property_schema
                .get("description")
                .and_then(JsonValue::as_str)
                .map(normalize_description)
                .filter(|description| !description.is_empty())
                .map(|description| format!("{name}: {description}"))
        })
        .collect::<Vec<_>>();

    Some(ObjectSchemaSummary {
        shape: format!("{{ {} }}.", shape_fields.join(", ")),
        required,
        property_notes,
    })
}

fn schema_value_placeholder(schema: &JsonValue) -> String {
    let placeholders = schema_value_placeholders(schema);
    if placeholders.is_empty() {
        "VALUE".to_owned()
    } else {
        placeholders.join(" | ")
    }
}

fn schema_value_placeholders(schema: &JsonValue) -> Vec<String> {
    if let Some(candidates) = schema_variants(schema) {
        let mut placeholders = Vec::new();
        for candidate in candidates {
            for placeholder in schema_value_placeholders(candidate) {
                if !placeholders.contains(&placeholder) {
                    placeholders.push(placeholder);
                }
            }
        }
        return placeholders;
    }

    match candidate_type(schema) {
        Some("string") => vec!["\"STRING\"".to_owned()],
        Some("integer") => vec!["123".to_owned()],
        Some("number") => vec!["1.23".to_owned()],
        Some("boolean") => vec!["true".to_owned()],
        Some("object") => vec!["{...}".to_owned()],
        Some("array") => {
            let item_placeholder = schema
                .get("items")
                .map(schema_value_placeholder)
                .unwrap_or_else(|| "VALUE".to_owned());
            vec![format!("[{item_placeholder}]")]
        }
        Some("null") => vec!["null".to_owned()],
        Some(_) | None => vec!["VALUE".to_owned()],
    }
}

fn schema_variants(schema: &JsonValue) -> Option<Vec<&JsonValue>> {
    schema
        .get("anyOf")
        .or_else(|| schema.get("oneOf"))
        .and_then(JsonValue::as_array)
        .map(|candidates| candidates.iter().collect())
}

fn first_schema_of_type<'a>(schema: &'a JsonValue, expected_type: &str) -> Option<&'a JsonValue> {
    if candidate_type(schema) == Some(expected_type) {
        return Some(schema);
    }

    schema_variants(schema)?
        .into_iter()
        .find(|candidate| first_schema_of_type(candidate, expected_type).is_some())
        .and_then(|candidate| first_schema_of_type(candidate, expected_type))
}

struct ObjectSchemaSummary {
    shape: String,
    required: Vec<String>,
    property_notes: Vec<String>,
}

impl ObjectSchemaSummary {
    fn required_keys_line(&self) -> Option<String> {
        (!self.required.is_empty()).then(|| format!("Required keys: {}.", self.required.join(", ")))
    }

    fn property_note_lines(&self) -> Vec<String> {
        self.property_notes
            .iter()
            .map(|note| format!("  {note}"))
            .collect()
    }
}

struct ToolParameterHelp {
    name: String,
    value_hint: String,
    description_lines: Vec<String>,
    required: bool,
    position: usize,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn sample_toolsets() -> Vec<CachedToolsetRecord> {
        vec![
            CachedToolsetRecord {
                name: "alpha".to_string(),
                summary: "Use Alpha workflows.".to_string(),
                tools: vec![ToolSnapshot {
                    name: "search".to_string(),
                    title: Some("Search".to_string()),
                    description: Some("Search Alpha resources".to_string()),
                    input_schema: json!({
                        "type": "object",
                        "required": ["query", "members"],
                        "properties": {
                            "query": {
                                "type": "string",
                                "description": "Query text"
                            },
                            "members": {
                                "type": "array",
                                "items": { "type": "string" },
                                "description": "Member IDs"
                            }
                        }
                    }),
                    output_schema: None,
                    annotations: None,
                    execution: None,
                    icons: None,
                    meta: None,
                }],
            },
            CachedToolsetRecord {
                name: "beta".to_string(),
                summary: "Use Beta workflows.".to_string(),
                tools: vec![],
            },
        ]
    }

    #[test]
    fn classifies_root_help_without_arguments() {
        assert!(matches!(classify_command(&[]), CliCommand::RootHelp));
    }

    #[test]
    fn classifies_toolset_help_for_one_mcp_name() {
        match classify_command(&[OsString::from("alpha")]) {
            CliCommand::ToolsetHelp { toolset_name } => assert_eq!(toolset_name, "alpha"),
            _ => panic!("expected toolset help"),
        }
    }

    #[test]
    fn classifies_tool_help_with_flag() {
        match classify_command(&[
            OsString::from("alpha"),
            OsString::from("search"),
            OsString::from("-h"),
        ]) {
            CliCommand::ToolHelp {
                toolset_name,
                tool_name,
            } => {
                assert_eq!(toolset_name, "alpha");
                assert_eq!(tool_name, "search");
            }
            _ => panic!("expected tool help"),
        }
    }

    #[test]
    fn classifies_tool_call_with_arguments() {
        match classify_command(&[
            OsString::from("alpha"),
            OsString::from("search"),
            OsString::from("--query"),
            OsString::from("rust"),
        ]) {
            CliCommand::ToolCall {
                toolset_name,
                tool_name,
                arguments,
            } => {
                assert_eq!(toolset_name, "alpha");
                assert_eq!(tool_name, "search");
                assert_eq!(
                    arguments,
                    vec![OsString::from("--query"), OsString::from("rust")]
                );
            }
            _ => panic!("expected tool call"),
        }
    }

    #[test]
    fn renders_root_help_with_mcp_summaries() {
        let help = render_root_help(&sample_toolsets());

        assert!(help.contains("MCP servers:"));
        assert!(help.contains("alpha  Use Alpha workflows."));
        assert!(help.contains("beta   Use Beta workflows."));
    }

    #[test]
    fn renders_toolset_help_with_truncated_tool_descriptions() {
        let mut toolsets = sample_toolsets();
        toolsets[0].tools[0].description = Some(
            "12345678901234567890123456789012345678901234567890123456789012345678901234567890extra"
                .to_string(),
        );

        let help = render_toolset_help(&toolsets[0]);

        assert!(help.contains("Tools:"));
        assert!(help.contains(
            "search  12345678901234567890123456789012345678901234567890123456789012345678901234567..."
        ));
    }

    #[test]
    fn resolves_toolset_by_sanitized_name() {
        let toolsets = sample_toolsets();

        let toolset = resolve_toolset(&toolsets, "Alpha").unwrap();

        assert_eq!(toolset.name, "alpha");
    }

    #[test]
    fn parses_tool_arguments_with_required_array_parameter() {
        let toolsets = sample_toolsets();
        let arguments = parse_tool_arguments(
            &toolsets[0],
            &toolsets[0].tools[0],
            &[
                OsString::from("--query"),
                OsString::from("rust"),
                OsString::from("--members"),
                OsString::from("u1"),
                OsString::from("--members"),
                OsString::from("u2"),
            ],
        )
        .unwrap();

        assert_eq!(arguments.get("query"), Some(&json!("rust")));
        assert_eq!(arguments.get("members"), Some(&json!(["u1", "u2"])));
    }

    #[test]
    fn renders_tool_help_with_usage_and_required_marker() {
        let toolsets = sample_toolsets();
        let help = render_tool_help(&toolsets[0], &toolsets[0].tools[0]);

        assert!(
            help.contains("Usage: msp cli [--output-toon] alpha search [--<parameter> <value>]")
        );
        assert!(help.contains("--query <STRING>"));
        assert!(help.contains("[required]"));
    }

    #[test]
    fn prefers_structured_content_when_formatting_results() {
        let result: CallToolResult = serde_json::from_value(json!({
            "content": [
                {
                    "type": "text",
                    "text": "{\"name\":\"alice\"}"
                }
            ],
            "structuredContent": {
                "name": "alice"
            },
            "isError": false
        }))
        .unwrap();

        let output = format_tool_result(&result, false).unwrap();

        assert_eq!(output, "{\n  \"name\": \"alice\"\n}\n");
    }

    #[test]
    fn formats_structured_content_as_toon_when_requested() {
        let result: CallToolResult = serde_json::from_value(json!({
            "content": [
                {
                    "type": "text",
                    "text": "{\"users\":[{\"id\":1,\"name\":\"Alice\"},{\"id\":2,\"name\":\"Bob\"}]}"
                }
            ],
            "structuredContent": {
                "users": [
                    {"id": 1, "name": "Alice"},
                    {"id": 2, "name": "Bob"}
                ]
            },
            "isError": false
        }))
        .unwrap();

        let output = format_tool_result(&result, true).unwrap();

        assert_eq!(output, "users[2]{id,name}:\n  1,Alice\n  2,Bob\n");
    }
}
