//! Codeium cloud completion engine (REST-based).
//!
//! This is the original REST API client that talks directly to the Codeium
//! cloud.  The local language-server path (via `server::CodeiumServer`) is
//! the preferred backend; this module is kept as a fallback.

use anyhow::Result;
use reqwest::Client;

use crate::ai::codeium::certs::CertHandler;
use crate::ai::codeium::types::{CompletionRequest, CompletionResponse};
use crate::config::Config;

// ---------------------------------------------------------------------------
// CompletionEngine
// ---------------------------------------------------------------------------

pub struct CompletionEngine {
    client: Client,
    config: Config,
}

impl CompletionEngine {
    pub fn new(config: Config, cert_handler: &CertHandler) -> Result<Self> {
        Ok(Self {
            client: cert_handler.client().clone(),
            config,
        })
    }

    /// Request a completion from the Codeium cloud API.
    pub async fn get_completion(
        &self,
        prompt: &str,
        suffix: &str,
        language: &str,
    ) -> Result<Option<String>> {
        let api_key =
            self.config.api_key.as_ref().ok_or_else(|| {
                anyhow::anyhow!("Not authenticated. Run: codeium-cli auth <api_key>")
            })?;

        let cursor_offset = prompt.len() as i32;
        let line_ending = if prompt.contains("\r\n") {
            "\r\n".to_string()
        } else {
            "\n".to_string()
        };

        let request = CompletionRequest {
            prompt: prompt.to_string(),
            suffix: suffix.to_string(),
            cursor_offset,
            editor_language: language.to_string(),
            line_ending,
            max_tokens: self.config.max_tokens,
            indentation_size: 4,
            region_includes_cursor: true,
        };

        let url = format!("{}/api/v1/completion", self.config.api_url);

        let response = self
            .client
            .post(&url)
            .header("User-Agent", "codeium-cli/0.1.0")
            .header("Content-Type", "application/json")
            .header("Authorization", format!("Basic {}", api_key))
            .json(&request)
            .send()
            .await?;

        if !response.status().is_success() {
            anyhow::bail!("Completion request failed: {}", response.status())
        }

        let completion: CompletionResponse = response.json().await?;

        if let Some(first) = completion.code_completions.first() {
            Ok(Some(first.completion.text.clone()))
        } else {
            Ok(None)
        }
    }
}
