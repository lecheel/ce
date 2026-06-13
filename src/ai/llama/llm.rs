//! Llama subsystem — built from scratch for llama.cpp local server integration.
//! Uses native TCP streams to avoid external HTTP library dependency conflicts.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use tokio::sync::mpsc;

use crate::ai::llama::skills;
use crate::ai::llama::skills::{Skill, ToolConversation};
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
    pub active_skill: Option<Skill>,
    pub tool_conversation: Option<ToolConversation>,

    /// True when the current request is for a CodeLlm single-buffer chat.
    /// The response handler checks this flag to decide whether to call
    /// codellm_finalize_response() or update the 2-panel Llm buffers.
    pub is_codellm: bool,
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
            is_codellm: false,
            active_skill: None,
            tool_conversation: None,
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
            "=== LLM Chat History ({}) '>' for input in prompt  ===\n",
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
                let skill = self.llm.active_skill.clone();

                let handle = tokio::spawn(async move {
                    log::debug!("[LLM] Using llama.cpp backend ({}:{})", url, port);
                    let res = query_llamacpp_local(
                        messages,
                        &url,
                        port,
                        api_key.as_deref(),
                        skill.as_ref(),
                    )
                    .await;
                    let _ = tx.send(res);
                });

                self.llm.task_handle = Some(handle);
            }
            LlmBackend::Ollama => {
                let url = self.config.ollama_url.clone();
                let port = self.config.ollama_port;
                let model = self.config.ollama_model.clone();
                let skill = self.llm.active_skill.clone();

                let handle = tokio::spawn(async move {
                    log::debug!(
                        "[LLM] Using Ollama backend ({}:{}, model={})",
                        url,
                        port,
                        model
                    );
                    let res =
                        query_ollama_local(messages, &url, port, &model, skill.as_ref()).await;
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
            let _ = self.llm.task_handle.take();

            match res {
                Ok(response_text) => {
                    // ── Infobar response (translation, etc.) ──────────────
                    if self.llm.infobar_response {
                        self.llm.infobar_response = false;

                        let trimmed = response_text.trim().to_string();
                        if trimmed.is_empty() {
                            self.set_status_msg("Translation returned empty", MessageKind::Error);
                        } else {
                            self.yank_to_register(trimmed.clone(), Some('z'));

                            const MAX_INFOBAR: usize = 200;
                            let display = if trimmed.chars().count() > MAX_INFOBAR {
                                let end = trimmed
                                    .char_indices()
                                    .nth(MAX_INFOBAR)
                                    .map(|(i, _)| i)
                                    .unwrap_or(trimmed.len());
                                format!("{}…  [reg z]", &trimmed[..end])
                            } else {
                                format!("{}  [reg z]", trimmed)
                            };
                            self.set_status_msg(&display, MessageKind::Success);
                        }
                    } else if self.git_commit_buffer_id.is_some() {
                        self.git_commit_on_llm_response(&response_text);
                    } else if self.llm.is_codellm {
                        // ── CodeLlm (single-buffer) response handling ──
                        self.llm.is_codellm = false;

                        let codellm_id = self
                            .buffers
                            .iter()
                            .find(|b| b.kind == BufferKind::CodeLlm)
                            .map(|b| b.id);

                        if let Some(id) = codellm_id {
                            if let Some(buf) = self.buf_mut_by_id(id) {
                                let wrapped = word_wrap(&response_text, 100);
                                let current_len = buf.rope.len_chars();
                                buf.rope.insert(current_len, &wrapped);
                                buf.parse_syntax();
                            }
                            self.codellm_finalize_response();
                        } else {
                            self.set_status_msg(
                                "CodeLlm buffer closed before response arrived",
                                MessageKind::Error,
                            );
                        }
                    } else {
                        // ── Legacy 2-panel Llm response handling ──
                        self.llm.buffer.text = response_text.clone();

                        let wrapped = word_wrap(&response_text, 100);

                        let history_id = self.ensure_llm_buffer_exists();
                        if let Some(buf) = self.buf_mut_by_id(history_id) {
                            let current_len = buf.rope.len_chars();
                            buf.rope
                                .insert(current_len, &format!("\nLLM: {}\n", wrapped));
                            buf.mark_modified();
                            buf.parse_syntax();
                        }

                        let total_lines = self
                            .buf_by_id(history_id)
                            .map(|b| b.len_lines())
                            .unwrap_or(1);

                        for win in self.windows.iter_mut() {
                            if win.buffer_id() != history_id {
                                continue;
                            }
                            let target_row = total_lines.saturating_sub(1);
                            let h = win.position.height;
                            win.row = target_row;
                            win.col = 0;
                            win.scroll_line = target_row.saturating_sub(h.saturating_sub(1));
                            win.scroll_col = 0;
                            win.desired_col = 0;
                        }

                        self.set_status_msg("Response is ready", MessageKind::Success);
                    }
                }
                Err(err) => {
                    let _ = self.llm.task_handle.take();

                    if self.llm.infobar_response {
                        self.llm.infobar_response = false;
                        self.set_status_msg(
                            &format!("Translation failed: {}", err),
                            MessageKind::Error,
                        );
                    } else if self.git_commit_buffer_id.is_some() {
                        self.git_commit_on_llm_error(&err);
                    } else if self.llm.is_codellm {
                        self.llm.is_codellm = false;

                        let codellm_id = self
                            .buffers
                            .iter()
                            .find(|b| b.kind == BufferKind::CodeLlm)
                            .map(|b| b.id);

                        if let Some(id) = codellm_id {
                            if let Some(buf) = self.buf_mut_by_id(id) {
                                let current_len = buf.rope.len_chars();
                                buf.rope
                                    .insert(current_len, &format!("\n**Error:** {}\n", err));
                                buf.parse_syntax();
                            }
                            self.codellm_finalize_response();
                        }
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
        if let Some(context) = self.llm.active_context.clone() {
            let template = format!("\n{}\n\n", context);
            if let Some(buf) = self.buf_mut_by_id(input_id) {
                buf.rope = ropey::Rope::from_str(&template);
                buf.mark_modified();
                buf.parse_syntax();
            }
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

    /// Send the current content of the LlmInput buffer as a chat message.
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

        // Reset cursor
        {
            let win = self.active_window_mut();
            win.row = 0;
            win.col = 0;
            win.scroll_line = 0;
            win.scroll_col = 0;
            win.desired_col = 0;
        }

        if !matches!(self.mode, Mode::Insert | Mode::Brief) {
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

    // ═══════════════════════════════════════════════════════════════
    // CodeLlm (single-buffer chat) methods
    // ═══════════════════════════════════════════════════════════════

    /// Send the current prompt in a CodeLlm buffer.
    pub fn codellm_send(&mut self) {
        if self.buf().kind != BufferKind::CodeLlm {
            return;
        }

        // Don't allow sending while a response is streaming
        if self.llm.task_handle.is_some() {
            self.set_status_msg("LLM is still responding…", MessageKind::Info);
            return;
        }

        let lock_line = self.buf().llm_lock_line;

        // ── Extract prompt text ───────────────────────────────
        let mut prompt = String::new();
        let total = self.buf().len_lines();
        for i in lock_line..total {
            let line = self.buf().line_text(i);
            let trimmed = line.trim_end_matches('\n').trim_end_matches('\r');
            prompt.push_str(trimmed);
            prompt.push('\n');
        }
        let prompt = prompt.trim().to_string();

        if prompt.is_empty() {
            self.set_status_msg("Empty prompt", MessageKind::Error);
            return;
        }

        // ── Lock the prompt area (it's now history) ──────────
        {
            let buf = self.buf_mut();
            buf.llm_lock_line = buf.len_lines();
            buf.rope.insert(buf.rope.len_chars(), "\n## Assistant\n\n");
            buf.parse_syntax();
        }

        // ── Move cursor to the end (watching the response) ──
        {
            let total = self.buf().len_lines().saturating_sub(1);
            let win = self.active_window_mut();
            win.row = total;
            win.col = 0;
            win.desired_col = 0;
        }

        // ── Switch to Normal mode while waiting ──────────────
        self.enter_normal();

        // ── Send to LLM ──────────────────────────────────────
        let full_prompt = if let Some(ctx) = &self.llm.active_context {
            format!("Selected code:\n```\n{}\n```\n\n{}", ctx, prompt)
        } else {
            prompt.clone()
        };

        self.llm_send_codellm(full_prompt);
    }

    /// Wrapper that wires into the existing LLM backend for CodeLlm buffers.
    fn llm_send_codellm(&mut self, prompt: String) {
        self.llm.is_codellm = true;

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

        let messages = vec![
            ("system".to_string(), system_prompt),
            ("user".to_string(), prompt),
        ];

        self.spawn_llm_request_with_backend(messages, backend);
    }

    /// Finalize a CodeLlm response: lock it and prepare the next prompt area.
    pub fn codellm_finalize_response(&mut self) {
        let codellm_id = self
            .buffers
            .iter()
            .find(|b| b.kind == BufferKind::CodeLlm)
            .map(|b| b.id);

        if let Some(id) = codellm_id {
            // Ensure we are viewing this buffer
            if self.active_window().buffer_id() != id {
                self.switch_window_to_buffer(id);
            }

            let (last_line, total_lines) = {
                let buf = self.buf_mut_by_id(id).unwrap();
                let len = buf.rope.len_chars();
                if len == 0 || buf.rope.char(len - 1) != '\n' {
                    buf.rope.insert(len, "\n");
                }
                buf.llm_lock_line = buf.len_lines();
                buf.rope.insert(buf.rope.len_chars(), "\n## You\n");
                buf.parse_syntax();
                let last = buf.len_lines().saturating_sub(1);
                (last, buf.len_lines())
            };

            let win = self.active_window_mut();
            win.row = last_line;
            win.col = 0;
            win.desired_col = 0;

            // Auto-scroll to follow the response if needed
            let h = win.position.height;
            win.scroll_line = last_line.saturating_sub(h.saturating_sub(1));

            self.enter_insert();
            self.set_status_msg(
                "LLM response complete — type next prompt",
                MessageKind::Info,
            );
        } else {
            self.set_status_msg(
                "CodeLlm buffer closed before response arrived",
                MessageKind::Error,
            );
        }
    }

    /// Extracts the function around the cursor and opens a CodeLlm
    /// session to explain it, including the function signature/head.
    pub fn llm_explain_function(&mut self) {
        // 1. Find the function span using Tree-sitter
        let span_info = self.function_around_span_info();

        let Some(info) = span_info else {
            self.set_status_msg("No function found around cursor", MessageKind::Error);
            return;
        };

        // 2. Extract the text from start_row to end_row (inclusive)
        let func_text = self.extract_line_range_text(info.start_row, info.end_row);

        let Some(func_text) = func_text else {
            self.set_status_msg("Failed to extract function text", MessageKind::Error);
            return;
        };

        if func_text.trim().is_empty() {
            self.set_status_msg("Function is empty", MessageKind::Error);
            return;
        }

        // 3. Open the CodeLlm session
        self.open_codellm_chat_session();

        // 4. Inject the function text and a prompt directly into the buffer
        let prompt = format!(
            "Explain this function:\n```\n{}\n```\n\nPlease explain what this function does, its parameters, and its return value.",
            func_text.trim_end()
        );

        let buf = self.buf_mut();
        if buf.kind == BufferKind::CodeLlm {
            // Append to the "## You" section
            let current_len = buf.rope.len_chars();
            buf.rope.insert(current_len, &prompt);
            buf.parse_syntax();

            // Move cursor to the very end of the injected prompt
            let last_line = buf.len_lines().saturating_sub(1);
            let last_col = buf.line_char_len(last_line);

            let win = self.active_window_mut();
            win.row = last_line;
            win.col = last_col;
            win.desired_col = last_col;
        }

        // 5. Auto-send the prompt
        self.codellm_send();
    }
}

/// Ollama with optional tool-calling support.
async fn query_ollama_local(
    messages: Vec<(String, String)>,
    url: &str,
    port: u16,
    model: &str,
    skill: Option<&Skill>,
) -> Result<String, String> {
    let Some(skill) = skill else {
        return query_ollama_plain(messages, url, port, model).await;
    };

    let tools_json: Vec<serde_json::Value> = skill
        .tools
        .iter()
        .map(|t| {
            serde_json::json!({
                "type": "function",
                "function": {
                    "name": t.name,
                    "description": t.description,
                    "parameters": t.parameters,
                }
            })
        })
        .collect();

    let mut messages = messages;
    if let Some(entry) = messages.iter_mut().find(|(role, _)| role == "system") {
        entry.1 = skill.system_prompt.clone();
    } else {
        messages.insert(0, ("system".into(), skill.system_prompt.clone()));
    }

    let mut conversation = ToolConversation::new();
    for (role, content) in &messages {
        conversation
            .messages
            .push(serde_json::json!({"role": role, "content": content}));
    }

    for _round in 0..MAX_TOOL_ROUNDS {
        let payload = build_ollama_payload(&conversation.messages, model, Some(&tools_json));
        let response = send_ollama_request_raw(&payload, url, port).await?;
        let choice = &response["message"];

        if let Some(tool_calls) = choice["tool_calls"].as_array() {
            if !tool_calls.is_empty() {
                conversation.push_assistant(choice);
                for tc in tool_calls {
                    let func = &tc["function"];
                    let tool_name = func["name"].as_str().unwrap_or("unknown");
                    let call_id = tc["id"].as_str().unwrap_or("").to_string(); // ← grab the ID
                    let args_str = func["arguments"].as_str().unwrap_or("{}");
                    let args: serde_json::Value =
                        serde_json::from_str(args_str).unwrap_or(serde_json::json!({}));
                    log::info!("[Tool] Calling {} with args: {}", tool_name, args_str);
                    let result = skills::execute_tool(
                        &skill
                            .tools
                            .iter()
                            .find(|t| t.name == tool_name)
                            .ok_or_else(|| format!("Unknown tool: {}", tool_name))?
                            .clone(),
                        &args,
                        &skill.config_defaults,
                    )
                    .unwrap_or_else(|e| format!("Error: {}", e));
                    log::info!("[Tool] Result ({} chars)", result.len());
                    conversation.push_tool_result(&call_id, tool_name, &result);
                    // ← pass ID
                }
                continue;
            }
        }

        if let Some(content) = choice["content"].as_str() {
            return Ok(content.to_string());
        }
        return Err("Ollama returned empty response with no tool calls".into());
    }

    let payload = build_ollama_payload(&conversation.messages, model, None);
    let response = send_ollama_request_raw(&payload, url, port).await?;
    if let Some(content) = response["message"]["content"].as_str() {
        return Ok(content.to_string());
    }
    Err("Ollama did not produce a final answer after max tool rounds".into())
}

/// Plain Ollama chat (no tools) — original logic.
async fn query_ollama_plain(
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

fn build_ollama_payload(
    messages: &[serde_json::Value],
    model: &str,
    tools: Option<&[serde_json::Value]>,
) -> serde_json::Value {
    let mut payload = serde_json::json!({
        "model": model,
        "messages": messages,
        "stream": false
    });
    if let Some(tools_arr) = tools {
        payload["tools"] = serde_json::json!(tools_arr);
    }
    payload
}

async fn send_ollama_request_raw(
    payload: &serde_json::Value,
    url: &str,
    port: u16,
) -> Result<serde_json::Value, String> {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpStream;

    let body =
        serde_json::to_string(payload).map_err(|e| format!("JSON serialization failed: {}", e))?;
    let clean_host = url
        .trim_start_matches("http://")
        .trim_start_matches("https://")
        .trim_end_matches('/');
    let addr = format!("{}:{}", clean_host, port);
    let request = format!(
        "POST /api/chat HTTP/1.0\r\n\
         Host: {}\r\n\
         Content-Type: application/json\r\n\
         Content-Length: {}\r\n\r\n{}",
        addr,
        body.len(),
        body
    );
    let mut stream = TcpStream::connect(&addr)
        .await
        .map_err(|e| format!("Connect failed: {}", e))?;
    stream
        .write_all(request.as_bytes())
        .await
        .map_err(|e| format!("Write failed: {}", e))?;
    let mut response = Vec::new();
    stream
        .read_to_end(&mut response)
        .await
        .map_err(|e| format!("Read failed: {}", e))?;
    let response_str = String::from_utf8_lossy(&response);
    let parts: Vec<&str> = response_str.splitn(2, "\r\n\r\n").collect();
    if parts.len() < 2 {
        return Err("Malformed HTTP response".into());
    }
    serde_json::from_str(parts[1]).map_err(|e| format!("JSON parse error: {}", e))
}

/// Communicates with a local Ollama server via its native `/api/chat` endpoint.
/// llama.cpp with optional tool-calling support.
/// When `skill` is None, falls back to plain chat.
async fn query_llamacpp_local(
    messages: Vec<(String, String)>,
    url: &str,
    port: u16,
    api_key: Option<&str>,
    skill: Option<&Skill>,
) -> Result<String, String> {
    let Some(skill) = skill else {
        return query_llamacpp_plain(messages, url, port, api_key).await;
    };

    let tools_json: Vec<serde_json::Value> = skill
        .tools
        .iter()
        .map(|t| {
            serde_json::json!({
                "type": "function",
                "function": {
                    "name": t.name,
                    "description": t.description,
                    "parameters": t.parameters,
                }
            })
        })
        .collect();

    let mut messages = messages;
    if let Some(entry) = messages.iter_mut().find(|(role, _)| role == "system") {
        entry.1 = skill.system_prompt.clone();
    } else {
        messages.insert(0, ("system".into(), skill.system_prompt.clone()));
    }

    let mut conversation = ToolConversation::new();
    for (role, content) in &messages {
        conversation
            .messages
            .push(serde_json::json!({"role": role, "content": content}));
    }

    for _round in 0..MAX_TOOL_ROUNDS {
        let payload = build_llm_payload(&conversation.messages, Some(&tools_json));
        let response = send_llm_request_raw(&payload, url, port, api_key).await?;
        let choice = &response["choices"][0]["message"];

        if let Some(tool_calls) = choice["tool_calls"].as_array() {
            if !tool_calls.is_empty() {
                conversation.push_assistant(choice);

                for tc in tool_calls {
                    let func = &tc["function"];
                    let tool_name = func["name"].as_str().unwrap_or("unknown");
                    let call_id = tc["id"].as_str().unwrap_or("").to_string(); // ← grab the ID
                    let args_str = func["arguments"].as_str().unwrap_or("{}");
                    let args: serde_json::Value =
                        serde_json::from_str(args_str).unwrap_or(serde_json::json!({}));
                    log::info!("[Tool] Calling {} with args: {}", tool_name, args_str);
                    let result = skills::execute_tool(
                        &skill
                            .tools
                            .iter()
                            .find(|t| t.name == tool_name)
                            .ok_or_else(|| format!("Unknown tool: {}", tool_name))?
                            .clone(),
                        &args,
                        &skill.config_defaults,
                    )
                    .unwrap_or_else(|e| format!("Error: {}", e));
                    log::info!("[Tool] Result ({} chars)", result.len());
                    conversation.push_tool_result(&call_id, tool_name, &result);
                    // ← pass ID
                }

                continue;
            }
        }

        if let Some(content) = choice["content"].as_str() {
            return Ok(content.to_string());
        }
        return Err("LLM returned empty response with no tool calls".into());
    }

    // Fallback: force final answer without tools
    let payload = build_llm_payload(&conversation.messages, None);
    let response = send_llm_request_raw(&payload, url, port, api_key).await?;
    if let Some(content) = response["choices"][0]["message"]["content"].as_str() {
        return Ok(content.to_string());
    }
    Err("LLM did not produce a final answer after max tool rounds".into())
}

/// Plain llama.cpp chat (no tools) — original logic.
async fn query_llamacpp_plain(
    messages: Vec<(String, String)>,
    url: &str,
    port: u16,
    api_key: Option<&str>,
) -> Result<String, String> {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpStream;

    let path = "/v1/chat/completions";
    let json_messages: Vec<serde_json::Value> = messages
        .iter()
        .map(|(role, content)| serde_json::json!({"role": role, "content": content}))
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
             {}\
             \r\n{}",
            path,
            addr,
            body.len(),
            auth_header,
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

/// Build the JSON payload for a llama.cpp request.
fn build_llm_payload(
    messages: &[serde_json::Value],
    tools: Option<&[serde_json::Value]>,
) -> serde_json::Value {
    let mut payload = serde_json::json!({
        "messages": messages,
        "max_tokens": 4096,
        "temperature": 0.7,
        "stream": false
    });
    if let Some(tools_arr) = tools {
        payload["tools"] = serde_json::json!(tools_arr);
        payload["tool_choice"] = serde_json::json!("auto");
    }
    payload
}

impl Editor {
    /// Action: Review the function or visual selection.
    pub fn llm_review(&mut self) {
        let (code, instruction) = self.get_code_context(
            "Review this code for bugs, performance issues, and style improvements.",
        );
        self.llm_action_helper(
            "You are an expert senior software engineer performing a code review. Be concise and constructive.",
            &code,
            &instruction,
            true // Auto-send
        );
    }

    /// Action: Fix the function or visual selection (optionally including LSP diagnostics).
    pub fn llm_fix(&mut self) {
        let (code, base_instruction) = self.get_code_context("Fix the issues in this code.");

        // Enrich with LSP diagnostics if available!
        let mut instruction = base_instruction;
        let row = self.active_window().row;
        let diagnostics: Vec<String> = self
            .buf()
            .diagnostics
            .iter()
            .filter(|d| d.line == row)
            .map(|d| format!("- {}", d.message))
            .collect();

        if !diagnostics.is_empty() {
            instruction.push_str("\n\nCompiler/LSP Errors:\n");
            instruction.push_str(&diagnostics.join("\n"));
        }

        self.llm_action_helper(
            "You are an expert debugger. Provide ONLY the corrected code inside a markdown block, with no extra explanation unless necessary.",
            &code,
            &instruction,
            true // Auto-send
        );
    }

    /// Action: Add current selection/context to the existing chat (no auto-send).
    pub fn llm_add_to_chat(&mut self) {
        let (code, _) = self.get_code_context("");
        self.llm_action_helper(
            "You are a helpful coding assistant.", // Reset to default system prompt
            &code,
            "I am adding this code to the context. My question is: ", // Leave open for user to type
            false, // DO NOT auto-send, let the user type their question first!
        );
    }

    /// Action: Generate code based on a prompt (leaves prompt empty for user to type).
    pub fn llm_generate(&mut self) {
        self.llm_action_helper(
            "You are an expert coder. Output ONLY the requested code inside a markdown block.",
            "",                         // No code provided
            "Generate the following: ", // User types the rest
            false,                      // DO NOT auto-send
        );
    }

    /// Helper to get either the visual selection or the current function under cursor
    pub fn get_code_context(&self, default_instruction: &str) -> (String, String) {
        // 1. Try visual selection first
        if let Some((r1, r2)) = self.get_visual_line_range() {
            if let Some(text) = self.extract_line_range_text(r1, r2) {
                return (text, default_instruction.to_string());
            }
        }

        // 2. Fallback to function under cursor
        if let Some(info) = self.function_around_span_info() {
            if let Some(text) = self.extract_line_range_text(info.start_row, info.end_row) {
                return (text, default_instruction.to_string());
            }
        }

        // 3. Fallback to empty (user must type/paste code)
        (String::new(), default_instruction.to_string())
    }
}

impl Editor {
    /// Core helper for CodeLlm actions. Opens a chat (or uses an existing one),
    /// injects the code with a specific instruction, and optionally auto-sends.
    fn llm_action_helper(
        &mut self,
        system_prompt: &str,
        code: &str,
        instruction: &str,
        auto_send: bool,
    ) {
        // Check if a CodeLlm buffer already exists
        let existing_id = self
            .buffers
            .iter()
            .find(|b| b.kind == BufferKind::CodeLlm)
            .map(|b| b.id);

        let chat_id = if let Some(id) = existing_id {
            // Switch to the existing chat buffer
            self.switch_window_to_buffer(id);
            id
        } else {
            // Open a new session
            self.open_codellm_chat_session();
            self.buf().id // get the newly created ID
        };

        // Format the payload
        let payload = format!("Code:\n```\n{}\n```\n\n{}", code.trim_end(), instruction);

        // Inject into the prompt area
        let buf = self.buf_mut();
        if buf.kind == BufferKind::CodeLlm {
            let current_len = buf.rope.len_chars();
            buf.rope.insert(current_len, &payload);
            buf.parse_syntax();

            let last_line = buf.len_lines().saturating_sub(1);
            let last_col = buf.line_char_len(last_line);
            let win = self.active_window_mut();
            win.row = last_line;
            win.col = last_col;
            win.desired_col = last_col;
        }

        // Override the system prompt for this specific request
        self.llm.system_prompt = Some(system_prompt.to_string());

        if auto_send {
            self.codellm_send();
        }
    }
}

pub fn word_wrap(text: &str, max_width: usize) -> String {
    let mut out = String::new();
    for line in text.split('\n') {
        let line = line.trim_end_matches('\r');
        if line.is_empty() {
            out.push('\n');
            continue;
        }
        let mut cur = String::new();
        for word in line.split(' ') {
            if word.is_empty() {
                continue;
            }
            if cur.is_empty() {
                cur = word.to_string();
            } else if cur.len() + 1 + word.len() <= max_width {
                cur.push(' ');
                cur.push_str(word);
            } else {
                out.push_str(&cur);
                out.push('\n');
                cur = word.to_string();
            }
        }
        if !cur.is_empty() {
            out.push_str(&cur);
        }
        out.push('\n');
    }
    out.pop(); // trailing newline
    out
}

/// Maximum tool-calling rounds before forcing a final answer
const MAX_TOOL_ROUNDS: usize = 15;

/// Send the raw request and return the parsed JSON response.
async fn send_llm_request_raw(
    payload: &serde_json::Value,
    url: &str,
    port: u16,
    api_key: Option<&str>,
) -> Result<serde_json::Value, String> {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpStream;

    let body =
        serde_json::to_string(payload).map_err(|e| format!("JSON serialization failed: {}", e))?;

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

    let request = format!(
        "POST /v1/chat/completions HTTP/1.0\r\n\
         Host: {}\r\n\
         Content-Type: application/json\r\n\
         Content-Length: {}\r\n\
         {}\
         \r\n{}",
        addr,
        body.len(),
        auth_header,
        body
    );

    let mut stream = TcpStream::connect(&addr)
        .await
        .map_err(|e| format!("Connect failed: {}", e))?;

    stream
        .write_all(request.as_bytes())
        .await
        .map_err(|e| format!("Write failed: {}", e))?;

    let mut response = Vec::new();
    stream
        .read_to_end(&mut response)
        .await
        .map_err(|e| format!("Read failed: {}", e))?;

    let response_str = String::from_utf8_lossy(&response);
    let parts: Vec<&str> = response_str.splitn(2, "\r\n\r\n").collect();
    if parts.len() < 2 {
        return Err("Malformed HTTP response".into());
    }

    serde_json::from_str(parts[1]).map_err(|e| format!("JSON parse error: {}", e))
}
