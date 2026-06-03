//! GitHub Copilot authentication manager.
//!
//! Handles:
//! - Auto-discovery of existing OAuth tokens from Neovim / VS Code caches
//! - GitHub device-flow browser login
//! - Token verification against the GitHub API
//! - Persisting tokens to `~/.config/github-copilot/hosts.json`

use anyhow::{Context, Result};
use colored::Colorize;
use reqwest::Client;
use std::path::PathBuf;
use tokio::time::{sleep, Duration};

use crate::ai::copilot::certs::CertHandler;
use crate::ai::copilot::types::{DeviceAccessTokenResponse, DeviceCodeResponse, HostsFile};
use crate::config::Config;

/// The public Copilot VS Code extension OAuth client-id (used by copilot.vim
/// and copilot.lua as well).
const COPILOT_CLIENT_ID: &str = "Iv1.b507a08c87ecfe98";

/// OAuth scopes requested during device flow.
const COPILOT_SCOPES: &str = "read:user";

// Add this enum to the top of auth.rs
pub enum CopilotAuthMsg {
    DeviceCode(String, String), // verification_uri, user_code
    Success(String),            // access_token
    Error(String),
}

// ──────────────────────────────────────────────────────────────────────────
// AuthManager
// ──────────────────────────────────────────────────────────────────────────

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

    /// Verify that a GitHub OAuth token is valid by calling the user API.
    pub async fn verify_key(&self, oauth_token: &str) -> Result<bool> {
        let url = "https://api.github.com/user";
        log::debug!("Verifying GitHub OAuth token against: {}", url);

        let response = self
            .client
            .get(url)
            .header("User-Agent", "copilot-cli/0.1.0")
            .header("Authorization", format!("token {}", oauth_token))
            .send()
            .await?;

        let status = response.status();
        log::debug!("Token verification status: {:?}", status);
        Ok(status.is_success())
    }

    /// Send a lightweight heartbeat to keep the token session alive.
    pub async fn send_heartbeat(&self) -> Result<()> {
        if let Some(ref oauth_token) = self.config.api_key {
            let url = "https://api.github.com/copilot_internal/v2/token";
            log::debug!("Sending heartbeat via token refresh. Target: {}", url);

            let response = self
                .client
                .get(url)
                .header("User-Agent", "copilot-cli/0.1.0")
                .header("Authorization", format!("token {}", oauth_token))
                .send()
                .await?;

            log::debug!("Heartbeat / token refresh status: {:?}", response.status());
        }
        Ok(())
    }

    /// Search standard locations for an existing GitHub Copilot OAuth token.
    ///
    /// Checks:
    /// 1. `GH_COPILOT_TOKEN` environment variable
    /// 2. `~/.config/github-copilot/hosts.json`  (standard Neovim / VS Code)
    /// 3. Windows AppData equivalents
    /// 4. XDG config fallbacks
    pub fn discover_local_token() -> Option<(String, PathBuf)> {
        log::debug!("Scanning local paths for pre-existing GitHub Copilot tokens...");

        // 0. Environment variable override
        if let Ok(token) = std::env::var("GH_COPILOT_TOKEN") {
            if !token.is_empty() {
                log::debug!("Found token in GH_COPILOT_TOKEN environment variable");
                return Some((token, PathBuf::from("$GH_COPILOT_TOKEN")));
            }
        }

        // 1. Standard XDG / Neovim / VS Code path
        if let Some(config_dir) = dirs::config_dir() {
            let path = config_dir.join("github-copilot").join("hosts.json");
            log::debug!("Searching: {:?}", path);
            if let Some(token) = Self::read_token_from_hosts(&path) {
                return Some((token, path));
            }
        }

        // 2. Neovim-specific XDG data path (some distros)
        if let Some(data_dir) = dirs::data_dir() {
            let path = data_dir.join("github-copilot").join("hosts.json");
            log::debug!("Searching: {:?}", path);
            if let Some(token) = Self::read_token_from_hosts(&path) {
                return Some((token, path));
            }
        }

        // 3. Windows AppData
        if let Some(home) = dirs::home_dir() {
            let path = home
                .join("AppData")
                .join("Local")
                .join("github-copilot")
                .join("hosts.json");
            log::debug!("Searching: {:?}", path);
            if let Some(token) = Self::read_token_from_hosts(&path) {
                return Some((token, path));
            }
        }

        // 4. Flat ~/.github-copilot/hosts.json fallback
        if let Some(home) = dirs::home_dir() {
            let path = home.join(".github-copilot").join("hosts.json");
            log::debug!("Searching: {:?}", path);
            if let Some(token) = Self::read_token_from_hosts(&path) {
                return Some((token, path));
            }
        }

        log::debug!("Local discovery did not find any existing Copilot tokens.");
        None
    }

    /// Parse `hosts.json` and extract the OAuth token.
    fn read_token_from_hosts(path: &PathBuf) -> Option<String> {
        if !path.exists() {
            return None;
        }

        if let Ok(content) = std::fs::read_to_string(path) {
            // The file may contain extra fields; try the typed struct first,
            // then fall back to raw JSON extraction.
            if let Ok(hosts) = serde_json::from_str::<HostsFile>(&content) {
                let token = hosts.github_com.oauth_token;
                if !token.is_empty() {
                    log::debug!("Discovered token in: {:?}", path);
                    return Some(token);
                }
            } else if let Ok(json) = serde_json::from_str::<serde_json::Value>(&content) {
                if let Some(token) = json
                    .get("github.com")
                    .and_then(|v| v.get("oauth_token"))
                    .and_then(|v| v.as_str())
                {
                    if !token.is_empty() {
                        log::debug!("Discovered token (raw JSON) in: {:?}", path);
                        return Some(token.to_string());
                    }
                }
            }
        }
        None
    }

    /// Persist an OAuth token to `~/.config/github-copilot/hosts.json`.
    pub fn save_token_to_hosts(oauth_token: &str) -> Result<PathBuf> {
        let config_dir = dirs::config_dir().context("Could not find user config directory")?;
        let dir = config_dir.join("github-copilot");
        std::fs::create_dir_all(&dir)?;

        let path = dir.join("hosts.json");

        // If the file already exists, merge; otherwise create fresh.
        let mut hosts_value = if path.exists() {
            let existing = std::fs::read_to_string(&path).unwrap_or_default();
            serde_json::from_str::<serde_json::Value>(&existing)
                .unwrap_or_else(|_| serde_json::json!({}))
        } else {
            serde_json::json!({})
        };

        let obj = hosts_value
            .as_object_mut()
            .context("hosts.json is not a JSON object")?;
        obj.insert(
            "github.com".to_string(),
            serde_json::json!({ "oauth_token": oauth_token }),
        );

        let pretty = serde_json::to_string_pretty(&hosts_value)?;
        std::fs::write(&path, pretty)?;

        log::debug!("Token persisted to: {:?}", path);
        Ok(path)
    }

    /// Execute the GitHub device-flow browser login to obtain a new OAuth
    /// token.
    pub async fn login_flow(&self) -> Result<String> {
        log::debug!("Starting GitHub device-flow authentication...");

        // ── Step 1: Request device code ────────────────────────────────
        let response = self
            .client
            .post("https://github.com/login/device/code")
            .header("User-Agent", "copilot-cli/0.1.0")
            .header("Accept", "application/json")
            .form(&[("client_id", COPILOT_CLIENT_ID), ("scope", COPILOT_SCOPES)])
            .send()
            .await
            .context("Failed to request device code from GitHub")?;

        if !response.status().is_success() {
            anyhow::bail!("GitHub device-code request failed: {}", response.status());
        }

        let device: DeviceCodeResponse = response
            .json()
            .await
            .context("Failed to parse device-code response")?;

        log::debug!(
            "Device code obtained. User code: {}, Expires in: {}s",
            device.user_code,
            device.expires_in
        );

        // ── Step 2: Prompt user ────────────────────────────────────────
        println!("\n{}", "=".repeat(60));
        println!(
            "To authenticate, visit:  {}",
            device.verification_uri.green().bold()
        );
        println!(
            "And enter code:          {}",
            device.user_code.green().bold()
        );
        println!("{}", "=".repeat(60));

        if let Err(e) = open_browser(&device.verification_uri) {
            log::debug!("Could not open browser automatically: {}", e);
            println!("\nCould not open browser automatically. Please visit the URL manually.");
        }

        // ── Step 3: Poll for access token ──────────────────────────────
        println!("\nWaiting for authorization...");
        let poll_interval = Duration::from_secs(device.interval.max(5) as u64);
        let deadline = std::time::Instant::now() + Duration::from_secs(device.expires_in as u64);

        loop {
            sleep(poll_interval).await;

            if std::time::Instant::now() > deadline {
                anyhow::bail!("Device-flow authentication timed out.");
            }

            let response = self
                .client
                .post("https://github.com/login/oauth/access_token")
                .header("User-Agent", "copilot-cli/0.1.0")
                .header("Accept", "application/json")
                .form(&[
                    ("client_id", COPILOT_CLIENT_ID),
                    ("device_code", &device.device_code),
                    ("grant_type", "urn:ietf:params:oauth:grant-type:device_code"),
                ])
                .send()
                .await
                .context("Failed to poll GitHub for access token")?;

            let result: DeviceAccessTokenResponse = response.json().await?;

            if let Some(error) = &result.error {
                match error.as_str() {
                    "authorization_pending" => {
                        log::debug!("Authorization still pending...");
                        continue;
                    }
                    "slow_down" => {
                        log::debug!("GitHub asked us to slow down; backing off 5s");
                        sleep(Duration::from_secs(5)).await;
                        continue;
                    }
                    "expired_token" => {
                        anyhow::bail!("The device code has expired. Please try again.");
                    }
                    _ => {
                        anyhow::bail!(
                            "GitHub auth error: {} — {}",
                            error,
                            result.error_description.as_deref().unwrap_or("?")
                        );
                    }
                }
            }

            if let Some(access_token) = result.access_token {
                log::debug!("Access token obtained (length={})", access_token.len());

                // Persist the token so subsequent sessions auto-discover it.
                match Self::save_token_to_hosts(&access_token) {
                    Ok(path) => {
                        println!("Token saved to: {:?}", path);
                    }
                    Err(e) => {
                        log::debug!("Could not persist token: {}", e);
                    }
                }

                // Verify the token actually works with Copilot.
                println!("Verifying token with GitHub Copilot... ");

                if self.verify_key(&access_token).await? {
                    // Try to get a Copilot token to confirm subscription.
                    match self.fetch_copilot_token(&access_token).await {
                        Ok(ct) => {
                            println!("{}", "Copilot subscription confirmed!".green());
                            log::debug!("Copilot token expires at: {}", ct.expires_at);
                        }
                        Err(e) => {
                            println!(
                                "{}",
                                format!("Warning: Could not confirm Copilot subscription: {}", e)
                                    .yellow()
                            );
                        }
                    }
                    return Ok(access_token);
                } else {
                    anyhow::bail!("Token was obtained but could not be verified.");
                }
            }
        }
    }

    /// Exchange a GitHub OAuth token for a short-lived Copilot token (JWT).
    ///
    /// The Copilot token is required by the direct proxy API.  The LSP agent
    /// handles this exchange internally.
    pub async fn fetch_copilot_token(
        &self,
        oauth_token: &str,
    ) -> Result<crate::ai::copilot::types::CopilotTokenResponse> {
        let url = "https://api.github.com/copilot_internal/v2/token";
        log::debug!("Fetching Copilot token from: {}", url);

        let response = self
            .client
            .get(url)
            .header("User-Agent", "copilot-cli/0.1.0")
            .header("Authorization", format!("token {}", oauth_token))
            .send()
            .await
            .context("Failed to request Copilot token")?;

        if !response.status().is_success() {
            anyhow::bail!(
                "Copilot token request failed: {} — ensure you have an active Copilot subscription",
                response.status()
            );
        }

        let token: crate::ai::copilot::types::CopilotTokenResponse = response.json().await?;
        Ok(token)
    }

    /// Smart authentication: auto-discovers token, verifies it, falls back
    /// to browser device flow.
    pub async fn smart_auth(&self) -> Result<String> {
        if let Some((token, path)) = Self::discover_local_token() {
            println!("Found existing GitHub Copilot token!");
            println!("   Location: {:?}", path);

            print!("Verifying token with GitHub... ");
            std::io::Write::flush(&mut std::io::stdout()).context("Failed to flush stdout")?;

            if self.verify_key(&token).await? {
                println!("{}", "Valid!".green());

                // Also verify Copilot subscription is active.
                print!("Checking Copilot subscription... ");
                std::io::Write::flush(&mut std::io::stdout()).context("Failed to flush stdout")?;

                match self.fetch_copilot_token(&token).await {
                    Ok(_) => {
                        println!("{}", "Active!".green());
                        return Ok(token);
                    }
                    Err(_) => {
                        println!("{}", "No active subscription.".red());
                    }
                }
            } else {
                println!("{}", "Expired or invalid.".red());
            }
        }

        println!("Could not find a valid Copilot token automatically.");
        println!("Let's walk through GitHub device-flow authentication.\n");
        self.login_flow().await
    }
}

// ──────────────────────────────────────────────────────────────────────────
// Browser helper
// ──────────────────────────────────────────────────────────────────────────

// In ai/copilot/auth.rs
pub fn open_browser(url: &str) -> Result<()> {
    #[cfg(target_os = "macos")]
    {
        std::process::Command::new("open")
            .arg(url)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()?;
    }
    #[cfg(target_os = "windows")]
    {
        std::process::Command::new("cmd")
            .args(&["/C", "start", "", url])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()?;
    }
    #[cfg(target_os = "linux")]
    {
        std::process::Command::new("xdg-open")
            .arg(url)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()?;
    }
    Ok(())
}
