use std::fs;
use std::path::PathBuf;

use async_trait::async_trait;
use rmcp::transport::auth::{AuthError, CredentialStore, StoredCredentials};

use crate::fs_util::{acquire_sibling_lock, write_file_atomically};

#[derive(Clone)]
pub(crate) struct FileCredentialStore {
    path: PathBuf,
}

impl FileCredentialStore {
    pub(crate) fn new(path: PathBuf) -> Self {
        Self { path }
    }
}

#[async_trait]
impl CredentialStore for FileCredentialStore {
    async fn load(&self) -> Result<Option<StoredCredentials>, AuthError> {
        let _guard = acquire_sibling_lock(&self.path)
            .map_err(|error| AuthError::InternalError(error.to_string()))?;
        if !self.path.exists() {
            return Ok(None);
        }
        let contents = fs::read_to_string(&self.path)
            .map_err(|error| AuthError::InternalError(error.to_string()))?;
        serde_json::from_str(&contents)
            .map(Some)
            .map_err(|error| AuthError::InternalError(error.to_string()))
    }

    async fn save(&self, credentials: StoredCredentials) -> Result<(), AuthError> {
        let _guard = acquire_sibling_lock(&self.path)
            .map_err(|error| AuthError::InternalError(error.to_string()))?;
        let contents = serde_json::to_string_pretty(&credentials)
            .map_err(|error| AuthError::InternalError(error.to_string()))?;
        write_file_atomically(&self.path, contents.as_bytes())
            .map_err(|error| AuthError::InternalError(error.to_string()))
    }

    async fn clear(&self) -> Result<(), AuthError> {
        let _guard = acquire_sibling_lock(&self.path)
            .map_err(|error| AuthError::InternalError(error.to_string()))?;
        if self.path.exists() {
            fs::remove_file(&self.path)
                .map_err(|error| AuthError::InternalError(error.to_string()))?;
        }
        Ok(())
    }
}
