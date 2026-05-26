//! Codeium AI backend.
//!
//! - [`types`]   — JSON request / response types
//! - [`certs`]   — TLS certificate handling & secure HTTP client
//! - [`auth`]    — Authentication (key discovery, browser login, verification)
//! - [`engine`]  — Cloud REST completion engine
//! - [`server`]  — Local language-server process management

pub mod auth;
pub mod certs;
pub mod engine;
pub mod server;
pub mod types;

#[allow(unused_imports)]
pub use auth::AuthManager;
#[allow(unused_imports)]
pub use certs::CertHandler;
#[allow(unused_imports)]
pub use engine::CompletionEngine;
pub use server::CodeiumServer;
