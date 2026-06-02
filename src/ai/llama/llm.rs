// File: src/ai/llama/llm.rs
//! Llama subsystem — built from scratch for llama.cpp local server integration.
//! Uses native TCP streams to avoid external HTTP library dependency conflicts.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use tokio::sync::mpsc;

use crate::config::app_config::LlmBackend;
use crate::ed::buffer::BufferKind;
use crate::ed::ext::CommandResult;
use crate::ed::Buffer;
use crate::ed::MessageKind;
use crate::ed::Mode;
use crate::Editor;
// ── Supporting LLM structures ─────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LlmPreset {
    CheckEnglish,
    TranslateToChinese,
    TranslateToEnglish,
    Explain,
    Summarize,
}

pub struct LlmBuffer {
    pub text: String,
}

impl LlmBuffer {
    pub fn new() -> Self {
        Self {
            text: String::new(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PromptAction {
    Changed,
    Submit,
    Cancel,
    None,
}

pub struct MiniInputPrompt {
    pub buffer: String,
    pub cursor: usize,
    pub history: Vec<String>,
}

impl MiniInputPrompt {
    pub fn new() -> Self {
        Self {
            buffer: String::new(),
            cursor: 0,
            history: Vec::new(),
        }
    }

    pub fn text(&self) -> &str {
        &self.buffer
    }

    pub fn clear(&mut self) {
        self.buffer.clear();
        self.cursor = 0;
    }

    pub fn push_history(&mut self, text: String) {
        self.history.push(text);
    }

    pub fn handle_key(&mut self, key: &KeyEvent) -> PromptAction {
        match key.code {
            KeyCode::Enter => PromptAction::Submit,
            KeyCode::Esc => PromptAction::Cancel,
            KeyCode::Char(c) => {
                let mut chars: Vec<char> = self.buffer.chars().collect();
                if self.cursor <= chars.len() {
                    chars.insert(self.cursor, c);
                    self.buffer = chars.into_iter().collect();
                    self.cursor += 1;
                    PromptAction::Changed
                } else {
                    PromptAction::None
                }
            }
            KeyCode::Backspace => {
                let mut chars: Vec<char> = self.buffer.chars().collect();
                if self.cursor > 0 && self.cursor <= chars.len() {
                    self.cursor -= 1;
                    chars.remove(self.cursor);
                    self.buffer = chars.into_iter().collect();
                    PromptAction::Changed
                } else {
                    PromptAction::None
                }
            }
            _ => PromptAction::None,
        }
    }
}

// ── LLM State struct ──────────────────────────────────────────────

pub struct LlmState {
    pub buffer: LlmBuffer,
    pub todo_prefix: bool,
    pub buffer_id: Option<usize>,
    pub response_tx: mpsc::UnboundedSender<Result<String, String>>,
    pub response_rx: mpsc::UnboundedReceiver<Result<String, String>>,
    pub task_handle: Option<tokio::task::JoinHandle<()>>,
    pub active_preset: Option<LlmPreset>,
    pub active_context: Option<String>,
    pub origin_buffer_id: Option<usize>,
    pub infobar_response: bool,
    pub infobar_accumulator: String,
    pub single_shot: bool,
    pub prompt: MiniInputPrompt,
    pub system_prompt: Option<String>,
}

impl LlmState {
    pub fn new() -> Self {
        let (response_tx, response_rx) = mpsc::unbounded_channel::<Result<String, String>>();

        Self {
            buffer: LlmBuffer::new(),
            todo_prefix: false,
            buffer_id: None,
            response_tx,
            response_rx,
            task_handle: None,
            active_preset: None,
            active_context: None,
            origin_buffer_id: None,
            infobar_response: false,
            infobar_accumulator: String::new(),
            single_shot: false,
            prompt: MiniInputPrompt::new(),
            system_prompt: None,
        }
    }
}

// ── Editor Struct Implementation ──────────────────────────────────
const SPINNER_CHARS: &[char] = &['⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧', '⠇', '⠏'];

impl Editor {
    /// Ensures the background LLM conversation buffer exists and returns its ID.
    /// Does not open windows, perform splits, or alter focus.
    pub fn ensure_llm_buffer_exists(&mut self) -> usize {
        if let Some(buf) = self.buffers.iter().find(|b| b.kind == BufferKind::Llm) {
            let id = buf.id;
            self.llm.buffer_id = Some(id);
            return id;
        }

        // Create the background history buffer if it doesn't exist
        let id = self.next_buf_id;
        self.next_buf_id += 1;

        let mut buf = Buffer::new(id, Some("*llm-chat*".to_string())).unwrap();
        buf.kind = BufferKind::Llm;
        buf.rope = ropey::Rope::from_str(&format!(
            "=== LLM Chat History ({}) ===\n",
            self.config.llm_backend
        ));

        self.buffers.push(buf);
        self.llm.buffer_id = Some(id);
        id
    }

    /// Spawns the async LLM request, dispatching to the correct backend.
    pub fn spawn_llm_request(&mut self, messages: Vec<(String, String)>) {
        self.spawn_llm_request_with_backend(messages, self.config.llm_backend);
    }

    /// Spawns the async LLM request with an explicit backend override.
    pub fn spawn_llm_request_with_backend(
        &mut self,
        messages: Vec<(String, String)>,
        backend: LlmBackend,
    ) {
        if let Some(handle) = self.llm.task_handle.take() {
            handle.abort();
        }

        let tx = self.llm.response_tx.clone();

        match backend {
            LlmBackend::Llamacpp => {
                let url = self.config.llm_url.clone();
                let port = self.config.llm_port;
                let api_key = self.config.llm_api_key.clone();

                let handle = tokio::spawn(async move {
                    log::debug!("[LLM] Using llama.cpp backend ({}:{})", url, port);
                    let res = query_llamacpp_local(messages, &url, port, api_key.as_deref()).await;
                    let _ = tx.send(res);
                });

                self.llm.task_handle = Some(handle);
            }
            LlmBackend::Ollama => {
                let url = self.config.ollama_url.clone();
                let port = self.config.ollama_port;
                let model = self.config.ollama_model.clone();

                let handle = tokio::spawn(async move {
                    log::debug!(
                        "[LLM] Using Ollama backend ({}:{}, model={})",
                        url,
                        port,
                        model
                    );
                    let res = query_ollama_local(messages, &url, port, &model).await;
                    let _ = tx.send(res);
                });

                self.llm.task_handle = Some(handle);
            }
        }
    }

    /// Animates the status infobar with a spinner while a general LLM request is processing.
    pub fn tick_llm_prompt(&mut self) {
        if self.llm.task_handle.is_some() && self.git_commit_buffer_id.is_none() {
            self.tick_spinner();
            let spinner = SPINNER_CHARS[self.spinner_frame() % SPINNER_CHARS.len()];
            self.set_status_msg(
                &format!(
                    "{} LLM is thinking ({})...",
                    spinner, self.config.llm_backend
                ),
                crate::ed::mode::MessageKind::Info,
            );
        }
    }

    /// Polls completed responses from the background runtime channels.
    pub fn poll_llm_responses(&mut self) {
        while let Ok(res) = self.llm.response_rx.try_recv() {
            // Take and drop the task handle to terminate the spinner animation
            let _ = self.llm.task_handle.take();

            match res {
                Ok(response_text) => {
                    if self.git_commit_buffer_id.is_some() {
                        self.git_commit_on_llm_response(&response_text);
                    } else {
                        self.llm.buffer.text = response_text.clone();

                        // Insert first, then read the line count for scrolling
                        let history_id = self.ensure_llm_buffer_exists();
                        if let Some(buf) = self.buf_mut_by_id(history_id) {
                            let current_len = buf.rope.len_chars();
                            buf.rope
                                .insert(current_len, &format!("\nLLM: {}\n", response_text));
                            buf.mark_modified();
                            buf.parse_syntax();
                        }

                        // Now read line count after the insert
                        let total_lines = self
                            .buf_by_id(history_id)
                            .map(|b| b.len_lines())
                            .unwrap_or(1);

                        for win in self.windows.iter_mut() {
                            if win.buffer_id() != history_id {
                                continue;
                            }
                            // Clamp to actual buffer length — guards against stale state
                            let target_row = total_lines.saturating_sub(1);
                            let h = win.position.height;
                            let w = win.position.width;
                            win.row = target_row;
                            win.col = 0;
                            win.scroll_line = target_row.saturating_sub(h.saturating_sub(1));
                            win.scroll_col = 0;
                            win.desired_col = 0;
                            // Don't call scroll_to_cursor here — we already computed scroll_line
                        }

                        self.set_status_msg("Response is ready", MessageKind::Success);
                    }
                }
                Err(err) => {
                    if self.git_commit_buffer_id.is_some() {
                        self.git_commit_on_llm_error(&err);
                    } else {
                        let history_id = self.ensure_llm_buffer_exists();
                        if let Some(buf) = self.buf_mut_by_id(history_id) {
                            let current_len = buf.rope.len_chars();
                            buf.rope
                                .insert(current_len, &format!("\nSystem Error: {}\n", err));
                            buf.mark_modified();
                            buf.parse_syntax();
                        }
                        self.set_status_msg(&format!("LLM Error: {}", err), MessageKind::Error);
                    }
                }
            }
        }
    }

    /// Horizontally splits the window and establishes an interactive LLM chat session.
    pub fn open_llm_chat_session(&mut self) {
        let history_id = self.ensure_llm_buffer_exists();

        let input_id = self
            .buffers
            .iter()
            .find(|b| b.kind == BufferKind::LlmInput)
            .map(|b| b.id)
            .unwrap_or_else(|| {
                let id = self.next_buf_id;
                self.next_buf_id += 1;
                let mut buf = Buffer::new(id, Some("*llm-input*".to_string())).unwrap();
                buf.kind = BufferKind::LlmInput;
                buf.rope = ropey::Rope::from_str("");
                self.buffers.push(buf);
                id
            });

        self.llm.buffer_id = Some(history_id);

        self.split_horizontal();

        // Bottom (active after split) → input
        self.active_window_mut().set_buffer_id(input_id);

        // Top → history
        self.focus_prev_window();
        self.active_window_mut().set_buffer_id(history_id);

        let prev_idx = self.active_window_idx;
        self.clamp_window_row_to_buf(prev_idx);

        // Back to input pane
        self.focus_next_window();

        debug_assert_eq!(self.active_window().buffer_id(), input_id);

        // ── Pre-populate input buffer with selection context ──────
        // The user sees what will be sent, can edit/trim it, then sends.
        // active_context is also kept so follow-up messages include it.
        if let Some(context) = self.llm.active_context.clone() {
            let template = format!("\n{}\n\n", context);
            if let Some(buf) = self.buf_mut_by_id(input_id) {
                buf.rope = ropey::Rope::from_str(&template);
                buf.mark_modified();
                buf.parse_syntax();
            }
            // Read line count first, then take the mutable borrow
            let line_count = self.buf_by_id(input_id).map(|b| b.len_lines()).unwrap_or(1);
            {
                let win = self.active_window_mut();
                win.row = line_count.saturating_sub(1);
                win.col = 0;
                win.scroll_line = 0;
                win.scroll_col = 0;
                win.desired_col = 0;
            }
        } else {
            let win = self.active_window_mut();
            win.row = 0;
            win.col = 0;
            win.scroll_line = 0;
            win.scroll_col = 0;
            win.desired_col = 0;
        }

        self.enter_insert();

        let backend = self.config.llm_backend;
        let context_info = if self.llm.active_context.is_some() {
            " (with selection context)"
        } else {
            ""
        };
        self.set_status_msg(
            &format!(
                "LLM input [{}]{} — Enter: newline  Ctrl-Enter: send  q(normal): close",
                backend, context_info
            ),
            MessageKind::Info,
        );
    }

    // handke_llm_input_buffer_key move to ed/handle_key.rs
    /// Send the current content of the LlmInput buffer as a chat message.
    /// Clears the input buffer and resets cursor on success.
    /// Send the current content of the LlmInput buffer as a chat message.
    /// Clears the input buffer and resets cursor on success.
    /// Send the current content of the LlmInput buffer as a chat message.
    /// Clears the input buffer and resets cursor on success.
    pub fn llm_send_input_buffer(&mut self) -> CommandResult {
        let input_bid = self.active_window().buffer_id();
        let input = self.buf().rope.to_string();

        if input.trim().is_empty() {
            self.set_status_msg("Empty message — type something first", MessageKind::Info);
            return CommandResult::Handled;
        }

        // Clear the input buffer
        if let Some(buf) = self.buf_mut_by_id(input_bid) {
            buf.rope = ropey::Rope::from_str("");
            buf.mark_modified();
            buf.parse_syntax();
        }

        // Reset cursor — buffer is now one empty line
        {
            let win = self.active_window_mut();
            win.row = 0;
            win.col = 0;
            win.scroll_line = 0;
            win.scroll_col = 0;
            win.desired_col = 0;
        }

        // Stay in whatever mode the user was in. If they were in Normal and
        // pressed Enter to send, they stay in Normal. If Insert, they stay
        // in Insert so they can immediately type the next message.
        // Only explicitly switch to Insert if the buffer was just cleared
        // and the mode is not already one of the input modes.
        if !matches!(self.mode, Mode::Insert | Mode::Brief) {
            // Caller came from Normal — switch to Insert so the buffer is
            // ready for the next message. This is opt-in rather than forced.
            self.enter_insert();
        }

        self.llm_send_from_prompt(input)
    }

    pub fn llm_close_split_session(&mut self) -> CommandResult {
        self.llm.active_context = None;
        self.close_window(true);
        CommandResult::Handled
    }

    /// Close the LLM buffer view by switching to a normal buffer.
    pub fn llm_close_buffer(&mut self) {
        if !matches!(self.buf().kind, BufferKind::Llm | BufferKind::LlmInput) {
            return;
        }

        self.llm.active_context = None;

        let target_id = self
            .buffers
            .iter()
            .find(|b| b.kind == BufferKind::Normal && b.filename.is_some())
            .or_else(|| self.buffers.iter().find(|b| b.kind == BufferKind::Normal))
            .map(|b| b.id);

        match target_id {
            Some(id) => self.switch_window_to_buffer(id),
            None => {
                self.open_buffer(None);
            }
        }
    }

    /// Handles sending data from the general interactive prompt
    pub fn llm_send_from_prompt(&mut self, input: String) -> CommandResult {
        // ── Append to history buffer (unchanged) ──
        let history_id = self.ensure_llm_buffer_exists();
        let mut total_lines = 0;
        if let Some(buf) = self.buf_mut_by_id(history_id) {
            let current_len = buf.rope.len_chars();
            buf.rope
                .insert(current_len, &format!("\nUser: {}\n", input));
            buf.mark_modified();
            buf.parse_syntax();
            total_lines = buf.len_lines();
        }

        for win in self.windows.iter_mut() {
            if win.buffer_id() == history_id {
                let target_row = total_lines.saturating_sub(1);
                let h = win.position.height;
                win.row = target_row;
                win.col = 0;
                win.scroll_line = target_row.saturating_sub(h.saturating_sub(1));
                win.scroll_col = 0;
                win.desired_col = 0;
            }
        }

        let backend = self.config.llm_backend;
        self.set_status_msg(
            &format!("Querying {}…", backend),
            crate::ed::mode::MessageKind::Info,
        );

        let system_prompt = self
            .llm
            .system_prompt
            .clone()
            .unwrap_or_else(|| self.config.llm_system_prompt.clone());

        // ── Include active_context (visual selection) if set ──────
        let user_msg = if let Some(context) = &self.llm.active_context {
            format!(
                "Selected code for reference:\n```\n{}\n```\n\n{}",
                context, input
            )
        } else {
            input
        };

        let messages = vec![
            ("system".to_string(), system_prompt),
            ("user".to_string(), user_msg),
        ];

        self.spawn_llm_request_with_backend(messages, backend);
        CommandResult::Handled
    }

    pub fn process_llm_prompt_key(&mut self, key: KeyEvent) -> CommandResult {
        if key.code == KeyCode::Char('r') && key.modifiers.contains(KeyModifiers::CONTROL) {
            self.cmd_waiting_register = true;
            return CommandResult::Handled;
        }

        if self.cmd_waiting_register {
            self.cmd_waiting_register = false;

            let insert_text = match key.code {
                KeyCode::Char('w') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    self.get_word_under_cursor()
                }
                KeyCode::Char('l') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    Some(self.get_current_line_text())
                }
                KeyCode::Char('%') => self.buf().filename.clone(),
                _ => None,
            };

            if let Some(text) = insert_text {
                if !text.is_empty() {
                    // Safe Unicode-friendly character insertion to prevent index errors
                    let mut chars: Vec<char> = self.llm.prompt.buffer.chars().collect();
                    let insert_chars: Vec<char> = text.chars().collect();

                    if self.llm.prompt.cursor <= chars.len() {
                        for (i, c) in insert_chars.iter().enumerate() {
                            chars.insert(self.llm.prompt.cursor + i, *c);
                        }
                        self.llm.prompt.buffer = chars.into_iter().collect();
                        self.llm.prompt.cursor += insert_chars.len();
                    }
                }
                return CommandResult::Handled;
            }
        }

        match self.llm.prompt.handle_key(&key) {
            PromptAction::Changed => CommandResult::Handled,
            PromptAction::Submit => {
                let input = self.llm.prompt.text().to_string();
                self.llm.prompt.clear();
                self.llm.prompt.push_history(input.clone());
                self.clear_status_msg();
                self.mode = Mode::Normal;

                self.llm_send_from_prompt(input)
            }
            PromptAction::Cancel => {
                self.llm.prompt.clear();
                self.llm.active_preset = None;
                self.llm.active_context = None;
                self.llm.todo_prefix = false;
                self.mode = Mode::Normal;
                CommandResult::Handled
            }
            PromptAction::None => CommandResult::Handled,
        }
    }

    /// True when the active window is viewing the LLM chat input buffer.
    pub fn is_in_llm_input(&self) -> bool {
        let bid = self.active_window().buffer_id();
        self.buffers
            .iter()
            .any(|b| b.id == bid && b.kind == BufferKind::LlmInput)
    }

    /// True when the active window is viewing the LLM chat history buffer.
    pub fn is_in_llm_history(&self) -> bool {
        let bid = self.active_window().buffer_id();
        self.buffers
            .iter()
            .any(|b| b.id == bid && b.kind == BufferKind::Llm)
    }
}

/// Communicates with llama.cpp local server using raw TCP sockets, targeting
/// the OpenAI-compatible chat completions endpoint (/v1/chat/completions).
async fn query_llamacpp_local(
    messages: Vec<(String, String)>,
    url: &str,
    port: u16,
    api_key: Option<&str>,
) -> Result<String, String> {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpStream;

    let path = "/v1/chat/completions";

    // Format the simple tuple array into standard API message objects
    let json_messages: Vec<serde_json::Value> = messages
        .iter()
        .map(|(role, content)| {
            serde_json::json!({
                "role": role,
                "content": content
            })
        })
        .collect();

    let payload = serde_json::json!({
        "messages": json_messages,
        "max_tokens": 4096,
        "temperature": 0.7,
        "stream": false
    });

    let body =
        serde_json::to_string(&payload).map_err(|e| format!("JSON Serialization failed: {}", e))?;

    let clean_host = url
        .trim_start_matches("http://")
        .trim_start_matches("https://")
        .trim_end_matches('/');

    let addr = format!("{}:{}", clean_host, port);

    let mut auth_header = String::new();
    if let Some(key) = api_key {
        if !key.trim().is_empty() {
            auth_header = format!("Authorization: Bearer {}\r\n", key.trim());
        }
    }

    let request = if auth_header.is_empty() {
        format!(
            "POST {} HTTP/1.0\r\n\
             Host: {}\r\n\
             Content-Type: application/json\r\n\
             Content-Length: {}\r\n\r\n{}",
            path,
            addr,
            body.len(),
            body
        )
    } else {
        format!(
            "POST {} HTTP/1.0\r\n\
             Host: {}\r\n\
             Content-Type: application/json\r\n\
             Content-Length: {}\r\n\
             {}{}\
             \r\n{}",
            path,
            addr,
            body.len(),
            auth_header,
            "",
            body
        )
    };

    let mut stream = TcpStream::connect(&addr)
        .await
        .map_err(|e| format!("Failed to connect to LLM server at {}: {}", addr, e))?;

    stream
        .write_all(request.as_bytes())
        .await
        .map_err(|e| format!("Failed to write payload to LLM server: {}", e))?;

    let mut response = Vec::new();
    stream
        .read_to_end(&mut response)
        .await
        .map_err(|e| format!("Failed to read stream contents: {}", e))?;

    let response_str = String::from_utf8_lossy(&response);

    let parts: Vec<&str> = response_str.splitn(2, "\r\n\r\n").collect();
    if parts.len() < 2 {
        return Err("Malformed HTTP response received from LLM server".to_string());
    }

    let http_body = parts[1];

    // Deserialize Chat Completion response format
    #[derive(serde::Deserialize)]
    struct ChatMessage {
        content: String,
    }

    #[derive(serde::Deserialize)]
    struct ChatChoice {
        message: ChatMessage,
    }

    #[derive(serde::Deserialize)]
    struct ChatResponse {
        choices: Vec<ChatChoice>,
    }

    let parsed: ChatResponse = serde_json::from_str(http_body).map_err(|e| {
        format!(
            "Failed to parse response payload: {}. Response: {}",
            e, http_body
        )
    })?;

    if parsed.choices.is_empty() {
        return Err("No choices returned from LLM chat completions server".to_string());
    }

    Ok(parsed.choices[0].message.content.clone())
}

/// Communicates with a local Ollama server via its native `/api/chat` endpoint.
///
/// Ollama defaults to port 11434. The response envelope is simpler than
/// the OpenAI format: `{"message":{"content":"..."}}`
async fn query_ollama_local(
    messages: Vec<(String, String)>,
    url: &str,
    port: u16,
    model: &str,
) -> Result<String, String> {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpStream;

    let path = "/api/chat";

    let json_messages: Vec<serde_json::Value> = messages
        .iter()
        .map(|(role, content)| serde_json::json!({ "role": role, "content": content }))
        .collect();

    let payload = serde_json::json!({
        "model": model,
        "messages": json_messages,
        "stream": false
    });

    let body =
        serde_json::to_string(&payload).map_err(|e| format!("JSON serialization failed: {}", e))?;

    let clean_host = url
        .trim_start_matches("http://")
        .trim_start_matches("https://")
        .trim_end_matches('/');

    let addr = format!("{}:{}", clean_host, port);

    let request = format!(
        "POST {} HTTP/1.0\r\n\
         Host: {}\r\n\
         Content-Type: application/json\r\n\
         Content-Length: {}\r\n\r\n{}",
        path,
        addr,
        body.len(),
        body
    );

    let mut stream = TcpStream::connect(&addr)
        .await
        .map_err(|e| format!("Failed to connect to Ollama at {}: {}", addr, e))?;

    stream
        .write_all(request.as_bytes())
        .await
        .map_err(|e| format!("Failed to write payload to Ollama: {}", e))?;

    let mut response = Vec::new();
    stream
        .read_to_end(&mut response)
        .await
        .map_err(|e| format!("Failed to read stream contents: {}", e))?;

    let response_str = String::from_utf8_lossy(&response);

    let parts: Vec<&str> = response_str.splitn(2, "\r\n\r\n").collect();
    if parts.len() < 2 {
        return Err("Malformed HTTP response from Ollama".to_string());
    }

    let http_body = parts[1];

    // Ollama /api/chat response:
    //   {"model":"llama3","message":{"role":"assistant","content":"..."},"done":true}
    #[derive(serde::Deserialize)]
    struct OllamaMessage {
        content: String,
    }

    #[derive(serde::Deserialize)]
    struct OllamaChatResponse {
        message: OllamaMessage,
    }

    let parsed: OllamaChatResponse = serde_json::from_str(http_body).map_err(|e| {
        format!(
            "Failed to parse Ollama response: {}. Body: {}",
            e, http_body
        )
    })?;

    Ok(parsed.message.content)
}
