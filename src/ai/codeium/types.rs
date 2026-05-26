//! Codeium API request / response types.
//!
//! These are the JSON shapes used by the Codeium REST endpoints (both the
//! cloud API and the local language-server gRPC-JSON gateway).

use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Completion (cloud API — used by CompletionEngine)
// ---------------------------------------------------------------------------

#[derive(Debug, Serialize)]
pub struct CompletionRequest {
    pub prompt: String,
    pub suffix: String,
    pub cursor_offset: i32,
    pub editor_language: String,
    pub line_ending: String,
    pub max_tokens: i32,
    pub indentation_size: i32,
    #[serde(rename = "region_includes_cursor")]
    pub region_includes_cursor: bool,
}

#[derive(Debug, Deserialize)]
pub struct CompletionResponse {
    pub code_completions: Vec<CodeCompletion>,
}

#[derive(Debug, Deserialize)]
pub struct CodeCompletion {
    pub completion: CompletionText,
}

#[derive(Debug, Deserialize)]
pub struct CompletionText {
    pub text: String,
}

// ---------------------------------------------------------------------------
// Heartbeat
// ---------------------------------------------------------------------------

#[derive(Debug, Serialize)]
pub struct HeartbeatRequest {
    pub session_id: String,
    pub timezone_offset_minutes: i32,
}

// ---------------------------------------------------------------------------
// Auth
// ---------------------------------------------------------------------------

#[derive(Debug, Serialize)]
pub struct AuthRequest {
    pub api_key: String,
    pub fps: i32,
}

#[derive(Debug, Deserialize)]
pub struct AuthResponse {
    pub api_key: String,
    pub user: User,
}

#[derive(Debug, Deserialize)]
pub struct User {
    pub name: String,
    pub email: String,
}

#[derive(Debug, Deserialize)]
pub struct StateResponse {
    pub api_key: Option<String>,
}
