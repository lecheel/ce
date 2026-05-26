//! Codeium authentication manager.
//!
//! Handles API-key verification, auto-discovery from Neovim / Windsurf
//! caches, and the interactive browser login flow.

use anyhow::{Context, Result};
use colored::Colorize;
use reqwest::Client;
use std::path::PathBuf;

use crate::ai::codeium::certs::CertHandler;
use crate::config::Config;

// ---------------------------------------------------------------------------
// AuthManager
// ---------------------------------------------------------------------------

pub struct AuthManager {
    client: Client,
    config: Config,
}

impl AuthManager {
    pub fn new(config: Config, cert_handler: &CertHandler) -> Result<Self> {
        Ok(Self {
            client: cert_handler.client().clone(),
            config,
        })
    }

    /// Verifies the API key works by making a simple authenticated request.
    pub async fn verify_key(&self, api_key: &str) -> Result<bool> {
        let url = format!("{}/api/v1/get_state", self.config.api_url);
        log::debug!("Verifying API key validity. URL target: {}", url);

        let response = self
            .client
            .get(&url)
            .header("Authorization", format!("Basic {}", api_key))
            .send()
            .await?;

        let status = response.status();
        log::debug!("API Key validation status result: {:?}", status);
        Ok(status.is_success())
    }

    /// Send a heartbeat to the Codeium server.
    pub async fn send_heartbeat(&self) -> Result<()> {
        if let Some(ref api_key) = self.config.api_key {
            let url = format!("{}/api/v1/heartbeat", self.config.api_url);
            log::debug!("Sending heartbeat check. Target: {}", url);

            let request = crate::ai::codeium::types::HeartbeatRequest {
                session_id: uuid::Uuid::new_v4().to_string(),
                timezone_offset_minutes: 0,
            };

            let _ = self
                .client
                .post(&url)
                .header("Authorization", format!("Basic {}", api_key))
                .json(&request)
                .send()
                .await?;
        }
        Ok(())
    }

    /// Automatically finds the API key from the Neovim/Codeium cache.
    pub fn discover_local_api_key() -> Option<(String, PathBuf)> {
        log::debug!("Checking local path cache for pre-saved API configurations...");

        if let Some(cache_dir) = dirs::cache_dir() {
            let path = cache_dir.join("nvim").join("codeium").join("config.json");
            log::debug!("Searching cache path: {:?}", path);
            if let Some(key) = Self::read_key_from_path(&path) {
                return Some((key, path));
            }
        }

        if let Some(data_dir) = dirs::data_dir() {
            let path = data_dir.join("nvim").join("codeium").join("config.json");
            log::debug!("Searching data path: {:?}", path);
            if let Some(key) = Self::read_key_from_path(&path) {
                return Some((key, path));
            }
        }

        if let Some(home) = dirs::home_dir() {
            let path = home
                .join("AppData")
                .join("Local")
                .join("nvim-data")
                .join("codeium")
                .join("config.json");
            log::debug!("Searching AppData Windows path: {:?}", path);
            if let Some(key) = Self::read_key_from_path(&path) {
                return Some((key, path));
            }

            let path = home.join(".codeium").join("config.json");
            log::debug!("Searching fallback path: {:?}", path);
            if let Some(key) = Self::read_key_from_path(&path) {
                return Some((key, path));
            }
        }

        log::debug!("Local discovery did not find any existing API key files.");
        None
    }

    fn read_key_from_path(path: &PathBuf) -> Option<String> {
        if path.exists() {
            if let Ok(content) = std::fs::read_to_string(path) {
                if let Ok(json) = serde_json::from_str::<serde_json::Value>(&content) {
                    if let Some(key) = json.get("api_key").and_then(|v| v.as_str()) {
                        if !key.is_empty() {
                            log::debug!("Discovered key successfully within: {:?}", path);
                            return Some(key.to_string());
                        }
                    }
                }
            }
        }
        None
    }

    /// Execute the standard browser authentication flow to fetch a new API key.
    pub async fn login_flow(&self) -> Result<String> {
        let state = uuid::Uuid::new_v4().to_string();
        let url = format!(
            "https://www.codeium.com/profile?response_type=token&redirect_uri=show-auth-token&state={}&scope=openid%20profile%20email&redirect_parameters_type=query",
            state
        );

        log::debug!(
            "Starting browser interactive authentication. UUID state: {}",
            state
        );
        println!("Generated state (UUID): {}", state);
        println!("\nOpening browser for Codeium login...");

        if let Err(e) = open_browser(&url) {
            log::debug!("Browser execution failed: {}", e);
            println!("Could not open browser automatically: {}", e);
            println!(
                "Please manually visit the following link to log in:\n{}",
                url
            );
        }

        println!("\n{}", "=".repeat(60));
        println!("After logging in, the page will present an authentication token.");
        println!("Please copy that token and paste it below:");
        println!("{}", "=".repeat(60));

        print!("Paste token here: ");
        std::io::Write::flush(&mut std::io::stdout()).context("Failed to flush stdout")?;

        let mut auth_token = String::new();
        std::io::stdin()
            .read_line(&mut auth_token)
            .context("Failed to read token from stdin")?;
        let auth_token = auth_token.trim().to_string();

        if auth_token.is_empty() {
            log::debug!("User provided an empty authentication token.");
            anyhow::bail!("No token was provided. Aborting authentication.");
        }

        log::debug!(
            "Exchanging pasted authentication token (length={}) for permanent API Key...",
            auth_token.len()
        );

        let response = self
            .client
            .post("https://api.codeium.com/register_user/")
            .json(&serde_json::json!({ "firebase_id_token": auth_token }))
            .send()
            .await
            .context("Failed to send registration request to Codeium API")?;

        let status = response.status();
        log::debug!("Token exchange response status: {:?}", status);

        if !status.is_success() {
            anyhow::bail!(
                "Failed to register token with Codeium API. Server returned: {}",
                status
            );
        }

        let data: serde_json::Value = response
            .json()
            .await
            .context("Failed to parse registration JSON response")?;

        let api_key = data
            .get("api_key")
            .and_then(|v| v.as_str())
            .ok_or_else(|| {
                anyhow::anyhow!("The response did not contain an api_key field: {:?}", data)
            })?
            .to_string();

        let username = data
            .get("name")
            .and_then(|v| v.as_str())
            .unwrap_or("Developer");

        log::debug!(
            "Successfully authenticated! User Name: {}. API Key length: {}",
            username,
            api_key.len()
        );
        println!("\nWelcome, {}!", username.green());

        Ok(api_key)
    }

    /// Smart authentication: Auto-discovers key, verifies it, falls back to native login.
    pub async fn smart_auth(&self) -> Result<String> {
        if let Some((api_key, path)) = Self::discover_local_api_key() {
            println!("Found existing Windsurf API key!");
            println!("   Location: {:?}", path);

            print!("Verifying key with Codeium... ");
            std::io::Write::flush(&mut std::io::stdout()).context("Failed to flush stdout")?;

            if self.verify_key(&api_key).await? {
                println!("{}", "Valid!".green());
                return Ok(api_key);
            } else {
                println!("{}", "Expired or invalid.".red());
            }
        }

        println!("Could not find a valid API key automatically.");
        println!("Let's walk through standard browser authentication.\n");
        self.login_flow().await
    }
}

// ---------------------------------------------------------------------------
// Browser helper
// ---------------------------------------------------------------------------

/// Simple cross-platform browser helper.
fn open_browser(url: &str) -> Result<()> {
    #[cfg(target_os = "macos")]
    {
        std::process::Command::new("open").arg(url).status()?;
    }
    #[cfg(target_os = "windows")]
    {
        std::process::Command::new("cmd")
            .args(&["/C", "start", "", url])
            .status()?;
    }
    #[cfg(target_os = "linux")]
    {
        std::process::Command::new("xdg-open").arg(url).status()?;
    }
    Ok(())
}
