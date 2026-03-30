use std::borrow::Cow;
use std::collections::HashMap;
use std::error::Error;
use std::fs;
use std::sync::Arc;

use futures::{StreamExt, stream::BoxStream};
use reqwest::header::{ACCEPT, AUTHORIZATION, CONTENT_TYPE, HeaderName, HeaderValue};
use rmcp::{
    RoleClient, ServiceExt,
    model::{ClientJsonRpcMessage, ServerJsonRpcMessage},
    service::RunningService,
    transport::{
        StreamableHttpClientTransport,
        auth::AuthError,
        common::http_header::{
            EVENT_STREAM_MIME_TYPE, HEADER_LAST_EVENT_ID, HEADER_SESSION_ID, JSON_MIME_TYPE,
        },
        streamable_http_client::{
            StreamableHttpClient, StreamableHttpClientTransportConfig, StreamableHttpError,
            StreamableHttpPostResponse,
        },
    },
};
use sse_stream::{Sse, SseStream};

use crate::fs_util::acquire_sibling_lock;
use crate::paths::oauth_credentials_path;
use crate::types::ConfiguredServer;

mod callback;
mod headers;
mod oauth;
mod store;

use headers::{
    forbidden_response, is_reserved_header, parse_json_rpc_error, remote_target,
    unauthorized_response,
};
use oauth::RemoteAuth;

type RemoteTransport = StreamableHttpClientTransport<OAuthAwareHttpClient>;

pub async fn connect_remote_client(
    server_name: &str,
    server: &ConfiguredServer,
) -> Result<RunningService<RoleClient, ()>, Box<dyn Error>> {
    let remote = remote_target(server)?;
    let auth = RemoteAuth::new(server_name, &remote.url).await?;
    let client = OAuthAwareHttpClient::new(
        reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .build()?,
        auth,
    );
    let transport = RemoteTransport::with_client(
        client,
        StreamableHttpClientTransportConfig::with_uri(remote.url.clone())
            .custom_headers(remote.headers),
    );

    ().serve(transport).await.map_err(Into::into)
}

pub async fn login_remote_server(
    server_name: &str,
    server: &ConfiguredServer,
) -> Result<(), Box<dyn Error>> {
    let remote = remote_target(server)?;
    let auth = RemoteAuth::new(server_name, &remote.url).await?;
    auth.ensure_authorized(None).await.map_err(Into::into)
}

pub fn logout_remote_server(server_name: &str) -> Result<bool, Box<dyn Error>> {
    let path = oauth_credentials_path(server_name)?;
    let _guard = acquire_sibling_lock(&path).map_err(Box::<dyn Error>::from)?;
    if !path.exists() {
        return Ok(false);
    }

    fs::remove_file(path)?;
    Ok(true)
}

#[derive(Clone)]
struct OAuthAwareHttpClient {
    inner: reqwest::Client,
    auth: RemoteAuth,
}

impl OAuthAwareHttpClient {
    fn new(inner: reqwest::Client, auth: RemoteAuth) -> Self {
        Self { inner, auth }
    }

    async fn current_access_token(
        &self,
    ) -> Result<Option<String>, StreamableHttpError<reqwest::Error>> {
        let manager = self.auth.manager.lock().await;
        match manager.get_access_token().await {
            Ok(token) => Ok(Some(token)),
            Err(AuthError::AuthorizationRequired) => Ok(None),
            Err(error) => Err(StreamableHttpError::Auth(error)),
        }
    }

    async fn maybe_retry_authorization(
        &self,
        error: &StreamableHttpError<reqwest::Error>,
    ) -> Result<bool, StreamableHttpError<reqwest::Error>> {
        match error {
            StreamableHttpError::Auth(AuthError::AuthorizationRequired) => {
                self.auth.ensure_authorized(None).await?;
                Ok(true)
            }
            StreamableHttpError::Auth(AuthError::InsufficientScope { required_scope, .. }) => {
                let scope = (!required_scope.is_empty()).then_some(required_scope.as_str());
                self.auth.ensure_authorized(scope).await?;
                Ok(true)
            }
            _ => Ok(false),
        }
    }

    fn attach_auth_header(
        mut request: reqwest::RequestBuilder,
        auth_token: Option<String>,
        custom_headers: &HashMap<HeaderName, HeaderValue>,
    ) -> reqwest::RequestBuilder {
        let has_authorization = custom_headers
            .keys()
            .any(|name| name.as_str().eq_ignore_ascii_case(AUTHORIZATION.as_str()));
        if !has_authorization && let Some(auth_header) = auth_token {
            request = request.bearer_auth(auth_header);
        }
        request
    }

    fn apply_custom_headers(
        mut request: reqwest::RequestBuilder,
        custom_headers: HashMap<HeaderName, HeaderValue>,
    ) -> Result<reqwest::RequestBuilder, StreamableHttpError<reqwest::Error>> {
        for (name, value) in custom_headers {
            if is_reserved_header(&name) {
                return Err(StreamableHttpError::ReservedHeaderConflict(
                    name.to_string(),
                ));
            }
            request = request.header(name, value);
        }
        Ok(request)
    }

    async fn get_stream_once(
        &self,
        uri: Arc<str>,
        session_id: Arc<str>,
        last_event_id: Option<String>,
        auth_token: Option<String>,
        custom_headers: HashMap<HeaderName, HeaderValue>,
    ) -> Result<
        BoxStream<'static, Result<Sse, sse_stream::Error>>,
        StreamableHttpError<reqwest::Error>,
    > {
        let mut request = self
            .inner
            .get(uri.as_ref())
            .header(ACCEPT, [EVENT_STREAM_MIME_TYPE, JSON_MIME_TYPE].join(", "))
            .header(HEADER_SESSION_ID, session_id.as_ref());
        if let Some(last_event_id) = last_event_id {
            request = request.header(HEADER_LAST_EVENT_ID, last_event_id);
        }
        request = Self::attach_auth_header(request, auth_token, &custom_headers);
        request = Self::apply_custom_headers(request, custom_headers)?;
        let response = request.send().await?;
        if response.status() == reqwest::StatusCode::UNAUTHORIZED {
            return unauthorized_response(response);
        }
        if response.status() == reqwest::StatusCode::FORBIDDEN {
            return forbidden_response(response);
        }
        if response.status() == reqwest::StatusCode::METHOD_NOT_ALLOWED {
            return Err(StreamableHttpError::ServerDoesNotSupportSse);
        }
        let response = response.error_for_status()?;
        match response.headers().get(CONTENT_TYPE) {
            Some(ct)
                if ct.as_bytes().starts_with(EVENT_STREAM_MIME_TYPE.as_bytes())
                    || ct.as_bytes().starts_with(JSON_MIME_TYPE.as_bytes()) => {}
            Some(ct) => {
                return Err(StreamableHttpError::UnexpectedContentType(Some(
                    String::from_utf8_lossy(ct.as_bytes()).to_string(),
                )));
            }
            None => return Err(StreamableHttpError::UnexpectedContentType(None)),
        }
        Ok(SseStream::from_byte_stream(response.bytes_stream()).boxed())
    }

    async fn delete_session_once(
        &self,
        uri: Arc<str>,
        session_id: Arc<str>,
        auth_token: Option<String>,
        custom_headers: HashMap<HeaderName, HeaderValue>,
    ) -> Result<(), StreamableHttpError<reqwest::Error>> {
        let mut request = self
            .inner
            .delete(uri.as_ref())
            .header(HEADER_SESSION_ID, session_id.as_ref());
        request = Self::attach_auth_header(request, auth_token, &custom_headers);
        request = Self::apply_custom_headers(request, custom_headers)?;
        let response = request.send().await?;
        if response.status() == reqwest::StatusCode::UNAUTHORIZED {
            return unauthorized_response(response);
        }
        if response.status() == reqwest::StatusCode::FORBIDDEN {
            return forbidden_response(response);
        }
        if response.status() == reqwest::StatusCode::METHOD_NOT_ALLOWED {
            return Ok(());
        }
        response.error_for_status()?;
        Ok(())
    }

    async fn post_message_once(
        &self,
        uri: Arc<str>,
        message: ClientJsonRpcMessage,
        session_id: Option<Arc<str>>,
        auth_token: Option<String>,
        custom_headers: HashMap<HeaderName, HeaderValue>,
    ) -> Result<StreamableHttpPostResponse, StreamableHttpError<reqwest::Error>> {
        let mut request = self
            .inner
            .post(uri.as_ref())
            .header(ACCEPT, [EVENT_STREAM_MIME_TYPE, JSON_MIME_TYPE].join(", "));
        request = Self::attach_auth_header(request, auth_token, &custom_headers);
        request = Self::apply_custom_headers(request, custom_headers)?;
        let session_was_attached = session_id.is_some();
        if let Some(session_id) = session_id {
            request = request.header(HEADER_SESSION_ID, session_id.as_ref());
        }
        let response = request.json(&message).send().await?;
        if response.status() == reqwest::StatusCode::UNAUTHORIZED {
            return unauthorized_response(response);
        }
        if response.status() == reqwest::StatusCode::FORBIDDEN {
            return forbidden_response(response);
        }
        let status = response.status();
        if matches!(
            status,
            reqwest::StatusCode::ACCEPTED | reqwest::StatusCode::NO_CONTENT
        ) {
            return Ok(StreamableHttpPostResponse::Accepted);
        }
        if status == reqwest::StatusCode::NOT_FOUND && session_was_attached {
            return Err(StreamableHttpError::SessionExpired);
        }
        let content_type = response
            .headers()
            .get(CONTENT_TYPE)
            .map(|ct| String::from_utf8_lossy(ct.as_bytes()).to_string());
        let session_id = response
            .headers()
            .get(HEADER_SESSION_ID)
            .and_then(|value| value.to_str().ok())
            .map(|value| value.to_string());
        if !status.is_success() {
            let body = response
                .text()
                .await
                .unwrap_or_else(|_| "<failed to read response body>".to_string());
            if content_type
                .as_deref()
                .is_some_and(|ct| ct.as_bytes().starts_with(JSON_MIME_TYPE.as_bytes()))
                && let Some(message) = parse_json_rpc_error(&body)
            {
                return Ok(StreamableHttpPostResponse::Json(message, session_id));
            }
            return Err(StreamableHttpError::UnexpectedServerResponse(Cow::Owned(
                format!("HTTP {status}: {body}"),
            )));
        }
        match content_type.as_deref() {
            Some(ct) if ct.as_bytes().starts_with(EVENT_STREAM_MIME_TYPE.as_bytes()) => {
                Ok(StreamableHttpPostResponse::Sse(
                    SseStream::from_byte_stream(response.bytes_stream()).boxed(),
                    session_id,
                ))
            }
            Some(ct) if ct.as_bytes().starts_with(JSON_MIME_TYPE.as_bytes()) => {
                match response.json::<ServerJsonRpcMessage>().await {
                    Ok(message) => Ok(StreamableHttpPostResponse::Json(message, session_id)),
                    Err(_) => Ok(StreamableHttpPostResponse::Accepted),
                }
            }
            _ => Err(StreamableHttpError::UnexpectedContentType(content_type)),
        }
    }
}

impl StreamableHttpClient for OAuthAwareHttpClient {
    type Error = reqwest::Error;

    async fn post_message(
        &self,
        uri: Arc<str>,
        message: ClientJsonRpcMessage,
        session_id: Option<Arc<str>>,
        _auth_header: Option<String>,
        custom_headers: HashMap<HeaderName, HeaderValue>,
    ) -> Result<StreamableHttpPostResponse, StreamableHttpError<Self::Error>> {
        let auth_token = self.current_access_token().await?;
        let retry_headers = custom_headers.clone();
        let retry_message = message.clone();
        let retry_session = session_id.clone();

        match self
            .post_message_once(uri.clone(), message, session_id, auth_token, custom_headers)
            .await
        {
            Ok(response) => Ok(response),
            Err(error) if self.maybe_retry_authorization(&error).await? => {
                let auth_token = self.current_access_token().await?;
                self.post_message_once(uri, retry_message, retry_session, auth_token, retry_headers)
                    .await
            }
            Err(error) => Err(error),
        }
    }

    async fn delete_session(
        &self,
        uri: Arc<str>,
        session_id: Arc<str>,
        _auth_header: Option<String>,
        custom_headers: HashMap<HeaderName, HeaderValue>,
    ) -> Result<(), StreamableHttpError<Self::Error>> {
        let auth_token = self.current_access_token().await?;
        let retry_headers = custom_headers.clone();
        let retry_session = session_id.clone();

        match self
            .delete_session_once(uri.clone(), session_id, auth_token, custom_headers)
            .await
        {
            Ok(()) => Ok(()),
            Err(error) if self.maybe_retry_authorization(&error).await? => {
                let auth_token = self.current_access_token().await?;
                self.delete_session_once(uri, retry_session, auth_token, retry_headers)
                    .await
            }
            Err(error) => Err(error),
        }
    }

    async fn get_stream(
        &self,
        uri: Arc<str>,
        session_id: Arc<str>,
        last_event_id: Option<String>,
        _auth_header: Option<String>,
        custom_headers: HashMap<HeaderName, HeaderValue>,
    ) -> Result<BoxStream<'static, Result<Sse, sse_stream::Error>>, StreamableHttpError<Self::Error>>
    {
        let auth_token = self.current_access_token().await?;
        let retry_headers = custom_headers.clone();
        let retry_session = session_id.clone();
        let retry_last_event_id = last_event_id.clone();

        match self
            .get_stream_once(
                uri.clone(),
                session_id,
                last_event_id,
                auth_token,
                custom_headers,
            )
            .await
        {
            Ok(stream) => Ok(stream),
            Err(error) if self.maybe_retry_authorization(&error).await? => {
                let auth_token = self.current_access_token().await?;
                self.get_stream_once(
                    uri,
                    retry_session,
                    retry_last_event_id,
                    auth_token,
                    retry_headers,
                )
                .await
            }
            Err(error) => Err(error),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::{BTreeMap, HashMap};

    use crate::remote::headers::resolve_remote_headers;
    use rmcp::transport::common::http_header::HEADER_MCP_PROTOCOL_VERSION;

    #[test]
    fn runtime_headers_allow_negotiated_protocol_version() {
        let client = reqwest::Client::new();
        let request = client.get("http://example.com");
        let headers = HashMap::from([(
            HeaderName::from_static("mcp-protocol-version"),
            HeaderValue::from_static("2025-06-18"),
        )]);

        let request = OAuthAwareHttpClient::apply_custom_headers(request, headers)
            .expect("negotiated protocol header should be allowed")
            .build()
            .expect("request should build");

        assert_eq!(
            request.headers().get(HEADER_MCP_PROTOCOL_VERSION),
            Some(&HeaderValue::from_static("2025-06-18"))
        );
    }

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
