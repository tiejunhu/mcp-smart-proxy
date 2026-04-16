use std::env;
use std::fs;
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{Value as JsonValue, json};
use toml::Table;

use crate::paths::cache_file_path_from_home;
use crate::types::{CachedTools, CachedToolsetRecord, ToolSnapshot};

use super::cache::load_cached_toolsets_from_home;
use super::lua_eval::EVAL_LUA_SCRIPT_NAME;
use super::tools::{
    ACTIVATE_ADDITIONAL_MCPS_NAME, ACTIVATE_TOOLS_IN_ADDITIONAL_MCP_NAME,
    CALL_TOOL_IN_ADDITIONAL_MCP_NAME, STDIO_HOST_REQUIRED_MESSAGE, ToolCatalog,
    build_activate_tool_description, build_activate_tool_detail_result, build_activate_tool_result,
    call_tool_in_additional_mcp_definition, parse_tool_arguments_json, resolve_toolset_name,
};
use super::validate_proxy_stdio_launch;

#[test]
fn builds_tool_description_from_cached_summaries() {
    let toolsets = vec![
        cached_stdio_toolset("alpha", "Use this when you need Alpha workflows.", vec![]),
        cached_stdio_toolset("beta", "Use this for Beta tasks.", vec![]),
    ];

    assert_eq!(
        build_activate_tool_description(&toolsets),
        "Use this tool to activate additional MCP servers. The following additional MCP servers are available to be activated when you need some tools to complete the user request:\n\n- alpha: Use this when you need Alpha workflows.\n- beta: Use this for Beta tasks."
    );
}

#[test]
fn loads_only_toolsets_with_cache_files() {
    let home = unique_test_home("load-cached-toolsets");
    let config: Table = toml::from_str(
        r#"
            [servers.alpha]
            transport = "stdio"
            command = "uvx"
            args = ["alpha-server"]

            [servers.beta]
            transport = "stdio"
            command = "uvx"
            args = ["beta-server"]
        "#,
    )
    .unwrap();

    let alpha_cache = cache_file_path_from_home(&home, "alpha").unwrap();
    fs::create_dir_all(alpha_cache.parent().unwrap()).unwrap();
    fs::write(
        &alpha_cache,
        serde_json::to_string(&CachedTools {
            server: "alpha".to_string(),
            summary: "Use Alpha.".to_string(),
            fetched_at_epoch_ms: 42,
            tools: vec![],
        })
        .unwrap(),
    )
    .unwrap();

    let toolsets = load_cached_toolsets_from_home(&config, &home).unwrap();

    assert_eq!(toolsets.len(), 1);
    assert_eq!(toolsets[0].name, "alpha");
    assert_eq!(toolsets[0].summary, "Use Alpha.");
}

#[test]
fn preserves_cached_annotations_when_loading_cached_toolsets() {
    let home = unique_test_home("sanitize-cached-toolsets");
    let config: Table = toml::from_str(
        r#"
            [servers.alpha]
            transport = "stdio"
            command = "uvx"
            args = ["alpha-server"]
        "#,
    )
    .unwrap();

    let alpha_cache = cache_file_path_from_home(&home, "alpha").unwrap();
    fs::create_dir_all(alpha_cache.parent().unwrap()).unwrap();
    fs::write(
        &alpha_cache,
        serde_json::to_string(&CachedTools {
            server: "alpha".to_string(),
            summary: "Use Alpha.".to_string(),
            fetched_at_epoch_ms: 42,
            tools: vec![ToolSnapshot {
                name: "delete".to_string(),
                title: Some("Delete".to_string()),
                description: Some("Remove a resource".to_string()),
                input_schema: json!({"type":"object"}),
                output_schema: None,
                annotations: Some(json!({
                    "destructiveHint": true,
                    "openWorldHint": true
                })),
                execution: None,
                icons: None,
                meta: None,
            }],
        })
        .unwrap(),
    )
    .unwrap();

    let toolsets = load_cached_toolsets_from_home(&config, &home).unwrap();

    assert_eq!(
        toolsets[0].tools[0].annotations,
        Some(json!({
            "destructiveHint": true,
            "openWorldHint": true
        }))
    );
}

#[test]
fn skips_disabled_toolsets() {
    let home = unique_test_home("load-disabled-toolsets");
    let config: Table = toml::from_str(
        r#"
            [servers.alpha]
            transport = "stdio"
            command = "uvx"
            args = ["alpha-server"]
            enabled = false

            [servers.beta]
            transport = "stdio"
            command = "uvx"
            args = ["beta-server"]
        "#,
    )
    .unwrap();

    let alpha_cache = cache_file_path_from_home(&home, "alpha").unwrap();
    let beta_cache = cache_file_path_from_home(&home, "beta").unwrap();
    fs::create_dir_all(alpha_cache.parent().unwrap()).unwrap();
    fs::write(
        &alpha_cache,
        serde_json::to_string(&CachedTools {
            server: "alpha".to_string(),
            summary: "Use Alpha.".to_string(),
            fetched_at_epoch_ms: 42,
            tools: vec![],
        })
        .unwrap(),
    )
    .unwrap();
    fs::write(
        &beta_cache,
        serde_json::to_string(&CachedTools {
            server: "beta".to_string(),
            summary: "Use Beta.".to_string(),
            fetched_at_epoch_ms: 43,
            tools: vec![],
        })
        .unwrap(),
    )
    .unwrap();

    let toolsets = load_cached_toolsets_from_home(&config, &home).unwrap();

    assert_eq!(toolsets.len(), 1);
    assert_eq!(toolsets[0].name, "beta");
}

#[test]
fn resolves_toolset_by_sanitized_name() {
    let toolsets = vec![cached_stdio_toolset("team-alpha", "Use Alpha.", vec![])];

    let found = resolve_toolset_name(&toolsets, "Team Alpha").unwrap();
    assert_eq!(found.name, "team-alpha");
}

#[test]
fn activate_tool_returns_only_tools() {
    let toolset = cached_stdio_toolset(
        "alpha",
        "Use Alpha.",
        vec![tool_snapshot("search", Some("Search things"))],
    );

    let result = build_activate_tool_result(&[&toolset]);

    assert_eq!(result.structured_content, None);
    assert_eq!(result.content.len(), 1);
    assert_eq!(
        result.content[0].as_text().unwrap().text,
        "[alpha]\nsearch: Search things"
    );
}

#[test]
fn activate_tool_truncates_tool_description_to_80_characters_with_ellipsis() {
    let toolset = cached_stdio_toolset(
        "alpha",
        "Use Alpha.",
        vec![tool_snapshot(
            "search",
            Some(
                "12345678901234567890123456789012345678901234567890123456789012345678901234567890EXTRA",
            ),
        )],
    );

    let result = build_activate_tool_result(&[&toolset]);

    assert_eq!(result.structured_content, None);
    assert_eq!(result.content.len(), 1);
    assert_eq!(
        result.content[0].as_text().unwrap().text,
        "[alpha]\nsearch: 12345678901234567890123456789012345678901234567890123456789012345678901234567..."
    );
}

#[test]
fn activate_tool_returns_name_only_when_description_is_missing() {
    let toolset = cached_stdio_toolset("alpha", "Use Alpha.", vec![tool_snapshot("search", None)]);

    let result = build_activate_tool_result(&[&toolset]);

    assert_eq!(result.structured_content, None);
    assert_eq!(result.content.len(), 1);
    assert_eq!(result.content[0].as_text().unwrap().text, "[alpha]\nsearch");
}

#[test]
fn activate_tool_returns_multiple_toolsets_in_request_order() {
    let alpha = cached_stdio_toolset(
        "alpha",
        "Use Alpha.",
        vec![tool_snapshot("search", Some("Search things"))],
    );
    let beta = cached_stdio_toolset(
        "beta",
        "Use Beta.",
        vec![tool_snapshot("list", Some("List things"))],
    );

    let result = build_activate_tool_result(&[&beta, &alpha]);

    assert_eq!(result.structured_content, None);
    assert_eq!(result.content.len(), 1);
    assert_eq!(
        result.content[0].as_text().unwrap().text,
        "[beta]\nlist: List things\n\n[alpha]\nsearch: Search things"
    );
}

#[test]
fn activate_tool_detail_returns_full_tool_definitions() {
    let search = tool_snapshot("search", Some("Search things"));
    let list = tool_snapshot("list", Some("List things"));

    let result = build_activate_tool_detail_result(&[&search, &list]);

    assert_eq!(
        result.structured_content,
        Some(json!({
            "tools": [
                {
                    "name": "search",
                    "title": "Search",
                    "description": "Search things",
                    "input_schema": {
                        "type": "object"
                    },
                    "output_schema": null,
                    "annotations": null,
                    "execution": null,
                    "icons": null,
                    "meta": null
                },
                {
                    "name": "list",
                    "title": "Search",
                    "description": "List things",
                    "input_schema": {
                        "type": "object"
                    },
                    "output_schema": null,
                    "annotations": null,
                    "execution": null,
                    "icons": null,
                    "meta": null
                }
            ]
        }))
    );
}

#[test]
fn parses_object_arguments_json() {
    let parsed = parse_tool_arguments_json(r#"{"query":"hello"}"#).unwrap();

    assert_eq!(
        parsed,
        Some(json!({ "query": "hello" }).as_object().unwrap().clone())
    );
}

#[test]
fn parses_null_arguments_json() {
    let parsed = parse_tool_arguments_json("null").unwrap();

    assert_eq!(parsed, None);
}

#[test]
fn rejects_non_object_arguments_json() {
    let error = parse_tool_arguments_json(r#"["hello"]"#).unwrap_err();

    assert_eq!(
        error.message,
        "`args_in_json` must decode to a JSON object or null"
    );
}

#[test]
fn call_tool_definition_contains_expected_fields() {
    let tool = call_tool_in_additional_mcp_definition(CALL_TOOL_IN_ADDITIONAL_MCP_NAME);
    let properties = tool
        .input_schema
        .get("properties")
        .and_then(JsonValue::as_object)
        .unwrap();

    assert!(properties.contains_key("external_mcp_name"));
    assert!(properties.contains_key("tool_name"));
    assert!(properties.contains_key("args_in_json"));
}

#[test]
fn activate_additional_mcps_definition_contains_expected_fields() {
    let catalog = ToolCatalog::new(&[]);
    let tool = catalog.get(ACTIVATE_ADDITIONAL_MCPS_NAME).unwrap();
    let properties = tool
        .input_schema
        .get("properties")
        .and_then(JsonValue::as_object)
        .unwrap();
    let required = tool
        .input_schema
        .get("required")
        .and_then(JsonValue::as_array)
        .unwrap();

    assert!(properties.contains_key("external_mcp_names"));
    assert_eq!(required, &vec![json!("external_mcp_names")]);
}

#[test]
fn activate_tools_in_additional_mcp_definition_contains_expected_fields() {
    let catalog = ToolCatalog::new(&[]);
    let tool = catalog.get(ACTIVATE_TOOLS_IN_ADDITIONAL_MCP_NAME).unwrap();
    let properties = tool
        .input_schema
        .get("properties")
        .and_then(JsonValue::as_object)
        .unwrap();
    let required = tool
        .input_schema
        .get("required")
        .and_then(JsonValue::as_array)
        .unwrap();

    assert!(properties.contains_key("external_mcp_name"));
    assert!(properties.contains_key("tool_names"));
    assert_eq!(
        required,
        &vec![json!("external_mcp_name"), json!("tool_names")]
    );
}

#[test]
fn eval_lua_script_definition_contains_expected_fields() {
    let catalog = ToolCatalog::new(&[]);
    let tool = catalog.get(EVAL_LUA_SCRIPT_NAME).unwrap();
    let properties = tool
        .input_schema
        .get("properties")
        .and_then(JsonValue::as_object)
        .unwrap();
    let required = tool
        .input_schema
        .get("required")
        .and_then(JsonValue::as_array)
        .unwrap();

    assert!(properties.contains_key("script"));
    assert!(properties.contains_key("globals"));
    assert_eq!(required, &vec![json!("script")]);
}

#[test]
fn proxy_tools_set_explicit_annotation_hints() {
    let catalog = ToolCatalog::new(&[]);
    let activate_tool = catalog.get(ACTIVATE_ADDITIONAL_MCPS_NAME).unwrap();
    let activate_tool_detail = catalog.get(ACTIVATE_TOOLS_IN_ADDITIONAL_MCP_NAME).unwrap();
    let call_tool = catalog.get(CALL_TOOL_IN_ADDITIONAL_MCP_NAME).unwrap();
    let eval_lua_script = catalog.get(EVAL_LUA_SCRIPT_NAME).unwrap();

    assert_eq!(
        activate_tool.annotations,
        Some(
            rmcp::model::ToolAnnotations::new()
                .read_only(true)
                .destructive(false)
        )
    );
    assert_eq!(
        activate_tool_detail.annotations,
        Some(
            rmcp::model::ToolAnnotations::new()
                .read_only(true)
                .destructive(false)
        )
    );
    assert_eq!(
        call_tool.annotations,
        Some(
            rmcp::model::ToolAnnotations::new()
                .read_only(false)
                .destructive(false)
        )
    );
    assert_eq!(
        eval_lua_script.annotations,
        Some(
            rmcp::model::ToolAnnotations::new()
                .read_only(false)
                .destructive(false)
        )
    );
}

#[test]
fn rejects_running_proxy_stdio_server_directly_in_terminal() {
    let error = validate_proxy_stdio_launch(true, true).unwrap_err();

    assert_eq!(
        error.to_string(),
        format!("mcp.serve.stdio_host: {STDIO_HOST_REQUIRED_MESSAGE}")
    );
}

#[test]
fn allows_running_proxy_stdio_server_when_connected_to_a_host() {
    validate_proxy_stdio_launch(false, false).unwrap();
}

#[test]
fn rejects_running_proxy_stdio_server_when_stdin_is_terminal() {
    let error = validate_proxy_stdio_launch(true, false).unwrap_err();

    assert_eq!(
        error.to_string(),
        format!("mcp.serve.stdio_host: {STDIO_HOST_REQUIRED_MESSAGE}")
    );
}

#[test]
fn rejects_running_proxy_stdio_server_when_stdout_is_terminal() {
    let error = validate_proxy_stdio_launch(false, true).unwrap_err();

    assert_eq!(
        error.to_string(),
        format!("mcp.serve.stdio_host: {STDIO_HOST_REQUIRED_MESSAGE}")
    );
}

fn cached_stdio_toolset(
    name: &str,
    summary: &str,
    tools: Vec<ToolSnapshot>,
) -> CachedToolsetRecord {
    CachedToolsetRecord {
        name: name.to_string(),
        summary: summary.to_string(),
        tools,
    }
}

fn tool_snapshot(name: &str, description: Option<&str>) -> ToolSnapshot {
    ToolSnapshot {
        name: name.to_string(),
        title: Some("Search".to_string()),
        description: description.map(str::to_string),
        input_schema: json!({
            "type": "object"
        }),
        output_schema: None,
        annotations: None,
        execution: None,
        icons: None,
        meta: None,
    }
}

fn unique_test_home(name: &str) -> std::path::PathBuf {
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();

    env::temp_dir().join(format!("mcp-smart-proxy-{unique}-{name}"))
}
