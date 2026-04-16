use rmcp::model::{CallToolResult, Content};
use serde_json::Value as JsonValue;

pub(crate) fn encode_json_to_toon(value: &JsonValue) -> Result<String, toon_format::ToonError> {
    toon_format::encode_default(value)
}

pub(crate) fn rewrite_call_tool_result_to_toon(
    result: &CallToolResult,
) -> Result<CallToolResult, toon_format::ToonError> {
    let Some(structured_content) = result.structured_content.as_ref() else {
        return Ok(result.clone());
    };

    let toon = encode_json_to_toon(structured_content)?;
    let mut rewritten = CallToolResult::success(vec![Content::text(toon)]);
    rewritten.is_error = result.is_error;
    rewritten.meta = result.meta.clone();
    Ok(rewritten)
}

#[cfg(test)]
mod tests {
    use rmcp::model::{CallToolResult, Content};
    use serde_json::json;

    use super::rewrite_call_tool_result_to_toon;

    #[test]
    fn rewrites_structured_tool_result_to_toon_text() {
        let mut result = CallToolResult::success(vec![Content::text("ignored")]);
        result.structured_content = Some(json!({
            "users": [
                {"id": 1, "name": "Alice"},
                {"id": 2, "name": "Bob"}
            ]
        }));

        let rewritten = rewrite_call_tool_result_to_toon(&result).unwrap();

        assert_eq!(rewritten.structured_content, None);
        assert_eq!(rewritten.is_error, Some(false));
        assert_eq!(rewritten.content.len(), 1);
        assert_eq!(
            rewritten.content[0].as_text().unwrap().text,
            "users[2]{id,name}:\n  1,Alice\n  2,Bob"
        );
    }

    #[test]
    fn leaves_unstructured_tool_result_unchanged() {
        let result = CallToolResult::success(vec![Content::text("plain text")]);

        let rewritten = rewrite_call_tool_result_to_toon(&result).unwrap();

        assert_eq!(rewritten, result);
    }
}
