//! Central editor state and key dispatch.
use anyhow::Result;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use std::collections::HashSet;

use crate::comp::state::CompletionMachine;
use crate::config::app_config::Config;
use crate::ed::buffer::{Buffer, BufferKind};
use crate::ed::mode::{MessageKind, Mode};
use crate::ed::syntax::TextObject;
use crate::ed::window::{LayoutNode, Window};
use crate::popup::{PopupItem, PopupKind, PopupState};
use crate::render::statusbar_state::StatusBarState;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PendingGitAction {
    None,
    SwitchBranch(String),
    PopStash(String),
}

#[derive(Debug, serde::Serialize, serde::Deserialize, Clone)]
pub struct PositionInfo {
    pub path: String,
    pub row: usize,
    pub col: usize,
}

/// What kind of character the editor is waiting for after a prefix key.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PendingInput {
    None,
    /// `m` pressed — waiting for the bookmark letter.
    SetBookmark,
    /// `` ` `` pressed — waiting for the bookmark letter or a second
    /// backtick for ping-pong.
    GotoBookmark,
}

// ---------------------------------------------------------------------------
// QuitPrompt
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QuitPrompt {
    None,
    BufferQuit,
    QuitAllConfirm,
}

// ---------------------------------------------------------------------------
// Editor
// ---------------------------------------------------------------------------

pub struct Editor {
    pub config: Config,

    /// All open buffers (documents).
    pub buffers: Vec<Buffer>,

    /// All open windows (viewports into buffers).
    pub windows: Vec<Window>,

    /// Layout tree describing how windows are arranged on screen.
    pub layout: LayoutNode,

    /// Index into `windows` for the currently focused window.
    pub active_window_idx: usize,

    pub next_buf_id: usize,
    pub next_win_id: usize,

    pub mode: Mode,
    pub comp: CompletionMachine,

    pub command: String,
    pub status_msg: String,
    pub status_kind: MessageKind,
    pub status_time: std::time::Instant,
    pub should_quit: bool,
    pub lsp_loading: bool,
    pub spinner_frame: usize,

    pub pending_keys: String,
    pub popup: PopupState,
    pub config_bool_keys: Vec<String>,

    pub vocab_words: HashSet<String>,
    pub buffer_words: Vec<String>,

    pub clipboard: Option<String>,

    pub cmd_history: Vec<String>,
    pub cmd_history_idx: Option<usize>,
    pub cmd_temp_input: String,
    pub history_search_prefix: Option<String>,

    pub status_state: StatusBarState,
    pub prev_mode: Mode,
    /// Last key pressed while scankey popup is active: (key_str, action_description)
    pub scankey_info: Option<(String, String, String)>,

    pub last_action: crate::ed::repeat::LastAction,
    pub repeat_pending: bool,
    pub current_count: usize,
    pub insert_buffer: Option<String>,
    pub last_search_query: Option<String>,
    pub mru_manager: crate::popup::mru::MruManager,
    pub positions: Vec<PositionInfo>,
    pub async_gutter: crate::git::gutter::AsyncGutterWorker,
    pub git_debounce: crate::git::debounce::DebounceManager,
    pub last_rg_pattern: Option<String>,
    pub last_rg_root_dir: Option<std::path::PathBuf>,
    pub last_rg_output: Option<crate::ed::ripgrep::RipgrepOutput>,
    pub quickfix_results: Vec<crate::ed::ripgrep::RipgrepResult>,
    pub quickfix_index: usize,
    pub pending_input: PendingInput,
    pub pending_git_action: PendingGitAction,
    pub git_commit_buffer_id: Option<usize>,
    pub git_commit_start_time: Option<std::time::Instant>,
    pub llm: crate::ai::llama::llm::LlmState,
    pub cmd_waiting_register: bool,

    //-- struct Editor (anchor dont removed) --//
    pub quit_prompt: QuitPrompt,
}

/// Maximum number of simultaneous windows.
const MAX_WINDOWS: usize = 8;

// ---------------------------------------------------------------------------
// Constructor
// ---------------------------------------------------------------------------

impl Editor {
    pub fn new(filename: Option<String>) -> Result<Self> {
        let first_buf = Buffer::new(0, filename.clone())?; // Cloned to reuse below
        let config = Config::load().unwrap_or_default();
        let cmd_history = Self::load_history();
        let vocab_words = Self::preload_vocabulary();
        let positions = Self::load_positions(); // Load persistent position history

        let mut first_win = Window::new(0, first_buf.id);

        // Restore saved cursor position for startup file if it exists
        if let Some(ref name) = filename {
            let path_buf = std::path::PathBuf::from(name);
            let canon_path = std::fs::canonicalize(&path_buf)
                .unwrap_or(path_buf)
                .to_string_lossy()
                .to_string();
            if let Some(pos) = positions.iter().find(|p| p.path == canon_path) {
                first_win.row = pos.row.min(first_buf.len_lines().saturating_sub(1));
                first_win.col = pos.col.min(first_buf.line_char_len(first_win.row));
            }
        }
        let layout = LayoutNode::leaf(first_win.id);
        let mut editor = Self {
            config,
            buffers: vec![first_buf],
            windows: vec![first_win],
            layout,
            active_window_idx: 0,
            next_buf_id: 1,
            next_win_id: 1,
            mode: Mode::Normal,
            comp: CompletionMachine::new(),
            command: String::new(),
            status_msg: String::new(),
            status_kind: MessageKind::Info,
            status_time: std::time::Instant::now() - std::time::Duration::from_secs(10),
            should_quit: false,
            lsp_loading: true,
            spinner_frame: 0,
            pending_keys: String::new(),
            popup: PopupState::new(),
            config_bool_keys: Vec::new(),
            vocab_words,
            buffer_words: Vec::new(),
            clipboard: None,
            cmd_history,
            cmd_history_idx: None,
            cmd_temp_input: String::new(),
            history_search_prefix: None,
            scankey_info: None,
            prev_mode: Mode::Normal,
            status_state: StatusBarState::default(),
            last_search_query: None,
            mru_manager: crate::popup::mru::MruManager::load(),
            positions,
            async_gutter: crate::git::gutter::AsyncGutterWorker::new(),
            git_debounce: crate::git::debounce::DebounceManager::new(),
            last_rg_pattern: None,
            last_rg_root_dir: None,
            last_rg_output: None,
            quickfix_results: Vec::new(),
            quickfix_index: 0,
            pending_input: PendingInput::None,
            pending_git_action: PendingGitAction::None,
            git_commit_buffer_id: None,
            git_commit_start_time: None,
            llm: crate::ai::llama::llm::LlmState::new(),
            cmd_waiting_register: false,

            //-- Editor fn new() (anchor dont removed) --//
            last_action: crate::ed::repeat::LastAction::default(),
            repeat_pending: false,
            current_count: 1,
            insert_buffer: None,
            quit_prompt: QuitPrompt::None,
        };

        // Record initial file in MRU if provided at startup with the correct position
        if let Some(name) = editor.active_filename() {
            let path_buf = std::path::PathBuf::from(name);
            let canon_path = std::fs::canonicalize(&path_buf).unwrap_or(path_buf);
            let win = editor.active_window();
            editor.mru_manager.insert(canon_path, win.row, win.col);
        }

        if editor.config.init_mode == "brief" {
            editor.enter_brief();
        }

        let init_msg = if editor.config.init_mode == "brief" {
            "Brief mode | F9 for :commands, :vim to vim mode"
        } else {
            "Type i to insert mode, gi to brief mode, :q to quit, :e <path> to open another file"
        };
        editor.set_status(init_msg, MessageKind::Info);

        if let Some(ref name) = filename {
            let bid = editor.active_window().buffer_id();
            let rope = editor.buf().rope.clone();
            log::debug!("Editor::new: Queueing initial startup diff for {}", name);
            editor.async_gutter.request_diff(bid, &rope, Some(name));
        }

        editor.refresh_buffer_words();
        Ok(editor)
    }

    /// Current scope (impl::function) at the active cursor position.
    pub fn current_scope(&self) -> Option<String> {
        let win = self.active_window();
        let buf = self.buf();
        buf.current_scope(win.row, win.col)
    }
}

// ---------------------------------------------------------------------------
// Active-window / active-buffer accessors
// ---------------------------------------------------------------------------

impl Editor {
    // -- Open Marks Popup --
    pub fn open_marks_popup(&mut self) {
        let mut entries = Vec::new();
        for buf in &self.buffers {
            for (&c, &(r, co)) in &buf.named_bookmarks {
                entries.push(crate::popup::MarkEntry {
                    ch: c,
                    row: r,
                    col: co,
                    buffer_id: buf.id,
                    buffer_name: buf.display_name(),
                });
            }
        }

        if entries.is_empty() {
            self.set_status_msg("No marks set", MessageKind::Info);
            return;
        }

        self.popup.marks = Some(crate::popup::MarksPopup::new(entries));
        self.popup.kind = Some(crate::popup::PopupKind::Marks);
    }

    /// Called by the `:scankey` command.
    pub fn open_scankey_popup(&mut self) {
        self.popup.open_scankey(
            "...".to_string(),
            "Waiting for keypress".to_string(),
            "".to_string(),
        );

        self.scankey_info = Some((
            "...".to_string(),
            "Waiting for keypress".to_string(),
            "".to_string(),
        ));
        self.set_status_msg(
            "Scankey: Press any key to inspect, 'q' to quit",
            MessageKind::Info,
        );
    }

    /// The active window.
    #[inline]
    pub fn active_window(&self) -> &Window {
        &self.windows[self.active_window_idx]
    }

    /// The active window, mutably.
    #[inline]
    pub fn active_window_mut(&mut self) -> &mut Window {
        &mut self.windows[self.active_window_idx]
    }

    /// Find a buffer by its ID. Returns None only for a state bug
    /// (window referencing a removed buffer).
    pub fn buf_by_id(&self, id: usize) -> Option<&Buffer> {
        self.buffers.iter().find(|b| b.id == id)
    }

    pub fn buf_mut_by_id(&mut self, id: usize) -> Option<&mut Buffer> {
        self.buffers.iter_mut().find(|b| b.id == id)
    }

    /// The buffer viewed by the active window.
    /// Falls back to the first buffer if the ID is stale.
    #[inline]
    pub fn buf(&self) -> &Buffer {
        let bid = self.active_window().buffer_id();
        let exists = self.buffers.iter().any(|b| b.id == bid);
        if !exists {
            // Stale buffer_id — we can't repair in &self, return first buffer
            return self
                .buffers
                .first()
                .expect("at least one buffer must exist");
        }
        self.buffers
            .iter()
            .find(|b| b.id == bid)
            .expect("buffer must exist after check")
    }

    /// The buffer viewed by the active window, mutably.
    #[inline]
    pub fn buf_mut(&mut self) -> &mut Buffer {
        let bid = self.active_window().buffer_id();
        let exists = self.buffers.iter().any(|b| b.id == bid);
        if !exists {
            // Stale buffer_id — reset to the first buffer
            let first_id = self.buffers.first().map(|b| b.id).unwrap_or(0);
            self.windows[self.active_window_idx].set_buffer_id(first_id);
        }
        let target_id = self.active_window().buffer_id();
        self.buffers
            .iter_mut()
            .find(|b| b.id == target_id)
            .expect("buffer must exist after repair")
        // buf.diff_alignment = None;
    }

    /// Disjoint mutable access to the active window **and** its buffer.
    pub fn active_window_and_buf_mut(&mut self) -> (&mut Window, &mut Buffer) {
        let bid = self.windows[self.active_window_idx].buffer_id();
        let exists = self.buffers.iter().any(|b| b.id == bid);

        if !exists {
            let first_id = self.buffers.first().map(|b| b.id).unwrap_or(0);
            self.windows[self.active_window_idx].set_buffer_id(first_id);
        }

        let target_bid = self.windows[self.active_window_idx].buffer_id();

        let win = &mut self.windows[self.active_window_idx];
        let buf = self
            .buffers
            .iter_mut()
            .find(|b| b.id == target_bid)
            .expect("buffer must exist after repair");

        (win, buf)
    }

    /// Alias kept for `main.rs` compatibility.
    #[inline]
    pub fn active_buf(&self) -> &Buffer {
        self.buf()
    }
}

// ---------------------------------------------------------------------------
// Quit
// ---------------------------------------------------------------------------

impl Editor {
    pub fn force_quit(&mut self) {
        self.save_all_window_positions();
        self.should_quit = true;
    }

    pub fn quit_all_check(&mut self) {
        // Locate the first buffer in the list that has unsaved modifications
        let first_dirty_bid = self.buffers.iter().find(|b| b.modified).map(|b| b.id);

        if let Some(bid) = first_dirty_bid {
            // Switch the viewport focus to the dirty buffer so the user sees what they are prompting
            self.switch_window_to_buffer(bid);

            self.quit_prompt = QuitPrompt::QuitAllConfirm;
            let name = self.buf().display_name();
            self.set_status_msg(
                &format!("Save changes to {}? (y/n/c)", name),
                MessageKind::Error,
            );
        } else {
            // No modified buffers left; save window positions and exit safely
            self.save_all_window_positions();
            self.should_quit = true;
        }
    }

    pub fn quit_check(&mut self) {
        if !self.buf().modified {
            if self.buffers.len() > 1 {
                self.close_buffer();
            } else {
                self.save_all_window_positions();
                self.should_quit = true;
            }
        } else {
            self.quit_prompt = QuitPrompt::BufferQuit;
            self.set_status_msg("Save changes? (y/n/c)", MessageKind::Error);
        }
    }
}

// ---------------------------------------------------------------------------
// Pending keys
// ---------------------------------------------------------------------------

impl Editor {
    pub fn clear_pending_keys(&mut self) {
        self.pending_keys.clear();
    }
}

// ---------------------------------------------------------------------------
// Config popup
// ---------------------------------------------------------------------------

impl Editor {
    pub fn open_config_popup(&mut self) {
        let mut items = Vec::new();
        let mut bool_keys = Vec::new();

        if let Ok(serde_json::Value::Object(map)) = serde_json::to_value(&self.config) {
            for (key, value) in map {
                if let serde_json::Value::Bool(val) = value {
                    let status = if val { " ON" } else { "OFF" };
                    let human_key = key
                        .split('_')
                        .map(|w| {
                            let mut c = w.chars();
                            c.next()
                                .map(|f| f.to_ascii_uppercase().to_string() + c.as_str())
                                .unwrap_or_default()
                        })
                        .collect::<Vec<_>>()
                        .join(" ");

                    items.push(PopupItem {
                        label: format!("{:<20} : [{}]", human_key, status),
                        detail: Some(key.clone()),
                        data: bool_keys.len(),
                    });
                    bool_keys.push(key);
                }
            }
        }

        self.config_bool_keys = bool_keys;
        self.popup.open_config(items, 0);
    }
}

// ---------------------------------------------------------------------------
// Word index
// ---------------------------------------------------------------------------

impl Editor {
    pub fn refresh_buffer_words(&mut self) {
        let total = self.buf().len_lines();
        log::debug!("refresh_buffer_words: {} lines", total);
        let mut words = HashSet::new();
        let buf = self.buf();
        for i in 0..total {
            for w in buf
                .line_text(i)
                .split(|c: char| !c.is_alphanumeric() && c != '_')
            {
                if w.len() >= 6 {
                    words.insert(w.to_string());
                }
            }
        }
        self.buffer_words = words.into_iter().collect();
    }
}

// ---------------------------------------------------------------------------
// Vocabulary
// ---------------------------------------------------------------------------

impl Editor {
    fn preload_vocabulary() -> HashSet<String> {
        let mut words = HashSet::new();
        if let Ok(dir) = Config::config_dir() {
            let path = dir.join("wordlist.txt");
            if let Ok(content) = std::fs::read_to_string(path) {
                for line in content.lines() {
                    let t = line.trim();
                    if !t.is_empty() {
                        words.insert(t.to_string());
                    }
                }
            }
        }
        words
    }

    pub fn add_vocab_word(&mut self, word: &str) -> anyhow::Result<()> {
        let trimmed = word.trim().to_string();
        if trimmed.is_empty() {
            return Ok(());
        }
        if self.vocab_words.insert(trimmed.clone()) {
            if let Ok(dir) = Config::config_dir() {
                use std::io::Write;
                let mut file = std::fs::OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(dir.join("wordlist.txt"))?;
                writeln!(file, "{}", trimmed)?;
            }
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Key dispatch  (delegates cursor work to Window)
// ---------------------------------------------------------------------------

impl Editor {
    pub fn handle_key(&mut self, key: KeyEvent) {
        if key.kind != crossterm::event::KeyEventKind::Press {
            return;
        }

        // ── Intercept for Pending Git Actions ────────────────────────
        if self.pending_git_action != PendingGitAction::None {
            self.handle_git_action_prompt_key(key);
            return;
        }

        // Intercept for FilePicker first
        if self.popup.file_picker.is_some() {
            self.handle_file_picker_key(key);
            return;
        }

        // ── Intercept for BufferList ──────────────────────────────────
        if self.popup.buffer_list.is_some() {
            self.handle_buffer_list_key(key);
            return;
        }

        // Intercept for FunctionListPopup second (before general is_open checks)
        if self.popup.function_list.is_some() {
            self.handle_function_list_key(key);
            return;
        }

        if self.popup.mru.is_some() {
            self.handle_mru_key(key);
            return;
        }

        if self.popup.marks.is_some() {
            self.handle_marks_key(key);
            return;
        }

        if self.popup.git_hunk.is_some() {
            self.handle_git_hunk_popup_key(key);
            return;
        }

        // WhichKey is a passive overlay, don't intercept keys for it!
        if self.popup.is_open() && self.popup.kind != Some(PopupKind::WhichKey) {
            self.handle_popup_key(key);
            return;
        }

        // ── Intercept for LLM Input Buffer overrides ──────────────────
        if self.buf().kind == BufferKind::LlmInput {
            if let Some(res) = self.handle_llm_input_buffer_key(&key) {
                if matches!(res, crate::ed::ext::CommandResult::Handled) {
                    return;
                }
            }
        }

        // ── Pending bookmark / quickmark input ──────────────────────────
        if self.pending_input != PendingInput::None
            && matches!(self.mode, Mode::Normal | Mode::Visual | Mode::VisualLine)
        {
            match key.code {
                KeyCode::Esc => {
                    self.pending_input = PendingInput::None;
                    self.clear_status_msg();
                }

                KeyCode::Char(c) if c.is_ascii_lowercase() || c == '`' => {
                    match self.pending_input {
                        PendingInput::SetBookmark => {
                            self.set_named_bookmark(c);
                        }
                        PendingInput::GotoBookmark => {
                            if c == '`' {
                                self.jump_last_position();
                            } else {
                                self.goto_named_bookmark(c);
                            }
                        }
                        PendingInput::None => unreachable!(),
                    }
                    self.pending_input = PendingInput::None;
                }

                // Any other key cancels
                _ => {
                    self.pending_input = PendingInput::None;
                    self.clear_status_msg();
                }
            }
            return;
        }

        if self.popup.is_open() {
            self.handle_popup_key(key);
            return;
        }

        if self.quit_prompt != QuitPrompt::None {
            self.handle_quit_prompt_key(key);
            return;
        }

        if key.modifiers.contains(KeyModifiers::ALT) && key.code == KeyCode::Char('Q') {
            self.save_all_window_positions();
            self.should_quit = true;
            return;
        }

        // ── Normal / Insert / Command / Search / Visual handling ─────
        let ghost_active = self.comp.has_ghost();

        //-- 1. Intercept typing and command modes (Insert/Brief/Command) and return early --//
        if self.mode == Mode::Insert || self.mode == Mode::Command || self.mode == Mode::Search {
            let key_str = crate::keybind::bindings::format_key(key);
            if let Some(action) = crate::keybind::bindings::resolve_single_key(
                &self.config,
                &key_str,
                self.mode,
                ghost_active,
                key,
            ) {
                crate::keybind::execute_action(self, action);
            }
            return;
        }

        // ═══════════════════════════════════════════════════════════════
        // ── 2. Special-buffer key overrides (GitLog / GitDiff) ──────────
        // ═══════════════════════════════════════════════════════════════
        let buf_kind = self.buf().kind;
        if buf_kind != BufferKind::Normal {
            let handled = match buf_kind {
                BufferKind::GitLog => self.handle_git_log_key(key),
                BufferKind::GitDiff => self.handle_git_diff_key(key),
                BufferKind::GitDiffHead => self.handle_git_diff_key(key),
                BufferKind::Ripgrep => self.handle_ripgrep_key(key),
                BufferKind::GitCommit => self.handle_git_commit_key(key),
                BufferKind::GitStatus => self.handle_git_status_key(key),
                _ => false,
            };
            if handled {
                return; // Early return only if consumed by the override
            }
        }

        // 3. Try visual-specific commands first, but fall through to allow hjkl movements
        if self.mode == Mode::Visual || self.mode == Mode::VisualLine {
            let key_str = crate::keybind::bindings::format_key(key);
            if let Some(action) = crate::keybind::bindings::resolve_single_key(
                &self.config,
                &key_str,
                self.mode,
                ghost_active,
                key,
            ) {
                crate::keybind::execute_action(self, action);
                return; // Only return if a visual command (like y, d, c, esc) was executed
            }
            // Fall through to allow hjkl, word forward, G, gg, etc.
        }

        let key_str = crate::keybind::bindings::format_key(key);
        if key_str.is_empty() {
            return;
        }

        if key_str == "esc" {
            let mut handled = false;
            if !self.pending_keys.is_empty() {
                self.clear_pending_keys();
                handled = true;
            }
            if !self.status_msg.is_empty() {
                self.clear_status_msg();
                handled = true;
            }
            if handled {
                return;
            }
        }

        let new_seq = if self.pending_keys.is_empty() {
            key_str
        } else {
            format!("{} {}", self.pending_keys, key_str)
        };

        match crate::keybind::bindings::resolve_sequence(
            &self.config,
            &new_seq,
            ghost_active,
            self.mode,
        ) {
            crate::keybind::bindings::ResolveResult::Action(action)
            | crate::keybind::bindings::ResolveResult::AutoAction(action) => {
                self.clear_pending_keys();
                crate::keybind::execute_action(self, action);
            }
            crate::keybind::bindings::ResolveResult::Pending => {
                self.pending_keys = new_seq.clone();
            }
            crate::keybind::bindings::ResolveResult::None => {
                self.clear_pending_keys();

                // ── Fallback for Brief Mode single-key presses ────────────
                if self.mode == Mode::Brief {
                    let key_str = crate::keybind::bindings::format_key(key);
                    if let Some(action) = crate::keybind::bindings::resolve_single_key(
                        &self.config,
                        &key_str,
                        self.mode,
                        ghost_active,
                        key,
                    ) {
                        crate::keybind::execute_action(self, action);
                    }
                }
            }
        }
        // ── Keep side-by-side viewports synchronized after executing any action ──
        self.sync_diff_windows();
    }

    pub fn edit_text_object(&mut self, obj: TextObject, inside: bool, change: bool) {
        let (row, col) = {
            let win = self.active_window();
            (win.row, win.col)
        };

        if let Some((sr, sc, er, ec)) = self.buf().syntax.text_object_range(row, col, obj, inside) {
            let (win, buf) = self.active_window_and_buf_mut();

            // PANIC-FIX: validate ranges before rope operations
            if sr >= buf.len_lines() || er >= buf.len_lines() {
                self.set_status_msg("Invalid text object range", MessageKind::Error);
                return;
            }

            let start_offset = buf.rope.line_to_char(sr).saturating_add(sc);
            let end_offset = buf.rope.line_to_char(er).saturating_add(ec);

            // PANIC-FIX: ensure valid range
            if end_offset <= start_offset || end_offset > buf.rope.len_chars() {
                self.set_status_msg("Invalid text object range", MessageKind::Error);
                return;
            }

            buf.push_undo(win.row, win.col);
            buf.rope.remove(start_offset..end_offset);

            win.row = sr;
            win.col = sc;
            // PANIC-FIX: clamp cursor after mutation
            win.col = win.col.min(buf.line_char_len(win.row));
            buf.modified = true;

            if change {
                self.enter_insert();
            }
        } else {
            self.set_status_msg("No text object found", MessageKind::Error);
        }
    }

    pub fn trigger_buffer_list_popup(&mut self) {
        let active_bid = self.active_window().buffer_id();

        // 1. Gather all open buffers
        let mut entries: Vec<crate::popup::BufferEntry> = self
            .buffers
            .iter()
            .map(|buf| {
                let name = buf
                    .filename
                    .as_ref()
                    .and_then(|f| std::path::Path::new(f).file_name())
                    .map(|f| f.to_string_lossy().to_string())
                    .unwrap_or_else(|| "[No Name]".to_string());

                crate::popup::BufferEntry {
                    id: buf.id,
                    name,
                    path: buf.filename.as_ref().map(std::path::PathBuf::from),
                    is_modified: buf.modified,
                    line_count: buf.len_lines(),
                }
            })
            .collect();

        // 2. Sort by recency (MRU order) using your self.positions array
        let positions = &self.positions;
        entries.sort_by_key(|entry| {
            if entry.id == active_bid {
                return std::cmp::Reverse(usize::MAX); // Current active buffer is always first
            }

            if let Some(ref path_buf) = entry.path {
                let canon_path = std::fs::canonicalize(path_buf)
                    .unwrap_or_else(|_| path_buf.clone())
                    .to_string_lossy()
                    .to_string();

                // Higher index in positions means more recently accessed
                if let Some(idx) = positions.iter().position(|p| p.path == canon_path) {
                    return std::cmp::Reverse(idx);
                }
            }

            // Fallback for unnamed/untracked buffers
            std::cmp::Reverse(0)
        });

        self.popup.open_buffer_list(entries);
    }
}
