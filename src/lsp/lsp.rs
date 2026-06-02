// lsp.rs
// ──────────────────────────────────────────────────────────────
// Non‑blocking LSP implementation with select! loop and pending requests.
// Protocol types, helpers, and conversion functions are included.
// ──────────────────────────────────────────────────────────────

use std::collections::HashMap;
use std::path::PathBuf;
use std::process::Stdio;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::process::Child;
use tokio::sync::{mpsc, oneshot};
use tokio::time::Duration;

use ropey::Rope;

use crate::ed::buffer::Diagnostic;
use crate::msgbox::{AppMessage, AppSender};

#[cfg(feature = "debug_log")]
use crate::dlog;
#[cfg(feature = "debug_log")]
use std::io::Write;

// ============================================================================
// LSP Protocol Types (from /tmp version)
// ============================================================================

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TextDocumentSyncKind {
    None = 0,
    Full = 1,
    Incremental = 2,
}

#[derive(Debug, Clone)]
pub struct TextDocumentSyncOptions {
    pub open_close: bool,
    pub change: Option<TextDocumentSyncKind>,
    pub will_save: bool,
    pub will_save_wait_until: bool,
    pub save: Option<TextDocumentSyncSaveOptions>,
}

#[derive(Debug, Clone)]
pub enum TextDocumentSyncSaveOptions {
    Supported(bool),
    SaveOptions { include_text: bool },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DocumentFormattingParams {
    pub text_document: TextDocumentIdentifier,
    pub options: FormattingOptions,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FormattingOptions {
    pub tab_size: u32,
    pub insert_spaces: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub trim_trailing_whitespace: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub insert_final_newline: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub trim_final_newlines: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InlayHintParams {
    pub text_document: TextDocumentIdentifier,
    pub range: Range,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InlayHint {
    pub position: Position,
    pub label: InlayHintLabel,
    pub kind: Option<u32>,
    pub tooltip: Option<Value>,
    pub padding_left: Option<bool>,
    pub padding_right: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum InlayHintLabel {
    String(String),
    Parts(Vec<InlayHintLabelPart>),
}

impl InlayHintLabel {
    pub fn to_string(&self) -> String {
        match self {
            InlayHintLabel::String(s) => s.clone(),
            InlayHintLabel::Parts(parts) => parts.iter().map(|p| p.value.clone()).collect(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InlayHintLabelPart {
    pub value: String,
    pub tooltip: Option<Value>,
    pub location: Option<Value>,
    pub command: Option<Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LocationLink {
    pub target_uri: String,
    pub target_range: Range,
    pub target_selection_range: Range,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Location {
    pub uri: String,
    pub range: Range,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum DefinitionResult {
    Single(Location),
    Multiple(Vec<Location>),
    None,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SignatureHelpParams {
    pub text_document: TextDocumentIdentifier,
    pub position: Position,
    pub context: Option<SignatureHelpContext>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SignatureHelpContext {
    pub trigger_kind: u32,
    pub trigger_character: Option<String>,
    pub is_retrigger: bool,
    pub active_signature_help: Option<SignatureHelp>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SignatureHelp {
    pub signatures: Vec<SignatureInformation>,
    pub active_signature: Option<u32>,
    pub active_parameter: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SignatureInformation {
    pub label: String,
    pub documentation: Option<Value>,
    pub parameters: Option<Vec<ParameterInformation>>,
    pub active_parameter: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ParameterInformation {
    pub label: ParameterLabel,
    pub documentation: Option<Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ParameterLabel {
    Offsets([u32; 2]),
    String(String),
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum OffsetEncoding {
    Utf8,
    Utf16, // Default per LSP spec
    Utf32,
    #[default]
    Unknown,
}

impl OffsetEncoding {
    pub fn from_capability(encoding: Option<&String>) -> Self {
        match encoding.map(|s| s.as_str()) {
            Some("utf-8") => OffsetEncoding::Utf8,
            Some("utf-16") => OffsetEncoding::Utf16,
            Some("utf-32") => OffsetEncoding::Utf32,
            _ => OffsetEncoding::Utf16,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GeneralClientCapabilities {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub position_encodings: Option<Vec<String>>,
}

#[derive(Debug, Clone)]
pub struct SignatureHelpState {
    pub full_label: String,
    pub params: Vec<String>,
    pub active_param: usize,
    pub signatures: Option<Vec<SignatureInformation>>,
    pub active_signature: usize,
}

impl SignatureHelpState {
    pub fn format_for_infobar(&self) -> String {
        if self.params.is_empty() {
            return self.full_label.clone();
        }
        let formatted_params: Vec<String> = self
            .params
            .iter()
            .enumerate()
            .map(|(i, p)| {
                if i == self.active_param {
                    format!("[{}]", p)
                } else {
                    p.clone()
                }
            })
            .collect();
        if let Some(open) = self.full_label.find('(') {
            let prefix = &self.full_label[..=open];
            let suffix = self
                .full_label
                .rfind(')')
                .map(|i| &self.full_label[i..])
                .unwrap_or(")");
            format!("{}{}{}", prefix, formatted_params.join(", "), suffix)
        } else {
            formatted_params.join(", ")
        }
    }

    pub fn to_multiline_popup(&self) -> Vec<String> {
        let mut lines = Vec::new();
        if let Some(signatures) = &self.signatures {
            for (idx, sig) in signatures.iter().enumerate() {
                let prefix = if idx == self.active_signature {
                    "→ "
                } else {
                    "  "
                };
                lines.push(format!("{}{}", prefix, sig.label));
            }
        } else {
            lines.push(self.full_label.clone());
            if self.active_param < self.params.len() {
                lines.push(format!("  Parameter: {}", self.params[self.active_param]));
            }
        }
        lines
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum CompletionTextEdit {
    InsertReplaceEdit(InsertReplaceEdit),
    TextEdit(TextEdit),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InsertReplaceEdit {
    pub new_text: String,
    pub insert: Range,
    pub replace: Range,
}

impl CompletionTextEdit {
    pub fn new_text(&self) -> String {
        match self {
            CompletionTextEdit::TextEdit(te) => te.new_text.clone(),
            CompletionTextEdit::InsertReplaceEdit(ire) => ire.new_text.clone(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct CompletionItem {
    pub label: String,
    pub detail: Option<String>,
    pub documentation: Option<Value>,
    pub insert_text: Option<String>,
    pub insert_text_format: Option<u32>, // 1 = PlainText, 2 = Snippet
    pub text_edit: Option<CompletionTextEdit>,
    #[serde(default)]
    pub additional_text_edits: Vec<TextEdit>,
    pub kind: Option<u32>,
    pub filter_text: Option<String>,
    pub sort_text: Option<String>,
    pub data: Option<Value>,
}

impl CompletionItem {
    /// Get the text to insert, handling both `insert_text` and `text_edit`
    pub fn get_insert_text(&self) -> Option<String> {
        if let Some(edit) = &self.text_edit {
            return Some(edit.new_text());
        }
        self.insert_text.clone()
    }

    /// Check if this is a snippet completion
    pub fn is_snippet(&self) -> bool {
        self.insert_text_format == Some(2)
    }

    /// Get additional text edits (for import insertion, etc.)
    pub fn get_additional_edits(&self) -> &[TextEdit] {
        &self.additional_text_edits
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TextEdit {
    pub range: Range,
    pub new_text: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum CompletionResponse {
    Array(Vec<CompletionItem>),
    List(CompletionList),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CompletionList {
    pub is_incomplete: bool,
    pub items: Vec<CompletionItem>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InitializeParams {
    pub process_id: Option<u32>,
    pub root_path: Option<String>,
    pub root_uri: Option<String>,
    pub initialization_options: Option<Value>,
    pub capabilities: ClientCapabilities,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ClientCapabilities {
    pub text_document: TextDocumentCapabilities,
    pub workspace: WorkspaceCapabilities,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub general: Option<GeneralClientCapabilities>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TextDocumentCapabilities {
    pub synchronization: Option<TextDocumentSyncCapabilities>,
    pub publish_diagnostics: Option<Value>,
    pub signature_help: Option<SignatureHelpCapability>,
    pub completion: Option<CompletionCapability>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CompletionCapability {
    pub completion_item: Option<Value>,
    pub context_support: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SignatureHelpCapability {
    pub dynamic_registration: Option<bool>,
    pub signature_information: Option<SignatureInformationCapability>,
    pub context_support: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SignatureInformationCapability {
    pub documentation_format: Option<Vec<String>>,
    pub parameter_information: Option<ParameterInformationCapability>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ParameterInformationCapability {
    pub label_offset_support: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TextDocumentSyncCapabilities {
    pub dynamic_registration: Option<bool>,
    pub will_save: Option<bool>,
    pub will_save_wait_until: Option<bool>,
    pub did_save: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceCapabilities {
    pub did_change_configuration: Option<Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DidOpenTextDocumentParams {
    pub text_document: TextDocumentItem,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DidChangeTextDocumentParams {
    pub text_document: VersionedTextDocumentIdentifier,
    pub content_changes: Vec<TextDocumentContentChangeEvent>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DidSaveTextDocumentParams {
    pub text_document: TextDocumentIdentifier,
    pub text: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TextDocumentItem {
    pub uri: String,
    pub language_id: String,
    pub version: i32,
    pub text: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TextDocumentIdentifier {
    pub uri: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VersionedTextDocumentIdentifier {
    pub uri: String,
    pub version: i32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TextDocumentContentChangeEvent {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub range: Option<Range>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub range_length: Option<u32>,
    pub text: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Range {
    pub start: Position,
    pub end: Position,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Position {
    pub line: u32,
    pub character: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PublishDiagnosticsParams {
    pub uri: String,
    pub version: Option<i32>,
    pub diagnostics: Vec<LspDiagnostic>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LspDiagnostic {
    pub range: Range,
    pub severity: Option<u32>,
    pub code: Option<Value>,
    pub source: Option<String>,
    pub message: String,
}

// ============================================================================
// Message Bus
// ============================================================================

#[derive(Debug)]
pub enum LspMessage {
    OpenFile(PathBuf),
    CloseFile(PathBuf),
    ChangeFile(PathBuf, String, String, i32),
    ChangeFileIncremental {
        path: PathBuf,
        version: i32,
        start_line: u32,
        start_char: u32,
        end_line: u32,
        end_char: u32,
        new_text: String,
    },
    SaveFile(PathBuf),
    Shutdown,
    Error(String),
    RequestInlayHintsRange(PathBuf, usize, usize, i32),
    RequestSignatureHelp(PathBuf, u32, u32),
    RequestCompletion(PathBuf, u32, u32, Option<String>, u64 /* version */),
    ResolveCompletionItem(CompletionItem),
    GotoDefinition {
        path: PathBuf,
        line: u32,
        col: u32,
        respond_to: oneshot::Sender<Vec<Location>>,
    },
    RequestFormatting(
        PathBuf,
        String,
        FormattingOptions,
        usize,                  // buffer_idx
        Option<(usize, usize)>, // cursor_state (line, col)
        bool,                   // save_after
    ),
}

// ============================================================================
// Pending Request Kind (non‑blocking)
// ============================================================================

enum PendingKind {
    GotoDefinition(oneshot::Sender<Vec<Location>>),
    Formatting {
        buffer_idx: usize,
        cursor_state: Option<(usize, usize)>,
        save_after: bool,
    },
    InlayHints {
        uri: String,
        version: i32,
    },
    SignatureHelp,
    Completion {
        version: u64,
    },
    ResolveCompletion,
    Shutdown,
    Ignore,
}

// ============================================================================
// Non‑blocking LanguageServer
// ============================================================================

pub struct LanguageServer {
    process: Child,
    stdin: tokio::process::ChildStdin,
    /// All server messages arrive here (from background reader task)
    pub rx: mpsc::UnboundedReceiver<Value>,
    next_id: i32,
    pub offset_encoding: OffsetEncoding,
    pub supports_snippets: bool,
    pub supports_completion_resolve: bool,
}

impl LanguageServer {
    pub async fn new(
        command: &str,
        args: &[&str],
        root_uri: &str,
    ) -> Result<(Self, Value), Box<dyn std::error::Error + Send + Sync>> {
        let mut child = tokio::process::Command::new(command)
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()?;

        let stdin = child.stdin.take().unwrap();
        let stdout = child.stdout.take().unwrap();
        let (tx, rx) = mpsc::unbounded_channel();

        // Background task: drain stdout → channel
        tokio::spawn(async move {
            let mut reader = BufReader::new(stdout);
            loop {
                let mut content_length: Option<usize> = None;
                loop {
                    let mut line = String::new();
                    match reader.read_line(&mut line).await {
                        Ok(0) => return,
                        Ok(_) => {
                            if line == "\r\n" {
                                if let Some(len) = content_length {
                                    let mut buf = vec![0u8; len];
                                    if reader.read_exact(&mut buf).await.is_err() {
                                        return;
                                    }
                                    if let Ok(v) = serde_json::from_slice::<Value>(&buf) {
                                        if tx.send(v).is_err() {
                                            return;
                                        }
                                    }
                                    break;
                                }
                            } else if line.starts_with("Content-Length:") {
                                if let Some(s) = line.split(':').nth(1) {
                                    content_length = s.trim().parse().ok();
                                }
                            }
                        }
                        Err(_) => return,
                    }
                }
            }
        });

        let mut server = Self {
            process: child,
            stdin,
            rx,
            next_id: 0,
            offset_encoding: OffsetEncoding::default(),
            supports_snippets: false,
            supports_completion_resolve: false,
        };

        // Blocking initialize – only once at startup
        let init_response = server.blocking_request("initialize", json!(
            InitializeParams {
                process_id: Some(std::process::id()),
                root_path: Some(root_uri.trim_start_matches("file://").to_string()),
                root_uri: Some(root_uri.to_string()),
                initialization_options: Some(json!({
                    "rust-analyzer": {
                        "inlayHints": {
                            "bindingModeHints": true,
                            "closureReturnTypeHints": "always",
                            "lifetimeElisionHints": "always",
                            "parameterHints": true,
                            "reborrowHints": true,
                            "renderColons": true,
                            "typeHints": true,
                            "chainingHints": true
                        }
                    }
                })),
                capabilities: ClientCapabilities {
                    text_document: TextDocumentCapabilities {
                        synchronization: Some(TextDocumentSyncCapabilities {
                            dynamic_registration: Some(false),
                            will_save: Some(true),
                            will_save_wait_until: Some(false),
                            did_save: Some(true),
                        }),
                        publish_diagnostics: Some(json!({})),
                        signature_help: Some(SignatureHelpCapability {
                            dynamic_registration: Some(false),
                            context_support: Some(true),
                            signature_information: Some(SignatureInformationCapability {
                                documentation_format: Some(vec!["plaintext".into()]),
                                parameter_information: Some(ParameterInformationCapability {
                                    label_offset_support: Some(true),
                                }),
                            }),
                        }),
                        completion: Some(CompletionCapability {
                            completion_item: Some(json!({
                                "snippetSupport": true,
                                "resolveSupport": {
                                    "properties": ["documentation", "detail", "additionalTextEdits"]
                                },
                                "insertReplaceSupport": false,
                            })),
                            context_support: Some(true),
                        }),
                    },
                    workspace: WorkspaceCapabilities {
                        did_change_configuration: Some(json!({})),
                    },
                    general: Some(GeneralClientCapabilities {
                        position_encodings: Some(vec![
                            "utf-8".to_string(),
                            "utf-16".to_string(),
                            "utf-32".to_string(),
                        ]),
                    }),
                },
            }
        )).await?;

        // Parse capabilities
        if let Some(enc) = init_response
            .get("capabilities")
            .and_then(|c| c.get("positionEncoding"))
            .and_then(|e| e.as_str())
        {
            server.offset_encoding = match enc {
                "utf-8" => OffsetEncoding::Utf8,
                "utf-32" => OffsetEncoding::Utf32,
                _ => OffsetEncoding::Utf16,
            };
        }

        server.supports_snippets = init_response
            .get("capabilities")
            .and_then(|c| c.get("completionProvider"))
            .and_then(|cp| cp.get("completionItem"))
            .and_then(|ci| ci.get("snippetSupport"))
            .and_then(|s| s.as_bool())
            .unwrap_or(false);

        server.supports_completion_resolve = init_response
            .get("capabilities")
            .and_then(|c| c.get("completionProvider"))
            .map(|cp| {
                cp.get("resolveProvider")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false)
                    || cp
                        .get("resolveSupport")
                        .and_then(|rs| rs.get("properties"))
                        .and_then(|p| p.as_array())
                        .map(|arr| {
                            arr.iter().any(|x| {
                                x.as_str() == Some("documentation") || x.as_str() == Some("detail")
                            })
                        })
                        .unwrap_or(false)
            })
            .unwrap_or(false);

        server.notify("initialized", json!({})).await?;
        Ok((server, init_response))
    }

    pub async fn did_change_incremental(
        &mut self,
        uri: &str,
        version: i32,
        start_line: u32,
        start_char: u32,
        end_line: u32,
        end_char: u32,
        new_text: &str,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        self.notify(
            "textDocument/didChange",
            json!(DidChangeTextDocumentParams {
                text_document: VersionedTextDocumentIdentifier {
                    uri: uri.to_string(),
                    version,
                },
                content_changes: vec![TextDocumentContentChangeEvent {
                    range: Some(Range {
                        start: Position {
                            line: start_line,
                            character: start_char
                        },
                        end: Position {
                            line: end_line,
                            character: end_char
                        },
                    }),
                    range_length: None, // Deprecated per LSP 3.16+
                    text: new_text.to_string(),
                }],
            }),
        )
        .await
    }

    /// Assign a new request id and return it.
    pub fn next_id(&mut self) -> i32 {
        self.next_id += 1;
        self.next_id
    }

    /// Send a JSON‑RPC message without waiting for a response.
    pub async fn send_raw(
        &mut self,
        msg: &Value,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let content = serde_json::to_string(msg)?;
        let header = format!("Content-Length: {}\r\n\r\n", content.len());
        self.stdin.write_all(header.as_bytes()).await?;
        self.stdin.write_all(content.as_bytes()).await?;
        self.stdin.flush().await?;
        Ok(())
    }

    /// Send a request, assign an id, and return the id.
    pub async fn send_request(
        &mut self,
        method: &str,
        params: Value,
    ) -> Result<i32, Box<dyn std::error::Error + Send + Sync>> {
        let id = self.next_id();
        self.send_raw(&json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params,
        }))
        .await?;
        Ok(id)
    }

    /// Fire‑and‑forget notification.
    pub async fn notify(
        &mut self,
        method: &str,
        params: Value,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        self.send_raw(&json!({
            "jsonrpc": "2.0",
            "method": method,
            "params": params,
        }))
        .await
    }

    /// Used ONLY during startup (initialize) – blocks waiting for the response.
    async fn blocking_request(
        &mut self,
        method: &str,
        params: Value,
    ) -> Result<Value, Box<dyn std::error::Error + Send + Sync>> {
        let id = self.next_id();
        self.send_raw(&json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params,
        }))
        .await?;

        let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
        loop {
            match tokio::time::timeout_at(deadline, self.rx.recv()).await {
                Ok(Some(msg)) => {
                    if msg.get("id").and_then(|v| v.as_i64()) == Some(id as i64) {
                        if let Some(e) = msg.get("error") {
                            return Err(e.to_string().into());
                        }
                        return Ok(msg.get("result").cloned().unwrap_or(json!(null)));
                    }
                    // notification during init – silently drop
                }
                Ok(None) => return Err("LSP closed during initialize".into()),
                Err(_) => return Err("initialize timed out".into()),
            }
        }
    }

    pub async fn shutdown(&mut self) {
        let id = self.next_id();
        let _ = self
            .send_raw(&json!({"jsonrpc":"2.0","id":id,"method":"shutdown"}))
            .await;
        let _ = self.notify("exit", json!({})).await;
        let _ = self.process.kill().await;
        let _ = self.process.wait().await;
    }

    // Convenience: build common notifications

    pub async fn did_open(
        &mut self,
        uri: &str,
        language_id: &str,
        text: &str,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        self.notify(
            "textDocument/didOpen",
            json!(DidOpenTextDocumentParams {
                text_document: TextDocumentItem {
                    uri: uri.to_string(),
                    language_id: language_id.to_string(),
                    version: 1,
                    text: text.to_string(),
                },
            }),
        )
        .await
    }

    pub async fn did_change(
        &mut self,
        uri: &str,
        new_text: &str,
        version: i32,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        self.notify(
            "textDocument/didChange",
            json!(DidChangeTextDocumentParams {
                text_document: VersionedTextDocumentIdentifier {
                    uri: uri.to_string(),
                    version,
                },
                content_changes: vec![TextDocumentContentChangeEvent {
                    range: None,
                    range_length: None,
                    text: new_text.to_string(),
                }],
            }),
        )
        .await
    }

    pub async fn did_save(
        &mut self,
        uri: &str,
        text: Option<&str>,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        self.notify(
            "textDocument/didSave",
            json!(DidSaveTextDocumentParams {
                text_document: TextDocumentIdentifier {
                    uri: uri.to_string()
                },
                text: text.map(|s| s.to_string()),
            }),
        )
        .await
    }
}

// ============================================================================
// LspManager – pure select! loop, no blocking awaits
// ============================================================================

pub struct LspManager {
    pub active_lsp: Option<LanguageServer>,
    lsp_rx: mpsc::UnboundedReceiver<LspMessage>,
    lsp_tx: mpsc::UnboundedSender<LspMessage>,
    app_tx: AppSender,
    opened_files: std::collections::HashSet<String>,
    current_file_version: HashMap<String, i32>,
    pending_hint_requests: HashMap<String, i32>,
    pending: HashMap<i32, PendingKind>,
    pub supports_snippets: bool,
    pub supports_completion_resolve: bool,
}

impl LspManager {
    pub fn new(app_tx: AppSender) -> Self {
        let (lsp_tx, lsp_rx) = mpsc::unbounded_channel();
        Self {
            active_lsp: None,
            lsp_rx,
            lsp_tx,
            app_tx,
            opened_files: std::collections::HashSet::new(),
            current_file_version: HashMap::new(),
            pending_hint_requests: HashMap::new(),
            pending: HashMap::new(),
            supports_snippets: false,
            supports_completion_resolve: false,
        }
    }

    pub fn get_sender(&self) -> mpsc::UnboundedSender<LspMessage> {
        self.lsp_tx.clone()
    }

    pub async fn run(&mut self) {
        loop {
            if let Some(lsp) = &mut self.active_lsp {
                tokio::select! {
                    msg = self.lsp_rx.recv() => {
                        match msg {
                            Some(m) => self.dispatch_editor_msg(m).await,
                            None    => break,
                        }
                    }
                    msg = lsp.rx.recv() => {
                        match msg {
                            Some(v) => self.handle_server_msg(v).await,

                    None => {
                        // server died — clear all pending as errors
                        let pending: Vec<_> = self.pending.drain().map(|(_, kind)| kind).collect();
                        for kind in pending {
                            self.reject_pending(kind, "LSP server closed");
                        }
                        self.active_lsp = None;
                    }
                        }
                    }
                }
            } else {
                match self.lsp_rx.recv().await {
                    Some(m) => self.dispatch_editor_msg(m).await,
                    None => break,
                }
            }
        }
    }

    async fn handle_server_msg(&mut self, msg: Value) {
        // Response to a pending request?
        if let Some(id_val) = msg.get("id") {
            if let Some(id) = id_val.as_i64().map(|v| v as i32) {
                if let Some(kind) = self.pending.remove(&id) {
                    self.resolve_pending(kind, &msg).await;
                    return;
                }
            }
        }

        // Server‑push notification
        if let Some(method) = msg.get("method").and_then(|m| m.as_str()) {
            if method == "textDocument/publishDiagnostics" {
                if let Some(params) = msg.get("params") {
                    if let Ok(dp) =
                        serde_json::from_value::<PublishDiagnosticsParams>(params.clone())
                    {
                        let _ = self.app_tx.send(AppMessage::LspDiagnostics {
                            uri: dp.uri.clone(),
                            version: dp.version,
                            diagnostics: convert_lsp_diagnostics(&dp),
                        });
                    }
                }
            }
        }
    }

    async fn resolve_pending(&mut self, kind: PendingKind, msg: &Value) {
        let result = msg.get("result").cloned().unwrap_or(json!(null));
        let is_error = msg.get("error").is_some();

        match kind {
            PendingKind::GotoDefinition(tx) => {
                let locations = if is_error || result.is_null() {
                    Vec::new()
                } else {
                    // try single Location
                    if let Ok(loc) = serde_json::from_value::<Location>(result.clone()) {
                        vec![loc]
                    }
                    // try array of Locations
                    else if let Ok(locs) = serde_json::from_value::<Vec<Location>>(result.clone())
                    {
                        locs
                    }
                    // (optional) support LocationLink array
                    else if let Ok(links) =
                        serde_json::from_value::<Vec<crate::lsp::LocationLink>>(result.clone())
                    {
                        links
                            .into_iter()
                            .map(|link| Location {
                                uri: link.target_uri,
                                range: link.target_range,
                            })
                            .collect()
                    } else {
                        Vec::new()
                    }
                };
                let _ = tx.send(locations);
            }
            PendingKind::Formatting {
                buffer_idx,
                cursor_state,
                save_after,
            } => {
                let result = if is_error {
                    let err = msg["error"]["message"]
                        .as_str()
                        .unwrap_or("unknown")
                        .to_string();
                    Err(err)
                } else if result.is_null() {
                    Ok(None)
                } else {
                    serde_json::from_value::<Vec<TextEdit>>(result)
                        .map(Some)
                        .map_err(|e| e.to_string())
                };
                let _ = self.app_tx.send(AppMessage::LspFormatResult {
                    result,
                    buffer_idx,
                    cursor_state,
                    save_after,
                });
            }
            PendingKind::InlayHints { uri, version } => {
                if !is_error {
                    if let Ok(hints) = serde_json::from_value::<Vec<InlayHint>>(result) {
                        let _ = self.app_tx.send(AppMessage::LspInlayHints {
                            uri,
                            hints,
                            version,
                        });
                    }
                }
            }
            PendingKind::SignatureHelp => {
                let state = if is_error || result.is_null() {
                    None
                } else {
                    parse_signature_help(result)
                };
                let _ = self.app_tx.send(AppMessage::LspSignatureHelp(state));
            }
            // ── resolve_pending ─────────────────────────────────────────────
            PendingKind::Completion { version } => {
                let items = if is_error || result.is_null() {
                    None
                } else {
                    serde_json::from_value::<CompletionResponse>(result)
                        .ok()
                        .map(|r| match r {
                            CompletionResponse::Array(a) => a,
                            CompletionResponse::List(l) => l.items,
                        })
                };
                let _ = self
                    .app_tx
                    .send(AppMessage::LspCompletion { items, version });
            }
            PendingKind::ResolveCompletion => {
                if !is_error {
                    if let Ok(item) = serde_json::from_value::<CompletionItem>(result) {
                        let _ = self.app_tx.send(AppMessage::LspCompletionResolved(item));
                    }
                }
            }
            PendingKind::Shutdown | PendingKind::Ignore => {}
        }
    }

    fn reject_pending(&self, kind: PendingKind, _reason: &str) {
        match kind {
            PendingKind::GotoDefinition(tx) => {
                let _ = tx.send(Vec::new());
            }
            PendingKind::Formatting {
                buffer_idx,
                cursor_state,
                save_after,
            } => {
                let _ = self.app_tx.send(AppMessage::LspFormatResult {
                    result: Err("LSP disconnected".into()),
                    buffer_idx,
                    cursor_state,
                    save_after,
                });
            }
            PendingKind::SignatureHelp => {
                let _ = self.app_tx.send(AppMessage::LspSignatureHelp(None));
            }
            _ => {}
        }
    }

    async fn dispatch_editor_msg(&mut self, msg: LspMessage) {
        match msg {
            LspMessage::GotoDefinition {
                path,
                line,
                col,
                respond_to,
            } => {
                let Some(lsp) = &mut self.active_lsp else {
                    let _ = respond_to.send(Vec::new());
                    return;
                };
                let uri = path_to_uri(&path);
                if !self.opened_files.contains(&uri) {
                    let _ = respond_to.send(Vec::new());
                    return;
                }
                match lsp
                    .send_request(
                        "textDocument/definition",
                        json!({
                            "textDocument": { "uri": uri },
                            "position": { "line": line, "character": col },
                        }),
                    )
                    .await
                {
                    Ok(id) => {
                        self.pending
                            .insert(id, PendingKind::GotoDefinition(respond_to));
                    }
                    Err(_) => {
                        let _ = respond_to.send(Vec::new());
                    }
                }
            }
            LspMessage::ChangeFileIncremental {
                path,
                version,
                start_line,
                start_char,
                end_line,
                end_char,
                new_text,
            } => {
                if let Some(lsp) = &mut self.active_lsp {
                    let uri = path_to_uri(&path);
                    self.current_file_version.insert(uri.clone(), version);
                    let _ = self.app_tx.send(AppMessage::LspDiagnostics {
                        uri: uri.clone(),
                        version: Some(version),
                        diagnostics: vec![],
                    });
                    let _ = lsp
                        .did_change_incremental(
                            &uri, version, start_line, start_char, end_line, end_char, &new_text,
                        )
                        .await;
                }
            }

            LspMessage::OpenFile(path) => {
                if let Some(lsp) = &mut self.active_lsp {
                    let uri = path_to_uri(&path);
                    let lang = crate::file_lang::FileLang::from_path(&path);
                    if let Ok(text) = std::fs::read_to_string(&path) {
                        let _ = lsp.did_open(&uri, lang.language_id, &text).await;
                        self.opened_files.insert(uri.clone());
                        self.current_file_version.insert(uri, 1);
                    }
                } else {
                    self.start_lsp_for_file(&path).await;
                }
            }

            LspMessage::CloseFile(path) => {
                if let Some(lsp) = &mut self.active_lsp {
                    let uri = path_to_uri(&path);
                    self.opened_files.remove(&uri);
                    let _ = lsp
                        .notify(
                            "textDocument/didClose",
                            json!({ "textDocument": { "uri": uri } }),
                        )
                        .await;
                }
            }

            LspMessage::ChangeFile(path, _old_text, new_text, version) => {
                if let Some(lsp) = &mut self.active_lsp {
                    let uri = path_to_uri(&path);
                    self.current_file_version.insert(uri.clone(), version);
                    // Optimistically clear diagnostics
                    let _ = self.app_tx.send(AppMessage::LspDiagnostics {
                        uri: uri.clone(),
                        version: Some(version),
                        diagnostics: vec![],
                    });
                    let _ = lsp.did_change(&uri, &new_text, version).await;
                }
            }

            LspMessage::SaveFile(path) => {
                if let Some(lsp) = &mut self.active_lsp {
                    let uri = path_to_uri(&path);
                    let _ = lsp.did_save(&uri, None).await;
                }
            }

            LspMessage::RequestFormatting(
                path,
                text,
                options,
                buffer_idx,
                cursor_state,
                save_after,
            ) => {
                let Some(lsp) = &mut self.active_lsp else {
                    let _ = self.app_tx.send(AppMessage::LspFormatResult {
                        result: Err("No active LSP".into()),
                        buffer_idx,
                        cursor_state,
                        save_after,
                    });
                    return;
                };
                let uri = path_to_uri(&path);
                let version = {
                    let v = self.current_file_version.entry(uri.clone()).or_insert(0);
                    *v += 1;
                    *v
                };
                if !self.opened_files.contains(&uri) {
                    let lang = crate::file_lang::FileLang::from_path(&path);
                    let _ = lsp.did_open(&uri, lang.language_id, &text).await;
                    self.opened_files.insert(uri.clone());
                } else {
                    let _ = lsp.did_change(&uri, &text, version).await;
                }
                let _ = lsp.did_save(&uri, Some(&text)).await;

                // Small delay for server to process the save before formatting
                tokio::time::sleep(Duration::from_millis(150)).await;

                match lsp
                    .send_request(
                        "textDocument/formatting",
                        json!(DocumentFormattingParams {
                            text_document: TextDocumentIdentifier { uri: uri.clone() },
                            options,
                        }),
                    )
                    .await
                {
                    Ok(id) => {
                        self.pending.insert(
                            id,
                            PendingKind::Formatting {
                                buffer_idx,
                                cursor_state,
                                save_after,
                            },
                        );
                    }
                    Err(e) => {
                        let _ = self.app_tx.send(AppMessage::LspFormatResult {
                            result: Err(e.to_string()),
                            buffer_idx,
                            cursor_state,
                            save_after,
                        });
                    }
                }
            }

            LspMessage::RequestInlayHintsRange(path, start_line, end_line, version) => {
                let Some(lsp) = &mut self.active_lsp else {
                    return;
                };
                let uri = path_to_uri(&path);
                if !self.opened_files.contains(&uri) {
                    return;
                }
                if let Some(&cv) = self.current_file_version.get(&uri) {
                    if version < cv {
                        return;
                    }
                }
                if let Some(&pv) = self.pending_hint_requests.get(&uri) {
                    if pv >= version {
                        return;
                    }
                }
                self.pending_hint_requests.insert(uri.clone(), version);
                let params = json!(InlayHintParams {
                    text_document: TextDocumentIdentifier { uri: uri.clone() },
                    range: Range {
                        start: Position {
                            line: start_line as u32,
                            character: 0
                        },
                        end: Position {
                            line: end_line as u32,
                            character: 0
                        },
                    },
                });
                if let Ok(id) = lsp.send_request("textDocument/inlayHint", params).await {
                    self.pending
                        .insert(id, PendingKind::InlayHints { uri, version });
                }
            }

            LspMessage::RequestSignatureHelp(path, line, character) => {
                let Some(lsp) = &mut self.active_lsp else {
                    let _ = self.app_tx.send(AppMessage::LspSignatureHelp(None));
                    return;
                };
                let uri = path_to_uri(&path);
                if !self.opened_files.contains(&uri) {
                    let _ = self.app_tx.send(AppMessage::LspSignatureHelp(None));
                    return;
                }
                let params = json!(SignatureHelpParams {
                    text_document: TextDocumentIdentifier { uri },
                    position: Position { line, character },
                    context: Some(SignatureHelpContext {
                        trigger_kind: 1,
                        trigger_character: None,
                        is_retrigger: false,
                        active_signature_help: None,
                    }),
                });
                if let Ok(id) = lsp.send_request("textDocument/signatureHelp", params).await {
                    self.pending.insert(id, PendingKind::SignatureHelp);
                }
            }

            // ── LspManager::dispatch_editor_msg ─────────────────────────────
            LspMessage::RequestCompletion(path, line, character, trigger, version) => {
                let Some(lsp) = &mut self.active_lsp else {
                    // FIX: always respond so CompletionMachine can transition
                    let _ = self.app_tx.send(AppMessage::LspCompletion {
                        items: None,
                        version,
                    });
                    return;
                };
                let uri = path_to_uri(&path);
                if !self.opened_files.contains(&uri) {
                    // FIX: same here
                    let _ = self.app_tx.send(AppMessage::LspCompletion {
                        items: None,
                        version,
                    });
                    return;
                }
                let context = if let Some(ref ch) = trigger {
                    json!({ "triggerKind": 2, "triggerCharacter": ch })
                } else {
                    json!({ "triggerKind": 1 })
                };
                let params = json!({
                    "textDocument": { "uri": uri },
                    "position": { "line": line, "character": character },
                    "context": context,
                });
                if let Ok(id) = lsp.send_request("textDocument/completion", params).await {
                    self.pending.insert(id, PendingKind::Completion { version });
                }
            }

            LspMessage::ResolveCompletionItem(item) => {
                let Some(lsp) = &mut self.active_lsp else {
                    return;
                };
                if !lsp.supports_completion_resolve || item.data.is_none() {
                    return;
                }
                if let Ok(id) = lsp
                    .send_request("completionItem/resolve", json!(item))
                    .await
                {
                    self.pending.insert(id, PendingKind::ResolveCompletion);
                }
            }

            LspMessage::Shutdown => {
                if let Some(mut lsp) = self.active_lsp.take() {
                    lsp.shutdown().await;
                }
            }

            LspMessage::Error(e) => {
                let _ = self.app_tx.send(AppMessage::LspError(e));
            }
        }
    }

    async fn start_lsp_for_file(&mut self, path: &PathBuf) {
        let lang = crate::file_lang::FileLang::from_path(path);
        let command = match lang.lsp_command {
            Some(cmd) => cmd,
            None => return,
        };
        let args = lang.lsp_args;
        let root_uri = format!(
            "file://{}",
            std::env::current_dir().unwrap_or_default().display()
        );

        match LanguageServer::new(command, args, &root_uri).await {
            Ok((mut lsp, _)) => {
                self.supports_snippets = lsp.supports_snippets;
                self.supports_completion_resolve = lsp.supports_completion_resolve;
                let uri = path_to_uri(path);
                let lang2 = crate::file_lang::FileLang::from_path(path);
                if let Ok(text) = std::fs::read_to_string(path) {
                    if lsp.did_open(&uri, lang2.language_id, &text).await.is_ok() {
                        self.opened_files.insert(uri.clone());
                        self.current_file_version.insert(uri, 1);
                    }
                }
                self.active_lsp = Some(lsp);
            }
            Err(e) => {
                let _ = self.app_tx.send(AppMessage::LspError(format!(
                    "Failed to start {}: {}",
                    command, e
                )));
            }
        }
    }
}

// ============================================================================
// Pure Helpers (no I/O)
// ============================================================================

fn parse_signature_help(result: Value) -> Option<SignatureHelpState> {
    let help: SignatureHelp = serde_json::from_value(result).ok()?;
    let active_sig = help.active_signature.unwrap_or(0) as usize;
    let sig = help.signatures.get(active_sig)?;
    let active_param = sig.active_parameter.or(help.active_parameter).unwrap_or(0) as usize;
    let params_list = sig.parameters.as_deref().unwrap_or(&[]);
    let param_labels: Vec<String> = params_list
        .iter()
        .map(|p| match &p.label {
            ParameterLabel::String(s) => s.clone(),
            ParameterLabel::Offsets([start, end]) => sig
                .label
                .get(*start as usize..*end as usize)
                .unwrap_or("")
                .to_string(),
        })
        .collect();
    Some(SignatureHelpState {
        full_label: sig.label.clone(),
        params: param_labels,
        active_param,
        signatures: Some(help.signatures),
        active_signature: active_sig,
    })
}

fn convert_lsp_diagnostics(params: &PublishDiagnosticsParams) -> Vec<Diagnostic> {
    params
        .diagnostics
        .iter()
        .map(|d| Diagnostic {
            line: d.range.start.line as usize,
            start_col: d.range.start.character as usize,
            end_col: d.range.end.character as usize,
            message: d.message.clone(),
            is_error: d.severity == Some(1),
            end_line: d.range.end.line as usize,
            severity: d.severity.unwrap_or(1),
        })
        .collect()
}

fn detect_language_from_path(path: &PathBuf) -> String {
    crate::file_lang::FileLang::from_path(path)
        .language_id
        .to_string()
}

pub fn path_to_uri(path: &PathBuf) -> String {
    let absolute_path = if path.is_absolute() {
        path.clone()
    } else {
        std::env::current_dir()
            .unwrap_or_else(|_| PathBuf::from("."))
            .join(path)
    };

    let canonical = absolute_path.canonicalize().unwrap_or(absolute_path);

    #[cfg(target_os = "windows")]
    {
        let path_str = canonical.to_string_lossy().replace('\\', "/");
        if path_str.chars().nth(1) == Some(':') {
            format!("file:///{}", path_str)
        } else {
            format!("file://{}", path_str)
        }
    }

    #[cfg(not(target_os = "windows"))]
    {
        format!("file://{}", canonical.display())
    }
}

pub fn uri_to_path(uri: &str) -> PathBuf {
    let without_protocol = uri.strip_prefix("file://").unwrap_or(uri);
    PathBuf::from(without_protocol)
}

/// Convert buffer position (char index) to LSP Position respecting encoding.
pub fn pos_to_lsp_pos(doc: &Rope, char_idx: usize, encoding: OffsetEncoding) -> Position {
    let line = doc.char_to_line(char_idx);
    let line_start_char = doc.line_to_char(line);
    let char_offset = char_idx - line_start_char;

    let character = match encoding {
        OffsetEncoding::Utf8 => {
            let line_start_byte = doc.line_to_byte(line);
            let char_byte = doc.char_to_byte(char_idx);
            (char_byte - line_start_byte) as u32
        }
        OffsetEncoding::Utf16 => {
            let line_start_utf16 = doc.char_to_utf16_cu(line_start_char);
            let char_utf16 = doc.char_to_utf16_cu(char_idx);
            (char_utf16 - line_start_utf16) as u32
        }
        OffsetEncoding::Utf32 => char_offset as u32,
        OffsetEncoding::Unknown => char_offset as u32,
    };

    Position {
        line: line as u32,
        character,
    }
}

/// Convert LSP Position back to buffer char index.
pub fn lsp_pos_to_char_idx(doc: &Rope, pos: &Position, encoding: OffsetEncoding) -> Option<usize> {
    let line = pos.line as usize;
    if line >= doc.len_lines() {
        return Some(doc.len_chars());
    }

    let line_start_char = doc.line_to_char(line);
    let line = doc.line(line);

    let line_ends_with_newline = |line_str: &str| -> (bool, usize) {
        if line_str.is_empty() {
            return (false, 0);
        }
        let bytes = line_str.len();
        let mut newline_bytes = 0;
        if line_str.ends_with('\n') {
            newline_bytes += 1;
            if line_str.len() > 1 && line_str.ends_with("\r\n") {
                newline_bytes += 1;
            }
        }
        (newline_bytes > 0, newline_bytes)
    };

    let line_str = line.to_string();
    let (has_newline, newline_len) = line_ends_with_newline(&line_str);

    let line_end_units = match encoding {
        OffsetEncoding::Utf8 => {
            let line_bytes = line.len_bytes();
            if has_newline {
                line_bytes - newline_len
            } else {
                line_bytes
            }
        }
        OffsetEncoding::Utf16 => {
            let line_utf16 = line.len_utf16_cu();
            if has_newline {
                line_utf16 - newline_len
            } else {
                line_utf16
            }
        }
        OffsetEncoding::Utf32 => {
            let count = line.len_chars();
            if has_newline {
                count - newline_len
            } else {
                count
            }
        }
        OffsetEncoding::Unknown => line.len_chars(),
    };

    let capped_offset = (pos.character as usize).min(line_end_units);

    match encoding {
        OffsetEncoding::Utf8 => {
            let line_start_byte = doc.line_to_byte(pos.line as usize);
            doc.try_byte_to_char(line_start_byte + capped_offset).ok()
        }
        OffsetEncoding::Utf16 => {
            let line_start_utf16 = doc.char_to_utf16_cu(line_start_char);
            doc.try_utf16_cu_to_char(line_start_utf16 + capped_offset)
                .ok()
        }
        OffsetEncoding::Utf32 | OffsetEncoding::Unknown => Some(line_start_char + capped_offset),
    }
}

/// Convert a buffer range to LSP Range.
pub fn range_to_lsp_range(
    doc: &Rope,
    start_char: usize,
    end_char: usize,
    encoding: OffsetEncoding,
) -> Range {
    Range {
        start: pos_to_lsp_pos(doc, start_char, encoding),
        end: pos_to_lsp_pos(doc, end_char, encoding),
    }
}

/// Convert LSP Range back to buffer char indices.
pub fn lsp_range_to_char_indices(
    doc: &Rope,
    range: &Range,
    encoding: OffsetEncoding,
) -> Option<(usize, usize)> {
    let start = lsp_pos_to_char_idx(doc, &range.start, encoding)?;
    let end = lsp_pos_to_char_idx(doc, &range.end, encoding)?;
    Some((start, end))
}
