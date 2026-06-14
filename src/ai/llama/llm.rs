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

#[derive(Debug, Clone)]
pub struct ToolCallSummary {
    pub tool_name: String,
    pub args_brief: String,    // truncated args for display
    pub result_chars: usize,   // length of result for summary
    pub error: Option<String>, // if tool execution failed
}

#[derive(Debug, Clone)]
pub struct LlmResponse {
    pub text: String,
    pub tool_summaries: Vec<ToolCallSummary>,
    pub total_rounds: usize,
}

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
    pub response_tx: mpsc::UnboundedSender<Result<LlmResponse, String>>,
    pub response_rx: mpsc::UnboundedReceiver<Result<LlmResponse, String>>,
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
    pub is_codellm: bool,
    pub llm_request_start: Option<std::time::Instant>,
    pub prompt_active: bool,
}

impl LlmState {
    pub fn new() -> Self {
        let (response_tx, response_rx) = mpsc::unbounded_channel::<Result<LlmResponse, String>>();

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
            llm_request_start: None,
            prompt_active: false,
        }
    }
}

/// Truncate a JSON args string for display purposes.
fn truncate_args(args_str: &str, max_len: usize) -> String {
    if args_str.len() <= max_len {
        args_str.to_string()
    } else {
        format!("{}…", &args_str[..max_len])
    }
}

/// Build a human-readable summary block from tool call summaries.
fn format_tool_summary_block(tool_summaries: &[ToolCallSummary], total_rounds: usize) -> String {
    if tool_summaries.is_empty() {
        return String::new();
    }
    let mut block = format!(
        "\n📋 Tool calls ({} round(s), {} call(s)):\n",
        total_rounds,
        tool_summaries.len()
    );
    for (i, tc) in tool_summaries.iter().enumerate() {
        if let Some(ref err) = tc.error {
            block.push_str(&format!(
                "  {}. ❌ {}({}) → ERROR: {}\n",
                i + 1,
                tc.tool_name,
                tc.args_brief,
                err
            ));
        } else {
            block.push_str(&format!(
                "  {}. ✅ {}({}) → {} chars\n",
                i + 1,
                tc.tool_name,
                tc.args_brief,
                tc.result_chars
            ));
        }
    }
    block.push('\n');
    block
}

// ── Editor Struct Implementation ──────────────────────────────────
const SPINNER_CHARS: &[char] = &['⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧', '⠇', '⠏'];

impl Editor {
    /// Ensures the background LLM conversation buffer exists and returns its ID.
    pub fn ensure_llm_buffer_exists(&mut self) -> usize {
        if let Some(buf) = self.buffers.iter().find(|b| b.kind == BufferKind::Llm) {
            let id = buf.id;
            self.llm.buffer_id = Some(id);
            return id;
        }

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

    pub fn spawn_llm_request(&mut self, messages: Vec<(String, String)>) {
        self.spawn_llm_request_with_backend(messages, self.config.llm_backend);
    }

    pub fn spawn_llm_request_with_backend(
        &mut self,
        messages: Vec<(String, String)>,
        backend: LlmBackend,
    ) {
        if let Some(handle) = self.llm.task_handle.take() {
            handle.abort();
        }
        self.llm.llm_request_start = Some(std::time::Instant::now());
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
            LlmBackend::Deepseek => {
                let url = self.config.deepseek_url.clone();
                let api_key = self.config.deepseek_api_key.clone();
                let model = self.config.deepseek_model.clone();
                let skill = self.llm.active_skill.clone();
                let handle = tokio::spawn(async move {
                    log::debug!("[LLM] Using DeepSeek backend ({} model={})", url, model);
                    let res =
                        query_deepseek(messages, &url, api_key.as_deref(), &model, skill.as_ref())
                            .await;
                    let _ = tx.send(res);
                });
                self.llm.task_handle = Some(handle);
            }
        }
    }

    pub fn tick_llm_prompt(&mut self) {
        if self.llm.task_handle.is_some() && self.git_commit_buffer_id.is_none() {
            self.tick_spinner();
            let spinner = SPINNER_CHARS[self.spinner_frame() % SPINNER_CHARS.len()];
            let elapsed = self
                .llm
                .llm_request_start
                .map(|t| t.elapsed().as_secs_f32())
                .unwrap_or(0.0);
            self.set_status_msg(
                &format!(
                    "{} LLM is thinking ({}) {:.1}s...",
                    spinner, self.config.llm_backend, elapsed
                ),
                crate::ed::mode::MessageKind::Info,
            );
        }
    }

    /// Polls completed responses from the background runtime channels.
    pub fn poll_llm_responses(&mut self) {
        while let Ok(res) = self.llm.response_rx.try_recv() {
            let _ = self.llm.task_handle.take();
            self.llm.llm_request_start = None;

            match res {
                Ok(llm_response) => {
                    let LlmResponse {
                        text: response_text,
                        tool_summaries,
                        total_rounds,
                    } = llm_response;

                    let tool_summary_block =
                        format_tool_summary_block(&tool_summaries, total_rounds);
                    let tool_info = if tool_summaries.is_empty() {
                        String::new()
                    } else {
                        format!(" [{} tool call(s)]", tool_summaries.len())
                    };

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
                                format!("{}…  [reg z]{}", &trimmed[..end], tool_info)
                            } else {
                                format!("{}  [reg z]{}", trimmed, tool_info)
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
                                buf.rope.insert(
                                    current_len,
                                    &format!("{}{}", tool_summary_block, wrapped),
                                );
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
                            buf.rope.insert(
                                current_len,
                                &format!("\nLLM: {}{}\n", tool_summary_block, wrapped),
                            );
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

                        self.set_status_msg(
                            &format!("Response is ready{}", tool_info),
                            MessageKind::Success,
                        );
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

        self.active_window_mut().set_buffer_id(input_id);

        self.focus_prev_window();
        self.active_window_mut().set_buffer_id(history_id);

        let prev_idx = self.active_window_idx;
        self.clamp_window_row_to_buf(prev_idx);

        self.focus_next_window();

        debug_assert_eq!(self.active_window().buffer_id(), input_id);

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

    pub fn llm_send_input_buffer(&mut self) -> CommandResult {
        let input_bid = self.active_window().buffer_id();
        let input = self.buf().rope.to_string();

        if input.trim().is_empty() {
            self.set_status_msg("Empty message — type something first", MessageKind::Info);
            return CommandResult::Handled;
        }

        if let Some(buf) = self.buf_mut_by_id(input_bid) {
            buf.rope = ropey::Rope::from_str("");
            buf.mark_modified();
            buf.parse_syntax();
        }

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

    pub fn llm_send_from_prompt(&mut self, input: String) -> CommandResult {
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
        self.llm.prompt_active = true;
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
                self.llm.prompt_active = false;
                self.llm_send_from_prompt(input)
            }
            PromptAction::Cancel => {
                self.llm.prompt.clear();
                self.llm.active_preset = None;
                self.llm.active_context = None;
                self.llm.todo_prefix = false;
                self.mode = Mode::Normal;
                self.llm.prompt_active = false;
                CommandResult::Handled
            }
            PromptAction::None => CommandResult::Handled,
        }
    }

    pub fn is_in_llm_input(&self) -> bool {
        let bid = self.active_window().buffer_id();
        self.buffers
            .iter()
            .any(|b| b.id == bid && b.kind == BufferKind::LlmInput)
    }

    pub fn is_in_llm_history(&self) -> bool {
        let bid = self.active_window().buffer_id();
        self.buffers
            .iter()
            .any(|b| b.id == bid && b.kind == BufferKind::Llm)
    }

    // ═══════════════════════════════════════════════════════════════
    // CodeLlm (single-buffer chat) methods
    // ═══════════════════════════════════════════════════════════════

    pub fn codellm_send(&mut self) {
        if self.buf().kind != BufferKind::CodeLlm {
            return;
        }

        if self.llm.task_handle.is_some() {
            self.set_status_msg("LLM is still responding…", MessageKind::Info);
            return;
        }

        let lock_line = self.buf().llm_lock_line;

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

        {
            let buf = self.buf_mut();
            buf.llm_lock_line = buf.len_lines();
            buf.rope.insert(buf.rope.len_chars(), "\n## Assistant\n\n");
            buf.parse_syntax();
        }

        {
            let total = self.buf().len_lines().saturating_sub(1);
            let win = self.active_window_mut();
            win.row = total;
            win.col = 0;
            win.desired_col = 0;
        }

        self.enter_normal();

        let full_prompt = if let Some(ctx) = &self.llm.active_context {
            format!("Selected code:\n```\n{}\n```\n\n{}", ctx, prompt)
        } else {
            prompt.clone()
        };

        self.llm_send_codellm(full_prompt);
    }

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

    pub fn codellm_finalize_response(&mut self) {
        let codellm_id = self
            .buffers
            .iter()
            .find(|b| b.kind == BufferKind::CodeLlm)
            .map(|b| b.id);

        if let Some(id) = codellm_id {
            if self.active_window().buffer_id() != id {
                self.switch_window_to_buffer(id);
            }

            let (last_line, _total_lines) = {
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

    pub fn llm_explain_function(&mut self) {
        let span_info = self.function_around_span_info();

        let Some(info) = span_info else {
            self.set_status_msg("No function found around cursor", MessageKind::Error);
            return;
        };

        let func_text = self.extract_line_range_text(info.start_row, info.end_row);

        let Some(func_text) = func_text else {
            self.set_status_msg("Failed to extract function text", MessageKind::Error);
            return;
        };

        if func_text.trim().is_empty() {
            self.set_status_msg("Function is empty", MessageKind::Error);
            return;
        }

        self.open_codellm_chat_session();

        let prompt = format!(
            "Explain this function:\n```\n{}\n```\n\nPlease explain what this function does, its parameters, and its return value.",
            func_text.trim_end()
        );

        let buf = self.buf_mut();
        if buf.kind == BufferKind::CodeLlm {
            let current_len = buf.rope.len_chars();
            buf.rope.insert(current_len, &prompt);
            buf.parse_syntax();

            let last_line = buf.len_lines().saturating_sub(1);
            let last_col = buf.line_char_len(last_line);

            let win = self.active_window_mut();
            win.row = last_line;
            win.col = last_col;
            win.desired_col = last_col;
        }

        self.codellm_send();
    }
}

// ═══════════════════════════════════════════════════════════════════
// Async query functions
// ═══════════════════════════════════════════════════════════════════

const MAX_TOOL_ROUNDS: usize = 15;

/// Query DeepSeek's OpenAI-compatible chat completions API.
/// Uses `reqwest` because DeepSeek requires HTTPS (api.deepseek.com).
async fn query_deepseek(
    messages: Vec<(String, String)>,
    base_url: &str,
    api_key: Option<&str>,
    model: &str,
    skill: Option<&Skill>,
) -> Result<LlmResponse, String> {
    let Some(skill) = skill else {
        return query_deepseek_plain(messages, base_url, api_key, model).await;
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

    let mut tool_summaries: Vec<ToolCallSummary> = Vec::new();

    for round in 0..MAX_TOOL_ROUNDS {
        let payload = build_deepseek_payload(&conversation.messages, model, Some(&tools_json));
        let response = send_deepseek_request(&payload, base_url, api_key).await?;
        let choice = &response["choices"][0]["message"];

        if let Some(tool_calls) = choice["tool_calls"].as_array() {
            if !tool_calls.is_empty() {
                conversation.push_assistant(choice);
                for tc in tool_calls {
                    let func = &tc["function"];
                    let tool_name = func["name"].as_str().unwrap_or("unknown").to_string();
                    let call_id = tc["id"].as_str().unwrap_or("").to_string();
                    let args_str = func["arguments"].as_str().unwrap_or("{}").to_string();
                    let args: serde_json::Value =
                        serde_json::from_str(&args_str).unwrap_or(serde_json::json!({}));
                    log::info!("[Tool] Calling {} with args: {}", tool_name, args_str);
                    let result = match skills::execute_tool(
                        &skill
                            .tools
                            .iter()
                            .find(|t| t.name == tool_name)
                            .ok_or_else(|| format!("Unknown tool: {}", tool_name))?
                            .clone(),
                        &args,
                        &skill.config_defaults,
                    ) {
                        Ok(r) => {
                            tool_summaries.push(ToolCallSummary {
                                tool_name: tool_name.clone(),
                                args_brief: truncate_args(&args_str, 80),
                                result_chars: r.len(),
                                error: None,
                            });
                            log::info!("[Tool] Result ({} chars)", r.len());
                            r
                        }
                        Err(e) => {
                            tool_summaries.push(ToolCallSummary {
                                tool_name: tool_name.clone(),
                                args_brief: truncate_args(&args_str, 80),
                                result_chars: 0,
                                error: Some(e.clone()),
                            });
                            log::info!("[Tool] Error: {}", e);
                            format!("Error: {}", e)
                        }
                    };
                    conversation.push_tool_result(&call_id, &tool_name, &result);
                }
                continue;
            }
        }

        if let Some(content) = choice["content"].as_str() {
            return Ok(LlmResponse {
                text: content.to_string(),
                tool_summaries,
                total_rounds: round + 1,
            });
        }
        return Err("DeepSeek returned empty response with no tool calls".into());
    }

    // Exhausted tool rounds — ask once more without tools
    let payload = build_deepseek_payload(&conversation.messages, model, None);
    let response = send_deepseek_request(&payload, base_url, api_key).await?;
    if let Some(content) = response["choices"][0]["message"]["content"].as_str() {
        return Ok(LlmResponse {
            text: content.to_string(),
            tool_summaries,
            total_rounds: MAX_TOOL_ROUNDS,
        });
    }
    Err("DeepSeek did not produce a final answer after max tool rounds".into())
}

/// Plain (no tool-calling) DeepSeek query.
async fn query_deepseek_plain(
    messages: Vec<(String, String)>,
    base_url: &str,
    api_key: Option<&str>,
    model: &str,
) -> Result<LlmResponse, String> {
    let json_messages: Vec<serde_json::Value> = messages
        .iter()
        .map(|(role, content)| serde_json::json!({"role": role, "content": content}))
        .collect();

    let payload = serde_json::json!({
        "model": model,
        "messages": json_messages,
        "max_tokens": 4096,
        "temperature": 0.7,
        "stream": false
    });

    let response = send_deepseek_request(&payload, base_url, api_key).await?;

    let content = response["choices"][0]["message"]["content"]
        .as_str()
        .ok_or_else(|| {
            let body = serde_json::to_string(&response).unwrap_or_default();
            format!("No content in DeepSeek response. Body: {}", body)
        })?;

    Ok(LlmResponse {
        text: content.to_string(),
        tool_summaries: Vec::new(),
        total_rounds: 1,
    })
}

/// Build a DeepSeek-compatible payload (OpenAI chat completions format with model).
fn build_deepseek_payload(
    messages: &[serde_json::Value],
    model: &str,
    tools: Option<&[serde_json::Value]>,
) -> serde_json::Value {
    let mut payload = serde_json::json!({
        "model": model,
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

/// Send a request to DeepSeek's API via `reqwest` (supports HTTPS + TLS).
async fn send_deepseek_request(
    payload: &serde_json::Value,
    base_url: &str,
    api_key: Option<&str>,
) -> Result<serde_json::Value, String> {
    let client = reqwest::Client::new();
    let url = format!("{}/v1/chat/completions", base_url.trim_end_matches('/'));

    let mut req = client.post(&url).header("Content-Type", "application/json");

    if let Some(key) = api_key {
        if !key.trim().is_empty() {
            req = req.header("Authorization", format!("Bearer {}", key.trim()));
        }
    }

    let response = req
        .json(payload)
        .send()
        .await
        .map_err(|e| format!("DeepSeek request failed: {}", e))?;

    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        return Err(format!("DeepSeek API error ({}): {}", status, body));
    }

    response
        .json::<serde_json::Value>()
        .await
        .map_err(|e| format!("Failed to parse DeepSeek response: {}", e))
}

/// Ollama with optional tool-calling support.
async fn query_ollama_local(
    messages: Vec<(String, String)>,
    url: &str,
    port: u16,
    model: &str,
    skill: Option<&Skill>,
) -> Result<LlmResponse, String> {
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

    let mut tool_summaries: Vec<ToolCallSummary> = Vec::new();

    for round in 0..MAX_TOOL_ROUNDS {
        let payload = build_ollama_payload(&conversation.messages, model, Some(&tools_json));
        let response = send_ollama_request_raw(&payload, url, port).await?;
        let choice = &response["message"];

        if let Some(tool_calls) = choice["tool_calls"].as_array() {
            if !tool_calls.is_empty() {
                conversation.push_assistant(choice);
                for tc in tool_calls {
                    let func = &tc["function"];
                    let tool_name = func["name"].as_str().unwrap_or("unknown").to_string();
                    let call_id = tc["id"].as_str().unwrap_or("").to_string();
                    let args_str = func["arguments"].as_str().unwrap_or("{}").to_string();
                    let args: serde_json::Value =
                        serde_json::from_str(&args_str).unwrap_or(serde_json::json!({}));

                    log::info!("[Tool] Calling {} with args: {}", tool_name, args_str);

                    let result = match skills::execute_tool(
                        &skill
                            .tools
                            .iter()
                            .find(|t| t.name == tool_name)
                            .ok_or_else(|| format!("Unknown tool: {}", tool_name))?
                            .clone(),
                        &args,
                        &skill.config_defaults,
                    ) {
                        Ok(r) => {
                            tool_summaries.push(ToolCallSummary {
                                tool_name: tool_name.clone(),
                                args_brief: truncate_args(&args_str, 80),
                                result_chars: r.len(),
                                error: None,
                            });
                            log::info!("[Tool] Result ({} chars)", r.len());
                            r
                        }
                        Err(e) => {
                            tool_summaries.push(ToolCallSummary {
                                tool_name: tool_name.clone(),
                                args_brief: truncate_args(&args_str, 80),
                                result_chars: 0,
                                error: Some(e.clone()),
                            });
                            log::info!("[Tool] Error: {}", e);
                            format!("Error: {}", e)
                        }
                    };

                    conversation.push_tool_result(&call_id, &tool_name, &result);
                }
                continue;
            }
        }

        if let Some(content) = choice["content"].as_str() {
            return Ok(LlmResponse {
                text: content.to_string(),
                tool_summaries,
                total_rounds: round + 1,
            });
        }

        return Err("Ollama returned empty response with no tool calls".into());
    }

    // Fallback after max rounds: force a final answer without tools
    let payload = build_ollama_payload(&conversation.messages, model, None);
    let response = send_ollama_request_raw(&payload, url, port).await?;
    if let Some(content) = response["message"]["content"].as_str() {
        return Ok(LlmResponse {
            text: content.to_string(),
            tool_summaries,
            total_rounds: MAX_TOOL_ROUNDS,
        });
    }
    Err("Ollama did not produce a final answer after max tool rounds".into())
}

/// Plain Ollama chat (no tools).
async fn query_ollama_plain(
    messages: Vec<(String, String)>,
    url: &str,
    port: u16,
    model: &str,
) -> Result<LlmResponse, String> {
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
    Ok(LlmResponse {
        text: parsed.message.content,
        tool_summaries: Vec::new(),
        total_rounds: 1,
    })
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

/// llama.cpp with optional tool-calling support.
async fn query_llamacpp_local(
    messages: Vec<(String, String)>,
    url: &str,
    port: u16,
    api_key: Option<&str>,
    skill: Option<&Skill>,
) -> Result<LlmResponse, String> {
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

    let mut tool_summaries: Vec<ToolCallSummary> = Vec::new();

    for round in 0..MAX_TOOL_ROUNDS {
        let payload = build_llm_payload(&conversation.messages, Some(&tools_json));
        let response = send_llm_request_raw(&payload, url, port, api_key).await?;
        let choice = &response["choices"][0]["message"];

        if let Some(tool_calls) = choice["tool_calls"].as_array() {
            if !tool_calls.is_empty() {
                conversation.push_assistant(choice);
                for tc in tool_calls {
                    let func = &tc["function"];
                    let tool_name = func["name"].as_str().unwrap_or("unknown").to_string();
                    let call_id = tc["id"].as_str().unwrap_or("").to_string();
                    let args_str = func["arguments"].as_str().unwrap_or("{}").to_string();
                    let args: serde_json::Value =
                        serde_json::from_str(&args_str).unwrap_or(serde_json::json!({}));

                    log::info!("[Tool] Calling {} with args: {}", tool_name, args_str);

                    let result = match skills::execute_tool(
                        &skill
                            .tools
                            .iter()
                            .find(|t| t.name == tool_name)
                            .ok_or_else(|| format!("Unknown tool: {}", tool_name))?
                            .clone(),
                        &args,
                        &skill.config_defaults,
                    ) {
                        Ok(r) => {
                            tool_summaries.push(ToolCallSummary {
                                tool_name: tool_name.clone(),
                                args_brief: truncate_args(&args_str, 80),
                                result_chars: r.len(),
                                error: None,
                            });
                            log::info!("[Tool] Result ({} chars)", r.len());
                            r
                        }
                        Err(e) => {
                            tool_summaries.push(ToolCallSummary {
                                tool_name: tool_name.clone(),
                                args_brief: truncate_args(&args_str, 80),
                                result_chars: 0,
                                error: Some(e.clone()),
                            });
                            log::info!("[Tool] Error: {}", e);
                            format!("Error: {}", e)
                        }
                    };

                    conversation.push_tool_result(&call_id, &tool_name, &result);
                }
                continue;
            }
        }

        if let Some(content) = choice["content"].as_str() {
            return Ok(LlmResponse {
                text: content.to_string(),
                tool_summaries,
                total_rounds: round + 1,
            });
        }

        return Err("LLM returned empty response with no tool calls".into());
    }

    // Fallback after max rounds: force a final answer without tools
    let payload = build_llm_payload(&conversation.messages, None);
    let response = send_llm_request_raw(&payload, url, port, api_key).await?;
    if let Some(content) = response["choices"][0]["message"]["content"].as_str() {
        return Ok(LlmResponse {
            text: content.to_string(),
            tool_summaries,
            total_rounds: MAX_TOOL_ROUNDS,
        });
    }
    Err("LLM did not produce a final answer after max tool rounds".into())
}

/// Plain llama.cpp chat (no tools).
async fn query_llamacpp_plain(
    messages: Vec<(String, String)>,
    url: &str,
    port: u16,
    api_key: Option<&str>,
) -> Result<LlmResponse, String> {
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
    Ok(LlmResponse {
        text: parsed.choices[0].message.content.clone(),
        tool_summaries: Vec::new(),
        total_rounds: 1,
    })
}

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

// ═══════════════════════════════════════════════════════════════════
// LLM action helpers
// ═══════════════════════════════════════════════════════════════════

impl Editor {
    pub fn llm_review(&mut self) {
        let (code, instruction) = self.get_code_context(
            "Review this code for bugs, performance issues, and style improvements.",
        );
        self.llm_action_helper(
            "You are an expert senior software engineer performing a code review. Be concise and constructive.",
            &code,
            &instruction,
            true
        );
    }

    pub fn llm_fix(&mut self) {
        let (code, base_instruction) = self.get_code_context("Fix the issues in this code.");
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
            true
        );
    }

    pub fn llm_add_to_chat(&mut self) {
        let (code, _) = self.get_code_context("");
        self.llm_action_helper(
            "You are a helpful coding assistant.",
            &code,
            "I am adding this code to the context. My question is: ",
            false,
        );
    }

    pub fn llm_generate(&mut self) {
        self.llm_action_helper(
            "You are an expert coder. Output ONLY the requested code inside a markdown block.",
            "",
            "Generate the following: ",
            false,
        );
    }

    pub fn get_code_context(&self, default_instruction: &str) -> (String, String) {
        if let Some((r1, r2)) = self.get_visual_line_range() {
            if let Some(text) = self.extract_line_range_text(r1, r2) {
                return (text, default_instruction.to_string());
            }
        }
        if let Some(info) = self.function_around_span_info() {
            if let Some(text) = self.extract_line_range_text(info.start_row, info.end_row) {
                return (text, default_instruction.to_string());
            }
        }
        (String::new(), default_instruction.to_string())
    }
}

impl Editor {
    fn llm_action_helper(
        &mut self,
        system_prompt: &str,
        code: &str,
        instruction: &str,
        auto_send: bool,
    ) {
        let existing_id = self
            .buffers
            .iter()
            .find(|b| b.kind == BufferKind::CodeLlm)
            .map(|b| b.id);

        let chat_id = if let Some(id) = existing_id {
            self.switch_window_to_buffer(id);
            id
        } else {
            self.open_codellm_chat_session();
            self.buf().id
        };

        let payload = format!("Code:\n```\n{}\n```\n\n{}", code.trim_end(), instruction);
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
    out.pop();
    out
}
