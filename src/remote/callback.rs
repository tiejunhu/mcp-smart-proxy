use std::borrow::Cow;
use std::time::Duration;

use reqwest::Error as ReqwestError;
use rmcp::transport::streamable_http_client::StreamableHttpError;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
    time::timeout,
};

const CALLBACK_HOST: &str = "127.0.0.1";
const CALLBACK_PATH: &str = "/oauth/callback";
const AUTH_TIMEOUT: Duration = Duration::from_secs(300);

pub(crate) struct CallbackServer {
    listener: TcpListener,
    redirect_uri: String,
}

impl CallbackServer {
    pub(crate) async fn bind() -> Result<Self, StreamableHttpError<ReqwestError>> {
        let listener = TcpListener::bind((CALLBACK_HOST, 0))
            .await
            .map_err(StreamableHttpError::Io)?;
        let port = listener
            .local_addr()
            .map_err(StreamableHttpError::Io)?
            .port();
        Ok(Self {
            listener,
            redirect_uri: format!("http://{CALLBACK_HOST}:{port}{CALLBACK_PATH}"),
        })
    }

    pub(crate) fn redirect_uri(&self) -> String {
        self.redirect_uri.clone()
    }

    pub(crate) async fn wait_for_callback(
        self,
    ) -> Result<CallbackResult, StreamableHttpError<ReqwestError>> {
        let accepted = timeout(AUTH_TIMEOUT, self.listener.accept())
            .await
            .map_err(|_| {
                StreamableHttpError::UnexpectedServerResponse(Cow::from(
                    "timed out waiting for OAuth callback",
                ))
            })?
            .map_err(StreamableHttpError::Io)?;
        let (mut stream, _) = accepted;
        let mut buffer = vec![0_u8; 8192];
        let read = stream
            .read(&mut buffer)
            .await
            .map_err(StreamableHttpError::Io)?;
        let request = String::from_utf8_lossy(&buffer[..read]).to_string();
        let first_line = request.lines().next().unwrap_or_default();
        let path = first_line
            .split_whitespace()
            .nth(1)
            .ok_or_else(|| {
                StreamableHttpError::UnexpectedServerResponse(Cow::from(
                    "received an invalid OAuth callback request",
                ))
            })?
            .to_string();
        let callback = parse_callback_request(&path)?;
        stream
            .write_all(success_http_response().as_bytes())
            .await
            .map_err(StreamableHttpError::Io)?;
        Ok(callback)
    }
}

pub(crate) struct CallbackResult {
    pub(crate) code: String,
    pub(crate) state: String,
}

fn parse_callback_request(path: &str) -> Result<CallbackResult, StreamableHttpError<ReqwestError>> {
    let parsed = reqwest::Url::parse(&format!("http://localhost{path}")).map_err(|error| {
        StreamableHttpError::UnexpectedServerResponse(Cow::Owned(error.to_string()))
    })?;
    if parsed.path() != CALLBACK_PATH {
        return Err(StreamableHttpError::UnexpectedServerResponse(Cow::Owned(
            format!("unexpected OAuth callback path `{}`", parsed.path()),
        )));
    }

    let mut code = None;
    let mut state = None;
    for (key, value) in parsed.query_pairs() {
        match key.as_ref() {
            "code" => code = Some(value.to_string()),
            "state" => state = Some(value.to_string()),
            _ => {}
        }
    }

    Ok(CallbackResult {
        code: code.ok_or_else(|| {
            StreamableHttpError::UnexpectedServerResponse(Cow::from(
                "OAuth callback did not include `code`",
            ))
        })?,
        state: state.ok_or_else(|| {
            StreamableHttpError::UnexpectedServerResponse(Cow::from(
                "OAuth callback did not include `state`",
            ))
        })?,
    })
}

fn success_http_response() -> &'static str {
    "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nConnection: close\r\n\r\n<html><body><h1>OAuth login complete</h1><p>You can close this window.</p></body></html>"
}
