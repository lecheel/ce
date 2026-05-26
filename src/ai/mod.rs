//! AI integration module.
//!
//! Currently only Codeium is supported; additional backends can be added as
//! sub-modules (e.g. `ai::copilot`, `ai::local`) that implement
//! [`crate::comp::provider::CompletionProvider`].

pub mod codeium;
pub mod llama;
