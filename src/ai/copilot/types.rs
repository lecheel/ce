//! GitHub Copilot request / response types.
//!
//! Covers three communication paths:
//! 1. **LSP JSON-RPC** — used by [`server::CopilotServer`] to talk to the
//!    local Copilot agent over stdio.
//! 2. **Copilot proxy REST API** — used by [`engine::CompletionEngine`] to
//!    call the cloud completion endpoint directly.
//! 3. **GitHub OAuth** — used by [`auth::AuthManager`] for device-flow
//!    authentication and token management.

use serde::{Deserialize, Serialize};

// ── LSP base types ────────────────────────────────────────────────────────

/// A single LSP position (zero-based).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Position {
    pub line: u32,
    pub character: u32,
}

/// An LSP range (zero-based, half-open).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Range {
    pub start: Position,
    pub end: Position,
}

// ── Copilot LSP `getCompletions` ──────────────────────────────────────────

/// Document metadata sent in the Copilot-specific `getCompletions` request.
#[derive(Debug, Clone, Serialize)]
pub struct CopilotDoc {
    pub source: String,
    #[serde(rename = "tabSize")]
    pub tab_size: u32,
    #[serde(rename = "indentSize")]
    pub indent_size: u32,
    #[serde(rename = "insertSpaces")]
    pub insert_spaces: bool,
    pub path: String,
    pub uri: String,
    #[serde(rename = "relativePath")]
    pub relative_path: String,
    #[serde(rename = "languageId")]
    pub language_id: String,
    pub position: Position,
    pub version: u32,
}

/// Parameters for the Copilot `getCompletions` LSP method.
#[derive(Debug, Clone, Serialize)]
pub struct GetCompletionsParams {
    pub doc: CopilotDoc,
    pub position: Position,
}

/// A single completion returned by the agent.
#[derive(Debug, Clone, Deserialize)]
pub struct CopilotCompletion {
    pub text: String,
    pub position: Option<Position>,
    pub uuid: Option<String>,
    pub range: Option<Range>,
    #[serde(rename = "displayText")]
    pub display_text: Option<String>,
}

/// The `result` field of the `getCompletions` response.
#[derive(Debug, Clone, Deserialize)]
pub struct GetCompletionsResult {
    pub completions: Vec<CopilotCompletion>,
}

// ── Copilot proxy REST API (engine.rs) ────────────────────────────────────

/// Request body for the Copilot proxy `/v1/engines/.../completions` endpoint.
#[derive(Debug, Serialize)]
pub struct ProxyCompletionRequest {
    pub prompt: String,
    pub suffix: String,
    #[serde(rename = "max_tokens")]
    pub max_tokens: i32,
    pub temperature: f32,
    #[serde(rename = "top_p")]
    pub top_p: f32,
    pub n: i32,
    pub stop: Vec<String>,
    pub stream: bool,
    pub extra: ProxyCompletionExtra,
}

#[derive(Debug, Serialize)]
pub struct ProxyCompletionExtra {
    pub language: String,
}

/// A single choice from the proxy completion response.
#[derive(Debug, Deserialize)]
pub struct ProxyCompletionChoice {
    pub text: String,
    pub index: Option<i32>,
}

/// The response from the Copilot proxy.
#[derive(Debug, Deserialize)]
pub struct ProxyCompletionResponse {
    pub choices: Vec<ProxyCompletionChoice>,
}

/// Response from `GET /copilot_internal/v2/token`.
#[derive(Debug, Deserialize)]
pub struct CopilotTokenResponse {
    pub token: String,
    #[serde(rename = "expires_at")]
    pub expires_at: i64,
}

// ── GitHub OAuth device flow (auth.rs) ────────────────────────────────────

/// Response from `POST /login/device/code`.
#[derive(Debug, Deserialize)]
pub struct DeviceCodeResponse {
    pub device_code: String,
    pub user_code: String,
    #[serde(rename = "verification_uri")]
    pub verification_uri: String,
    #[serde(rename = "expires_in")]
    pub expires_in: i64,
    pub interval: i64,
}

/// Possible responses from `POST /login/oauth/access_token` during polling.
#[derive(Debug, Deserialize)]
pub struct DeviceAccessTokenResponse {
    #[serde(rename = "access_token")]
    pub access_token: Option<String>,
    #[serde(rename = "token_type")]
    pub token_type: Option<String>,
    pub scope: Option<String>,
    pub error: Option<String>,
    #[serde(rename = "error_description")]
    pub error_description: Option<String>,
}

/// Shape of `~/.config/github-copilot/hosts.json`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HostsFile {
    #[serde(rename = "github.com")]
    pub github_com: HostEntry,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HostEntry {
    #[serde(rename = "oauth_token")]
    pub oauth_token: String,
}

// ── Heartbeat (for telemetry / keep-alive) ────────────────────────────────

#[derive(Debug, Serialize)]
pub struct HeartbeatRequest {
    #[serde(rename = "sessionId")]
    pub session_id: String,
}
