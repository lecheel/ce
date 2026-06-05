//! Language Server Protocol integration.
//!
//! Currently provides a client for the `ctagd` daemon — a lightweight
//! LSP multiplexer that provides go-to-definition, symbol search, and
//! save-notification over a Unix domain socket.

pub mod ctagd;
pub mod jsonrpc;
pub mod lsp;

pub use ctagd::{DaemonInfo, DefinitionResult, RustLspClient, SessionInfo, SymbolResult};
pub use lsp::{
    path_to_uri, pos_to_lsp_pos, uri_to_path, CompletionItem, CompletionResponse,
    FormattingOptions, InlayHint, InsertReplaceEdit, Location, LocationLink, LspManager,
    LspMessage, OffsetEncoding, Position, Range, SignatureHelpState, TextEdit,
};
