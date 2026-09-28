//! Application Default Credentials (ADC) authentication for Gemini on Vertex AI.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde_json::Value;

use crate::ProviderError;

const DEFAULT_TOKEN_URL: &str = "https://oauth2.googleapis.com/token";

/// Authentication handler for Vertex AI requests.
///
/// Supports static OAuth tokens or automatic refresh via an authorized-user ADC file.
pub struct VertexAuth {
    inner: AuthInner,
}

enum AuthInner {
    AccessToken(String),
    AuthorizedUser(AuthorizedUserAuth),
}

#[derive(Clone)]
struct AuthorizedUserAuth {
    client_id: String,
    client_secret: String,
    refresh_token: String,
    quota_project_id: Option<String>,
    token_url: String,
    cached: Arc<Mutex<Option<CachedToken>>>,
}

#[derive(Clone)]
struct CachedToken {
    access_token: String,
    expires_at: Instant,
}

impl std::fmt::Debug for VertexAuth {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match &self.inner {
            AuthInner::AccessToken(_) => f
                .debug_struct("VertexAuth")
                .field("source", &"AccessToken")
                .finish(),
            AuthInner::AuthorizedUser(user) => f
                .debug_struct("VertexAuth")
                .field("source", &"AuthorizedUser")
                .field("token_url", &user.token_url)
                .field("quota_project_id", &user.quota_project_id)
                .finish(),
        }
    }
}

impl VertexAuth {
    /// Creates a [`VertexAuth`] instance using a static access token.
    ///
    /// The token is used verbatim and is never refreshed.
    pub fn from_token(token: impl Into<String>) -> Self {
        Self {
            inner: AuthInner::AccessToken(token.into()),
        }
    }

    /// Creates a [`VertexAuth`] instance by reading an ADC credentials JSON file.
    ///
    /// If `token_url` is `None`, defaults to `https://oauth2.googleapis.com/token`.
    pub fn from_adc_file(
        path: impl AsRef<Path>,
        token_url: Option<&str>,
    ) -> Result<Self, ProviderError> {
        let path_ref = path.as_ref();
        let content = std::fs::read_to_string(path_ref)
            .map_err(|_| ProviderError::MissingKey("GOOGLE_APPLICATION_CREDENTIALS"))?;

        let v: Value = serde_json::from_str(&content).map_err(|e| {
            ProviderError::Auth(format!("failed to parse ADC credentials file: {e}"))
        })?;

        let cred_type = v.get("type").and_then(|t| t.as_str());
        match cred_type {
            Some("service_account") => Err(ProviderError::Auth(
                "service-account credentials are not supported yet; set GOOGLE_OAUTH_ACCESS_TOKEN, for example from gcloud auth print-access-token".to_string(),
            )),
            Some("authorized_user") => {
                let client_id = v
                    .get("client_id")
                    .and_then(|s| s.as_str())
                    .ok_or_else(|| {
                        ProviderError::Auth("missing client_id in credentials file".to_string())
                    })?
                    .to_string();
                let client_secret = v
                    .get("client_secret")
                    .and_then(|s| s.as_str())
                    .ok_or_else(|| {
                        ProviderError::Auth("missing client_secret in credentials file".to_string())
                    })?
                    .to_string();
                let refresh_token = v
                    .get("refresh_token")
                    .and_then(|s| s.as_str())
                    .ok_or_else(|| {
                        ProviderError::Auth("missing refresh_token in credentials file".to_string())
                    })?
                    .to_string();
                let quota_project_id = v
                    .get("quota_project_id")
                    .and_then(|s| s.as_str())
                    .filter(|s| !s.trim().is_empty())
                    .map(String::from);
                let url = token_url
                    .map(String::from)
                    .unwrap_or_else(|| DEFAULT_TOKEN_URL.to_string());
                Ok(Self {
                    inner: AuthInner::AuthorizedUser(AuthorizedUserAuth {
                        client_id,
                        client_secret,
                        refresh_token,
                        quota_project_id,
                        token_url: url,
                        cached: Arc::new(Mutex::new(None)),
                    }),
                })
            }
            _ => Err(ProviderError::Auth(
                "unsupported credential type in credentials file".to_string(),
            )),
        }
    }

    /// Discovers credentials from the ambient environment:
    ///
    /// 1. `GOOGLE_OAUTH_ACCESS_TOKEN`, if set and non-empty.
    /// 2. ADC file: `GOOGLE_APPLICATION_CREDENTIALS`, else
    ///    `$HOME/.config/gcloud/application_default_credentials.json`.
    pub fn from_env() -> Result<Self, ProviderError> {
        if let Ok(token) = std::env::var("GOOGLE_OAUTH_ACCESS_TOKEN") {
            let token = token.trim();
            if !token.is_empty() {
                return Ok(Self::from_token(token));
            }
        }

        let path = adc_path_from_env()
            .ok_or(ProviderError::MissingKey("GOOGLE_APPLICATION_CREDENTIALS"))?;
        Self::from_adc_file(path, None)
    }

    /// Returns a valid OAuth access token, refreshing if necessary.
    pub fn token(&self) -> Result<String, ProviderError> {
        match &self.inner {
            AuthInner::AccessToken(tok) => Ok(tok.clone()),
            AuthInner::AuthorizedUser(user) => user.token(),
        }
    }

    /// Invalidates any cached token (e.g. following a 401 Unauthorized response).
    pub fn clear_cached_token(&self) {
        if let AuthInner::AuthorizedUser(user) = &self.inner {
            user.clear_cached_token();
        }
    }

    /// The `quota_project_id` defined in the ADC file, if present.
    pub fn quota_project_id(&self) -> Option<&str> {
        match &self.inner {
            AuthInner::AccessToken(_) => None,
            AuthInner::AuthorizedUser(user) => user.quota_project_id.as_deref(),
        }
    }
}

impl AuthorizedUserAuth {
    fn token(&self) -> Result<String, ProviderError> {
        let mut guard = self.cached.lock().unwrap();
        let now = Instant::now();
        if let Some(cached) = &*guard {
            let remaining = cached
                .expires_at
                .checked_duration_since(now)
                .unwrap_or(Duration::ZERO);
            if remaining >= Duration::from_secs(300) {
                return Ok(cached.access_token.clone());
            }
        }

        let agent = crate::http_client();
        let resp = agent.post(&self.token_url).send_form(&[
            ("grant_type", "refresh_token"),
            ("client_id", &self.client_id),
            ("client_secret", &self.client_secret),
            ("refresh_token", &self.refresh_token),
        ]);

        match resp {
            Ok(r) => {
                let v: Value = r
                    .into_json()
                    .map_err(|e| ProviderError::Parse(e.to_string()))?;
                let access_token = v
                    .get("access_token")
                    .and_then(|s| s.as_str())
                    .ok_or_else(|| {
                        ProviderError::Auth("missing access_token in token response".to_string())
                    })?
                    .to_string();
                let expires_in = v.get("expires_in").and_then(|n| n.as_u64()).unwrap_or(3600);

                let expires_at = Instant::now() + Duration::from_secs(expires_in);
                *guard = Some(CachedToken {
                    access_token: access_token.clone(),
                    expires_at,
                });
                Ok(access_token)
            }
            Err(ureq::Error::Status(status, _)) => Err(ProviderError::Auth(format!(
                "token refresh failed with status {status}"
            ))),
            Err(e) => Err(ProviderError::Transport(e.to_string())),
        }
    }

    fn clear_cached_token(&self) {
        if let Ok(mut guard) = self.cached.lock() {
            *guard = None;
        }
    }
}

/// Resolves the candidate path to the ADC file from environment variables.
pub fn adc_path_from_env() -> Option<PathBuf> {
    if let Ok(path) = std::env::var("GOOGLE_APPLICATION_CREDENTIALS") {
        let trimmed = path.trim();
        if !trimmed.is_empty() {
            return Some(PathBuf::from(trimmed));
        }
    }
    if let Ok(home) = std::env::var("HOME") {
        let trimmed = home.trim();
        if !trimmed.is_empty() {
            return Some(
                PathBuf::from(trimmed).join(".config/gcloud/application_default_credentials.json"),
            );
        }
    }
    None
}

/// Attempts to read `quota_project_id` from the ambient ADC file without performing full auth setup.
pub fn quota_project_id_from_env() -> Option<String> {
    let path = adc_path_from_env()?;
    let content = std::fs::read_to_string(path).ok()?;
    let v: Value = serde_json::from_str(&content).ok()?;
    v.get("quota_project_id")
        .and_then(|s| s.as_str())
        .filter(|s| !s.trim().is_empty())
        .map(String::from)
}
