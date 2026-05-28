// File: src/ai/llama/llm.rs
//! Llama subsystem — built from scratch for llama.cpp local server integration.
//! Uses native TCP streams to avoid external HTTP library dependency conflicts.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use tokio::sync::mpsc;

use crate::ed::buffer::BufferKind;
use crate::ed::ext::CommandResult;
use crate::ed::Buffer;
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
        buf.rope = ropey::Rope::from_str("=== LLM Chat History ===\n");

        self.buffers.push(buf);
        self.llm.buffer_id = Some(id);
        id
    }

    /// Spawns the async task using the background Tokio runtime.
    pub fn spawn_llm_request(&mut self, messages: Vec<(String, String)>) {
        if let Some(handle) = self.llm.task_handle.take() {
            handle.abort();
        }

        let tx = self.llm.response_tx.clone();

        let url = self.config.llm_url.clone();
        let port = self.config.llm_port;
        let api_key = self.config.llm_api_key.clone();

        let handle = tokio::spawn(async move {
            log::debug!("[LLM] Inside tokio task, calling query_llamacpp_local...");
            let res = query_llamacpp_local(messages, &url, port, api_key.as_deref()).await;
            log::debug!(
                "[LLM] query_llamacpp_local returned. Is Ok: {}",
                res.is_ok()
            );
            let _ = tx.send(res);
        });

        self.llm.task_handle = Some(handle);
    }

    /// Animates the status infobar with a spinner while a general LLM request is processing.
    pub fn tick_llm_prompt(&mut self) {
        if self.llm.task_handle.is_some() && self.git_commit_buffer_id.is_none() {
            self.tick_spinner();
            let spinner = SPINNER_CHARS[self.spinner_frame() % SPINNER_CHARS.len()];
            self.set_status_msg(
                &format!("{} LLM is thinking...", spinner),
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
                        // Cache response inside background state
                        self.llm.buffer.text = response_text.clone();

                        // Append response silently to the background history buffer
                        let history_id = self.ensure_llm_buffer_exists();
                        let mut total_lines = 0;
                        if let Some(buf) = self.buf_mut_by_id(history_id) {
                            let current_len = buf.rope.len_chars();
                            buf.rope
                                .insert(current_len, &format!("\nLLM: {}\n", response_text));
                            buf.mark_modified();
                            buf.parse_syntax();
                            total_lines = buf.len_lines();
                        }

                        // Scroll any windows viewing the history buffer to the bottom
                        for win in &mut self.windows {
                            if win.buffer_id() == history_id {
                                win.row = total_lines.saturating_sub(1);
                                win.col = 0;
                                let h = win.position.height;
                                let w = win.position.width;
                                win.scroll_to_cursor(h, w, 0);
                            }
                        }

                        // Simply display the completion message, keeping current layout intact
                        self.set_status_msg(
                            "Response is ready",
                            crate::ed::mode::MessageKind::Success,
                        );
                    }
                }
                Err(err) => {
                    if self.git_commit_buffer_id.is_some() {
                        self.git_commit_on_llm_error(&err);
                    } else {
                        // Append error silently to the background history buffer
                        let history_id = self.ensure_llm_buffer_exists();
                        if let Some(buf) = self.buf_mut_by_id(history_id) {
                            let current_len = buf.rope.len_chars();
                            buf.rope
                                .insert(current_len, &format!("\nSystem Error: {}\n", err));
                            buf.mark_modified();
                            buf.parse_syntax();
                        }

                        self.set_status_msg(
                            &format!("LLM Error: {}", err),
                            crate::ed::mode::MessageKind::Error,
                        );
                    }
                }
            }
        }
    }

    /// Horizontally splits the window and establishes an interactive LLM chat session.
    pub fn open_llm_chat_session(&mut self) {
        use crate::ed::MessageKind;

        // 1. Create or fetch the Llm chat history buffer
        let history_id = self.ensure_llm_buffer_exists();

        // 2. Create or fetch the LlmInput query buffer
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

        // Track the active history buffer in your LlmState
        self.llm.buffer_id = Some(history_id);

        // 3. Perform a horizontal subdivision
        // This splits window 0, creating window 1 (bottom) and focusing it.
        self.split_horizontal();

        // Since split_horizontal focuses the bottom window (index 1), assign input_id to it.
        self.active_window_mut().set_buffer_id(input_id);

        // Switch to the top window (index 0) and assign history_id to it.
        self.focus_prev_window();
        self.active_window_mut().set_buffer_id(history_id);

        // Focus back down to the bottom window (index 1) so it is active.
        self.focus_next_window();

        // Automatically enter insert mode so you can begin typing immediately.
        self.enter_insert();

        self.set_status_msg(
            "LLM Chat Session — Type and press Enter to send, 'q' to close split",
            MessageKind::Info,
        );
    }

    // handke_llm_input_buffer_key move to ed/handle_key.rs

    pub fn llm_send_input_buffer(&mut self) -> CommandResult {
        let input = self.buf().rope.to_string();

        // Clear the LlmInput query buffer so the user can type the next message
        let input_bid = self.active_window().buffer_id();
        if let Some(buf) = self.buf_mut_by_id(input_bid) {
            buf.rope = ropey::Rope::from_str("");
            buf.mark_modified();
            buf.parse_syntax();
        }

        // Focus/reset cursor in the active LlmInput window
        let win = self.active_window_mut();
        win.row = 0;
        win.col = 0;
        win.scroll_line = 0;
        win.scroll_col = 0;
        win.desired_col = 0;

        self.llm_send_from_prompt(input)
    }

    pub fn llm_close_split_session(&mut self) -> CommandResult {
        self.close_window(true);
        CommandResult::Handled
    }

    /// Close the LLM buffer view by switching to a normal buffer.
    /// Used when 'q' is pressed in an LLM buffer but there's no split to close.
    /// Unlike `llm_close_split_session`, this will NOT quit the app if it's
    /// the only window.
    pub fn llm_close_buffer(&mut self) {
        if !matches!(self.buf().kind, BufferKind::Llm | BufferKind::LlmInput) {
            return;
        }

        let target_id = self
            .buffers
            .iter()
            .find(|b| b.kind == BufferKind::Normal && b.filename.is_some())
            .or_else(|| self.buffers.iter().find(|b| b.kind == BufferKind::Normal))
            .map(|b| b.id);

        let target_id = match target_id {
            Some(id) => id,
            None => self.buffers.first().map(|b| b.id).unwrap_or(0),
        };

        self.switch_window_to_buffer(target_id);
    }

    /// Handles sending data from the general interactive prompt
    pub fn llm_send_from_prompt(&mut self, input: String) -> CommandResult {
        // Append prompt silently to the background history buffer
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

        // Scroll any windows viewing the history buffer to the bottom
        for win in &mut self.windows {
            if win.buffer_id() == history_id {
                win.row = total_lines.saturating_sub(1);
                win.col = 0;
                let h = win.position.height;
                let w = win.position.width;
                win.scroll_to_cursor(h, w, 0);
            }
        }

        self.set_status_msg("Querying llama.cpp...", crate::ed::mode::MessageKind::Info);

        // Extract system prompt config fallback
        let system_prompt = self
            .llm
            .system_prompt
            .clone()
            .unwrap_or_else(|| self.config.llm_system_prompt.clone());

        // Structure prompt into system + user messages
        let messages = vec![
            ("system".to_string(), system_prompt),
            ("user".to_string(), input),
        ];

        self.spawn_llm_request(messages);
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
