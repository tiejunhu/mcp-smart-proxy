use std::borrow::Cow;
use std::collections::{BTreeMap, HashMap};
use std::error::Error;

use reqwest::{
    self,
    header::{ACCEPT, HeaderName, HeaderValue, WWW_AUTHENTICATE},
};
use rmcp::{
    model::{JsonRpcMessage, ServerJsonRpcMessage},
    transport::{
        auth::AuthError,
        common::http_header::{
            HEADER_LAST_EVENT_ID, HEADER_MCP_PROTOCOL_VERSION, HEADER_SESSION_ID,
        },
        streamable_http_client::StreamableHttpError,
    },
};

use crate::env_template::render_env_placeholders;
use crate::types::{ConfiguredServer, ConfiguredTransport};

#[derive(Clone)]
pub(crate) struct RemoteTarget {
    pub(crate) url: String,
    pub(crate) headers: HashMap<HeaderName, HeaderValue>,
}

pub(crate) fn remote_target(server: &ConfiguredServer) -> Result<RemoteTarget, Box<dyn Error>> {
    let ConfiguredTransport::Remote { url, headers } = &server.transport else {
        return Err("configured server is not a remote transport".into());
    };

    Ok(RemoteTarget {
        url: url.clone(),
        headers: resolve_remote_headers(headers, &server.resolved_env_map())?,
    })
}

pub(crate) fn resolve_remote_headers(
    headers: &BTreeMap<String, String>,
    env_values: &BTreeMap<String, std::ffi::OsString>,
) -> Result<HashMap<HeaderName, HeaderValue>, Box<dyn Error>> {
    let mut resolved = HashMap::new();

    for (name, value) in headers {
        let header_name = HeaderName::from_bytes(name.as_bytes())
            .map_err(|error| format!("invalid remote header name `{name}`: {error}"))?;
        if is_config_reserved_header(&header_name) {
            return Err(
                format!("remote header `{name}` is reserved and cannot be configured").into(),
            );
        }
        let header_value = resolve_remote_header_value(value, env_values)?;
        let header_value = HeaderValue::from_str(&header_value)
            .map_err(|error| format!("invalid remote header value for `{name}`: {error}"))?;
        resolved.insert(header_name, header_value);
    }

    Ok(resolved)
}

pub(crate) fn unauthorized_response<T>(
    response: reqwest::Response,
) -> Result<T, StreamableHttpError<reqwest::Error>> {
    if let Some(header) = response.headers().get(WWW_AUTHENTICATE) {
        header.to_str().map_err(|_| {
            StreamableHttpError::UnexpectedServerResponse(Cow::from(
                "invalid www-authenticate header value",
            ))
        })?;
        return Err(StreamableHttpError::Auth(AuthError::AuthorizationRequired));
    }
    Err(StreamableHttpError::UnexpectedServerResponse(Cow::from(
        "remote server returned 401 without a www-authenticate header",
    )))
}

pub(crate) fn forbidden_response<T>(
    response: reqwest::Response,
) -> Result<T, StreamableHttpError<reqwest::Error>> {
    if let Some(header) = response.headers().get(WWW_AUTHENTICATE) {
        let header = header
            .to_str()
            .map_err(|_| {
                StreamableHttpError::UnexpectedServerResponse(Cow::from(
                    "invalid www-authenticate header value",
                ))
            })?
            .to_string();
        return Err(StreamableHttpError::Auth(AuthError::InsufficientScope {
            required_scope: extract_scope_from_header(&header).unwrap_or_default(),
            upgrade_url: None,
        }));
    }
    Err(StreamableHttpError::UnexpectedServerResponse(Cow::from(
        "remote server returned 403 without a www-authenticate header",
    )))
}

pub(crate) fn parse_json_rpc_error(body: &str) -> Option<ServerJsonRpcMessage> {
    match serde_json::from_str::<ServerJsonRpcMessage>(body) {
        Ok(message @ JsonRpcMessage::Error(_)) => Some(message),
        _ => None,
    }
}

pub(crate) fn is_reserved_header(name: &HeaderName) -> bool {
    let value = name.as_str();
    value.eq_ignore_ascii_case(ACCEPT.as_str())
        || value.eq_ignore_ascii_case(HEADER_SESSION_ID)
        || value.eq_ignore_ascii_case(HEADER_LAST_EVENT_ID)
}

fn resolve_remote_header_value(
    value: &str,
    env_values: &BTreeMap<String, std::ffi::OsString>,
) -> Result<String, Box<dyn Error>> {
    render_env_placeholders(value, env_values, &mut |name| {
        Err(format!(
            "missing environment variable `{name}` required by remote header"
        ))
    })
    .map_err(Into::into)
}

fn extract_scope_from_header(header: &str) -> Option<String> {
    let header_lowercase = header.to_ascii_lowercase();
    let scope_key = "scope=";
    let position = header_lowercase.find(scope_key)?;
    let start = position + scope_key.len();
    let value = &header[start..];
    if let Some(quoted) = value.strip_prefix('"') {
        let end = quoted.find('"')?;
        return Some(quoted[..end].to_string());
    }
    let end = value
        .find(|character: char| character == ',' || character == ';' || character.is_whitespace())
        .unwrap_or(value.len());
    (end > 0).then(|| value[..end].to_string())
}

fn is_config_reserved_header(name: &HeaderName) -> bool {
    is_reserved_header(name)
        || name
            .as_str()
            .eq_ignore_ascii_case(HEADER_MCP_PROTOCOL_VERSION)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn configured_headers_reject_protocol_version() {
        let headers =
            BTreeMap::from([("mcp-protocol-version".to_string(), "2025-06-18".to_string())]);

        let error = resolve_remote_headers(&headers, &BTreeMap::new())
            .expect_err("configured protocol header should be rejected");

        assert_eq!(
            error.to_string(),
            "remote header `mcp-protocol-version` is reserved and cannot be configured"
        );
    }
}
