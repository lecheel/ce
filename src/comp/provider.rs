//! Completion provider trait.
//!
//! `CompletionProvider` is the async trait that every AI backend must
//! implement.  The editor calls `get_completions()` through this trait so
//! that swapping backends (Codeium cloud, local LSP, custom) is a config
//! change, not a code change.

#![allow(dead_code)]

use anyhow::Result;

// ---------------------------------------------------------------------------
// CompletionContext — what the provider receives
// ---------------------------------------------------------------------------

/// Context passed to the completion provider on each request.
#[derive(Debug, Clone)]
pub struct CompletionContext {
    /// Full document text.
    pub text: String,
    /// Absolute character offset of the cursor.
    pub cursor_offset: usize,
    /// Editor language identifier (e.g. "rust", "python").
    pub language: String,
}

// ---------------------------------------------------------------------------
// CompletionProvider — the trait
// ---------------------------------------------------------------------------

/// Trait for async completion backends.
///
/// Implementors fetch zero or more completion strings for the given context.
/// The editor's completion state machine handles ghost-text display, cycling,
/// and acceptance independently of the provider.
#[async_trait::async_trait]
pub trait CompletionProvider: Send + Sync {
    /// Return completion candidates for the given context.
    ///
    /// An empty `Vec` means "no completions available".
    /// Errors are logged and swallowed by the editor loop.
    async fn get_completions(&self, ctx: &CompletionContext) -> Result<Vec<String>>;
}
