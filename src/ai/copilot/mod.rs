//! GitHub Copilot AI backend.
//!
//! - [`types`]   — LSP, API, and auth request/response types
//! - [`certs`]   — TLS certificate handling & secure HTTP client
//! - [`auth`]    — Authentication (GitHub device flow, token discovery, verification)
//! - [`engine`]  — Direct Copilot proxy REST completion engine (fallback)
//! - [`server`]  — Local Copilot agent (LSP over stdio) process management (primary)

pub mod auth;
pub mod certs;
pub mod engine;
pub mod server;
pub mod types;

pub use server::{start_copilot_server_task, CopilotServerCell};
