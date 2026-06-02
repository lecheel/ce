// src/msgbox.rs
//! Message bus between the LSP background task and the editor.
//!
//! This is a *separate* enum from the main-loop `AppMessage` in `main.rs`.
//! The LSP task sends these through a tokio channel → std::sync bridge →
//! `editor.lsp_rx`, which is drained by `editor.poll_lsp_responses()`.

use crate::lsp::lsp::{CompletionItem, InlayHint, SignatureHelpState, TextEdit};

/// Sender half of the LSP response channel.
pub type AppSender = tokio::sync::mpsc::UnboundedSender<AppMessage>;

/// Messages flowing FROM the LSP task TO the editor.
#[derive(Debug)]
pub enum AppMessage {
    LspDiagnostics {
        uri: String,
        version: Option<i32>,
        diagnostics: Vec<crate::ed::buffer::Diagnostic>,
    },
    LspFormatResult {
        result: Result<Option<Vec<TextEdit>>, String>,
        buffer_idx: usize,
        cursor_state: Option<(usize, usize)>,
        save_after: bool,
    },

    LspInlayHints {
        uri: String,
        hints: Vec<InlayHint>,
        version: i32,
    },
    LspSignatureHelp(Option<SignatureHelpState>),
    LspCompletion {
        items: Option<Vec<CompletionItem>>,
        version: u64,
    },
    LspCompletionResolved(CompletionItem),
    LspError(String),
}
