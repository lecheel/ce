//! Direct Copilot proxy REST completion engine.
//!
//! This engine talks straight to the Copilot cloud API at
//! `https://copilot-proxy.githubusercontent.com`.  It is kept as a
//! fallback; the preferred backend is the local LSP agent managed by
//! [`server::CopilotServer`].
//!
//! **Flow:**
//! 1. Exchange the GitHub OAuth token for a short-lived Copilot JWT.
//! 2. Use the JWT to request completions from the proxy.
//! 3. Automatically refresh the JWT when it expires.

use anyhow::{Context, Result};
use reqwest::Client;
use std::sync::Arc;
use tokio::sync::RwLock;

use crate::ai::copilot::certs::CertHandler;
use crate::ai::copilot::types::{
    CopilotTokenResponse, ProxyCompletionExtra, ProxyCompletionRequest, ProxyCompletionResponse,
};
use crate::config::Config;

// ──────────────────────────────────────────────────────────────────────────
// Internal token cache
// ──────────────────────────────────────────────────────────────────────────

#[derive(Debug)]
struct CachedToken {
    token: String,
    expires_at: i64,
}

impl CachedToken {
    fn is_expired(&self) -> bool {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs() as i64;
        now >= self.expires_at - 30 // 30-second safety margin
    }
}

// ──────────────────────────────────────────────────────────────────────────
// CompletionEngine
// ──────────────────────────────────────────────────────────────────────────

pub struct CompletionEngine {
    client: Client,
    config: Config,
    copilot_token: Arc<RwLock<Option<CachedToken>>>,
}

impl CompletionEngine {
    pub fn new(config: Config, cert_handler: &CertHandler) -> Result<Self> {
        Ok(Self {
            client: cert_handler.client().clone(),
            config,
            copilot_token: Arc::new(RwLock::new(None)),
        })
    }

    /// Obtain a valid Copilot JWT, refreshing from the GitHub OAuth token
    /// if necessary.
    async fn get_copilot_token(&self) -> Result<String> {
        // Check cache first.
        {
            let read = self.copilot_token.read().await;
            if let Some(ref cached) = *read {
                if !cached.is_expired() {
                    return Ok(cached.token.clone());
                }
            }
        }

        // Refresh.
        let oauth_token = self.config.api_key.as_ref().ok_or_else(|| {
            anyhow::anyhow!("Not authenticated. Run auth flow first to get a GitHub OAuth token.")
        })?;

        let url = "https://api.github.com/copilot_internal/v2/token";
        log::debug!("Refreshing Copilot token from: {}", url);

        let response = self
            .client
            .get(url)
            .header("User-Agent", "copilot-cli/0.1.0")
            .header("Authorization", format!("token {}", oauth_token))
            .send()
            .await
            .context("Failed to refresh Copilot token")?;

        if !response.status().is_success() {
            anyhow::bail!(
                "Copilot token refresh failed: {} — ensure your Copilot subscription is active",
                response.status()
            );
        }

        let data: CopilotTokenResponse = response.json().await?;

        let cached = CachedToken {
            token: data.token.clone(),
            expires_at: data.expires_at,
        };

        {
            let mut write = self.copilot_token.write().await;
            *write = Some(cached);
        }

        log::debug!("Copilot token refreshed. Expires at: {}", data.expires_at);
        Ok(data.token)
    }

    /// Request a completion from the Copilot proxy API.
    pub async fn get_completion(
        &self,
        prompt: &str,
        suffix: &str,
        language: &str,
    ) -> Result<Option<String>> {
        let copilot_token = self.get_copilot_token().await?;

        let request = ProxyCompletionRequest {
            prompt: prompt.to_string(),
            suffix: suffix.to_string(),
            max_tokens: self.config.max_tokens,
            temperature: 0.0,
            top_p: 1.0,
            n: 1,
            stop: vec!["\n".to_string()],
            stream: false,
            extra: ProxyCompletionExtra {
                language: language.to_string(),
            },
        };

        let url =
            "https://copilot-proxy.githubusercontent.com/v1/engines/copilot-codex/completions";
        log::debug!("Requesting completion from: {}", url);

        let response = self
            .client
            .post(url)
            .header("User-Agent", "copilot-cli/0.1.0")
            .header("Content-Type", "application/json")
            .header("Authorization", format!("Bearer {}", copilot_token))
            .header("X-Request-Id", uuid::Uuid::new_v4().to_string())
            .header("Vscode-Sessionid", uuid::Uuid::new_v4().to_string())
            .header("Vscode-Machineid", uuid::Uuid::new_v4().to_string())
            .header("Editor-Version", "neovim/0.9.0")
            .header("Editor-Plugin-Version", "copilot-cli/0.1.0")
            .json(&request)
            .send()
            .await?;

        if !response.status().is_success() {
            anyhow::bail!(
                "Copilot proxy completion request failed: {}",
                response.status()
            );
        }

        let completion: ProxyCompletionResponse = response.json().await?;

        if let Some(first) = completion.choices.first() {
            if !first.text.is_empty() {
                return Ok(Some(first.text.clone()));
            }
        }

        Ok(None)
    }
}
