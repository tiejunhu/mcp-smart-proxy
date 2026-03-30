use std::error::Error;
use std::sync::Arc;

use reqwest::Error as ReqwestError;
use rmcp::transport::{
    auth::{AuthError, AuthorizationManager, AuthorizationSession},
    streamable_http_client::StreamableHttpError,
};
use tokio::sync::Mutex;

use crate::console::{print_app_event, print_app_warning};
use crate::paths::oauth_credentials_path;

use super::callback::CallbackServer;
use super::store::FileCredentialStore;

#[derive(Clone)]
pub(crate) struct RemoteAuth {
    server_name: String,
    base_url: String,
    pub(crate) manager: Arc<Mutex<AuthorizationManager>>,
    flow_lock: Arc<Mutex<()>>,
}

impl RemoteAuth {
    pub(crate) async fn new(server_name: &str, base_url: &str) -> Result<Self, Box<dyn Error>> {
        let credentials_path = oauth_credentials_path(server_name)?;
        let mut manager = AuthorizationManager::new(base_url).await?;
        manager.set_credential_store(FileCredentialStore::new(credentials_path));
        manager.initialize_from_store().await?;

        Ok(Self {
            server_name: server_name.to_string(),
            base_url: base_url.to_string(),
            manager: Arc::new(Mutex::new(manager)),
            flow_lock: Arc::new(Mutex::new(())),
        })
    }

    pub(crate) async fn ensure_authorized(
        &self,
        required_scope: Option<&str>,
    ) -> Result<(), StreamableHttpError<ReqwestError>> {
        let _flow_guard = self.flow_lock.lock().await;
        if required_scope.is_none() {
            let manager = self.manager.lock().await;
            match manager.get_access_token().await {
                Ok(_) => return Ok(()),
                Err(AuthError::AuthorizationRequired) => {}
                Err(error) => return Err(StreamableHttpError::Auth(error)),
            }
        }

        let callback = CallbackServer::bind().await?;
        let redirect_uri = callback.redirect_uri();

        let mut manager_guard = self.manager.lock().await;
        let placeholder = AuthorizationManager::new(self.base_url.as_str())
            .await
            .map_err(StreamableHttpError::Auth)?;
        let manager = std::mem::replace(&mut *manager_guard, placeholder);
        drop(manager_guard);

        let session = build_authorization_session(manager, &redirect_uri, required_scope).await?;
        let auth_url = session.get_authorization_url().to_string();
        print_app_event(
            "remote.oauth",
            format!(
                "Opening browser for OAuth login for remote MCP server `{}`",
                self.server_name
            ),
        );
        print_app_event("remote.oauth", format!("OAuth URL: {auth_url}"));
        if let Err(error) = webbrowser::open(&auth_url) {
            print_app_warning(
                "remote.oauth",
                format!("failed to open a browser automatically: {error}"),
            );
        }

        let callback_result = callback.wait_for_callback().await?;
        session
            .handle_callback(&callback_result.code, &callback_result.state)
            .await?;
        let mut manager_guard = self.manager.lock().await;
        *manager_guard = session.auth_manager;
        drop(manager_guard);

        print_app_event(
            "remote.oauth",
            format!(
                "Completed OAuth login for remote MCP server `{}`",
                self.server_name
            ),
        );
        Ok(())
    }
}

async fn build_authorization_session(
    mut manager: AuthorizationManager,
    redirect_uri: &str,
    required_scope: Option<&str>,
) -> Result<AuthorizationSession, StreamableHttpError<ReqwestError>> {
    let metadata = manager
        .discover_metadata()
        .await
        .map_err(StreamableHttpError::Auth)?;
    manager.set_metadata(metadata);

    if let Some(required_scope) = required_scope {
        match manager.request_scope_upgrade(required_scope).await {
            Ok(auth_url) => {
                return Ok(AuthorizationSession::for_scope_upgrade(
                    manager,
                    auth_url,
                    redirect_uri,
                ));
            }
            Err(AuthError::AuthorizationRequired) | Err(AuthError::InternalError(_)) => {}
            Err(error) => return Err(StreamableHttpError::Auth(error)),
        }
    }

    let scopes = if let Some(required_scope) = required_scope {
        vec![required_scope.to_string()]
    } else {
        manager.select_scopes(None, &[])
    };
    let scope_refs = scopes.iter().map(String::as_str).collect::<Vec<_>>();
    AuthorizationSession::new(manager, &scope_refs, redirect_uri, Some("msp"), None)
        .await
        .map_err(StreamableHttpError::Auth)
}
