//! Central editor state and key dispatch.
use crate::keybind::bindings::Action;
use anyhow::Result;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use std::collections::HashSet;

use crate::comp::state::CompletionMachine;
use crate::config::app_config::Config;
use crate::ed::buffer::{Buffer, BufferKind};
use crate::ed::mode::{MessageKind, Mode};
use crate::ed::syntax::TextObject;
use crate::ed::window::{LayoutNode, Window};
use crate::keybind::bindings::FunctionSpanInfo;
use crate::popup::{PopupItem, PopupKind, PopupState};
use crate::render::statusbar_state::StatusBarState;

#[derive(Debug, Clone)]
pub struct VisualBlockInsertState {
    pub rows: Vec<usize>, // The rows in the rectangular selection
    pub col: usize,       // The target column of the insertion
}

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
    pub clipboard_is_block: bool,
    pub visual_block_insert_state: Option<VisualBlockInsertState>,

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
    pub command_cursor: usize,
    pub needs_initial_scroll: bool,
    pub pending_register: bool,

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
            command_cursor: 0,
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
            clipboard_is_block: false,
            visual_block_insert_state: None,
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
            needs_initial_scroll: true,
            pending_register: false,

            //-- Editor fn new() (anchor dont removed) --//
            last_action: crate::ed::repeat::LastAction::default(),
            repeat_pending: false,
            current_count: 0,
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

        if editor.config.show_startup_hints {
            let init_msg = if editor.config.init_mode == "brief" {
                "Brief mode | F9 for :commands, :vim to vim mode"
            } else {
                "Type i to insert mode, gi to brief mode, :q to quit, :e <path> to open another file"
            };
            editor.set_status(init_msg, MessageKind::Info);
        }

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
            // 1. Vim-style named bookmarks (also includes auto-incremented a..z from Alt+M)
            for (&c, &(r, co)) in &buf.named_bookmarks {
                entries.push(crate::popup::MarkEntry {
                    ch: c,
                    row: r,
                    col: co,
                    buffer_id: buf.id,
                    buffer_name: buf.display_name(),
                });
            }

            // 2. Any line bookmarks that don't have a named character
            for &row in &buf.bookmarks {
                let has_named = buf.named_bookmarks.values().any(|(r, _)| *r == row);
                if !has_named {
                    entries.push(crate::popup::MarkEntry {
                        ch: '*',
                        row,
                        col: 0,
                        buffer_id: buf.id,
                        buffer_name: buf.display_name(),
                    });
                }
            }
        }

        // 3. Inject the last jump position as the `` mark
        if let Some((r, c)) = self.active_window().last_jump {
            let buf = self.buf();
            entries.push(crate::popup::MarkEntry {
                ch: '`',
                row: r,
                col: c,
                buffer_id: buf.id,
                buffer_name: buf.display_name(),
            });
        }

        // Sort entries by file, then by row so they appear in logical order
        entries.sort_by(|a, b| {
            a.buffer_id
                .cmp(&b.buffer_id)
                .then_with(|| a.row.cmp(&b.row))
        });

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
    /// Center the viewport so that the cursor row is in the middle of
    /// the visible area (Vim's `zz` behaviour).
    pub fn center_viewport_on_cursor(&mut self) {
        let gutter = self.active_gutter_width();
        let (win, buf) = self.active_window_and_buf_mut();
        let viewport_h = win.position.height;
        let viewport_w = win.position.width;
        let half = viewport_h / 2;

        // Ideal scroll_line puts the cursor row at the vertical midpoint
        let ideal = if win.row >= half { win.row - half } else { 0 };

        // Clamp: don't scroll past the end of the file
        let max_scroll = buf.len_lines().saturating_sub(viewport_h.saturating_sub(1));
        win.scroll_line = ideal.min(max_scroll);

        // Ensure cursor is still visible (should be by construction, but
        // keeps the internal invariants consistent)
        win.scroll_to_cursor(viewport_h, viewport_w, gutter);
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
        use crossterm::event::KeyCode;

        // ── Register Mode Intercept ────────────────────────────────────
        // If we are in the Ctrl-r register prompt, catch the next key here
        // before it goes through the normal key resolution.
        if self.pending_register {
            let action = match (self.mode(), key.code) {
                (Mode::Command | Mode::Search, KeyCode::Char('%')) => {
                    Some(Action::CommandInsertFilename)
                }
                (Mode::Command | Mode::Search, KeyCode::Char('w'))
                    if key.modifiers.contains(KeyModifiers::CONTROL) =>
                {
                    Some(Action::CommandInsertWord)
                }
                _ => Some(Action::CommandCancelRegister), // Any other key cancels register mode
            };

            if let Some(act) = action {
                crate::keybind::bindings::execute_action(self, act);
            }
            return; // Key fully handled, skip normal resolution
        }

        if key.kind != crossterm::event::KeyEventKind::Press {
            return;
        }
        if matches!(key.code, crossterm::event::KeyCode::Modifier(_)) {
            return;
        }
        /*
        // TEMP DIAGNOSTIC — remove after confirming
        log::debug!(
            "RAW key={:?} mod={:?} kind={:?}",
            key.code,
            key.modifiers,
            key.kind
        );
        */

        /*
        // [2026-05-27T02:35:44Z DEBUG ce::ed::editor] RAW key=Modifier(LeftAlt) mod=KeyModifiers(CONTROL | ALT) kind=Press
        // [2026-05-27T02:35:45Z DEBUG ce::ed::editor] RAW key=Char('p') mod=KeyModifiers(CONTROL | ALT) kind=Press
        // [2026-05-27T02:35:46Z DEBUG ce::ed::editor] RAW key=Down mod=KeyModifiers(0x0) kind=Press
        // [2026-05-27T02:35:46Z DEBUG ce::keybind::bindings] execute_action: MoveDown
        // [2026-05-27T02:35:46Z DEBUG ce::ed::editor] RAW key=Down mod=KeyModifiers(0x0) kind=Press
        // [2026-05-27T02:35:46Z DEBUG ce::keybind::bindings] execute_action: MoveDown
        // [2026-05-27T02:35:48Z DEBUG ce::ed::editor] RAW key=Modifier(LeftControl) mod=KeyModifiers(CONTROL) kind=Press
        // [2026-05-27T02:35:48Z DEBUG ce::ed::editor] RAW key=Modifier(LeftShift) mod=KeyModifiers(SHIFT | CONTROL) kind=Press
        // [2026-05-27T02:35:49Z DEBUG ce::ed::editor] RAW key=Enter mod=KeyModifiers(0x0) kind=Press
        // [2026-05-27T02:35:49Z DEBUG ce::ed::editor] RAW key=Enter mod=KeyModifiers(0x0) kind=Press
        // [2026-05-27T02:35:50Z DEBUG ce::ed::editor] RAW key=Esc mod=KeyModifiers(0x0) kind=Press
        // [2026-05-27T02:35:50Z DEBUG ce::ed::editor] RAW key=Modifier(LeftAlt) mod=KeyModifiers(ALT) kind=Press
        // shift swallow by terminal
        // the patch is working for Ctrl+SHIFT+p but lag behind for UI render
        // ── Global shortcuts: bypass sequence resolver entirely ──────────
        // These must fire instantly with zero pending-key accumulation.
         */
        /*
        if key.modifiers.contains(KeyModifiers::CONTROL)
            && key.modifiers.contains(KeyModifiers::SHIFT)
        {
            match key.code {
                KeyCode::Char('p') | KeyCode::Char('P') => {
                    self.clear_pending_keys();
                    crate::keybind::execute_action(self, Action::EnterCommandPalette);
                    return;
                }
                _ => {}
            }
        }
        */

        // ── Count prefix for Normal / Visual modes ──────────────────
        if matches!(
            self.mode,
            Mode::Normal | Mode::Visual | Mode::VisualLine | Mode::VisualBlock
        ) {
            if let KeyCode::Char(c) = key.code {
                if key.modifiers.is_empty() && c.is_ascii_digit() {
                    let digit = c.to_digit(10).unwrap() as usize;
                    if digit == 0 && self.current_count == 0 {
                        // Let '0' fall through to be handled as MoveLineStart
                    } else {
                        self.current_count = self.current_count * 10 + digit;
                        self.set_status_msg(&format!("{}", self.current_count), MessageKind::Info);
                        return; // Key consumed
                    }
                }
            }
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

        if self.popup.guide.is_some() {
            self.handle_guide_popup_key(key);
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

        if self.popup.command_palette.is_some() {
            self.handle_command_palette_key(key);
            return;
        }

        // WhichKey is a passive overlay, don't intercept keys for it!
        if self.popup.is_open() && self.popup.kind != Some(PopupKind::WhichKey) {
            self.handle_popup_key(key);
            return;
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
            let key_str = crate::keybind::binding_ex::format_key(key);
            let action = crate::keybind::bindings::resolve_single_key(
                &self.config,
                &key_str,
                self.mode,
                ghost_active,
                key,
            );
            log::debug!(
                "[key] mode={:?} key_str={:?} raw={:?} mod={:?} → {:?}",
                self.mode,
                key_str,
                key.code,
                key.modifiers,
                action
            );
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
                BufferKind::CheckHealth => self.handle_checkhealth_key(key),

                // ── LLM Buffers ──────────────────────────────────────────
                BufferKind::Llm => self.handle_llm_key(key),
                BufferKind::LlmInput => self.handle_llm_input_key(key),
                _ => false,
            };
            if handled {
                return;
            }
        }

        // 3. Try visual-specific commands first, but fall through to allow hjkl movements
        if self.mode == Mode::Visual
            || self.mode == Mode::VisualLine
            || self.mode == Mode::VisualBlock
        {
            if self.pending_keys.is_empty() {
                let key_str = crate::keybind::binding_ex::format_key(key);
                if let Some(action) = crate::keybind::bindings::resolve_single_key(
                    &self.config,
                    &key_str,
                    self.mode,
                    ghost_active,
                    key,
                ) {
                    crate::keybind::execute_action(self, action);
                    return;
                }
            }
        }

        let key_str = crate::keybind::binding_ex::format_key(key);
        if key_str.is_empty() {
            return;
        }

        if key_str == "esc" {
            let mut handled = false;
            if !self.pending_keys.is_empty() {
                self.clear_pending_keys();
                handled = true;
            }

            if self.current_count > 0 {
                self.current_count = 0; // Reset count on Esc
                handled = true;
            }

            let is_selecting = matches!(
                self.mode,
                Mode::Visual | Mode::VisualLine | Mode::VisualBlock
            ) || self.visual_block_insert_state.is_some()
                || self.windows.iter().any(|w| w.visual_anchor.is_some());

            if !self.status_msg.is_empty() {
                self.clear_status_msg();
                if !is_selecting {
                    handled = true;
                }
            }

            // AGGRESSIVE CLEANUP: If ESC is pressed and we have lingering anchors
            // clear them immediately to prevent ghost marks in the next round.
            if is_selecting && self.visual_block_insert_state.is_none() {
                for win in &mut self.windows {
                    win.visual_anchor = None;
                }
            }

            if handled {
                return;
            }
        }

        let new_seq = if self.pending_keys.is_empty() {
            key_str.clone()
        } else {
            format!("{} {}", self.pending_keys, key_str)
        };

        match crate::keybind::binding_ex::resolve_sequence(
            &self.config,
            &new_seq,
            ghost_active,
            self.mode,
        ) {
            crate::keybind::binding_ex::ResolveResult::Action(action)
            | crate::keybind::binding_ex::ResolveResult::AutoAction(action) => {
                self.clear_pending_keys();
                crate::keybind::execute_action(self, action);
            }
            crate::keybind::binding_ex::ResolveResult::Pending => {
                self.pending_keys = new_seq.clone();
            }
            crate::keybind::binding_ex::ResolveResult::None => {
                self.clear_pending_keys();

                // ── Fallback for Brief Mode single-key presses ────────────
                if self.mode == Mode::Brief {
                    let key_str = crate::keybind::binding_ex::format_key(key);
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

        // ── Word text object: use traditional boundary scanning (not Tree-sitter) ──
        if obj == TextObject::Word {
            let buf = self.buf();
            if row >= buf.len_lines() {
                self.set_status_msg("No text object found", MessageKind::Error);
                return;
            }

            let line_text = buf.line_text(row);
            let chars: Vec<char> = line_text.chars().collect();
            if chars.is_empty() {
                self.set_status_msg("No text object found", MessageKind::Error);
                return;
            }

            let c = col.min(chars.len().saturating_sub(1));
            let is_word_char = |ch: char| ch.is_alphanumeric() || ch == '_';

            let (start, end) = if is_word_char(chars[c]) {
                // On a word character — select the whole word
                let mut s = c;
                while s > 0 && is_word_char(chars[s - 1]) {
                    s -= 1;
                }
                let mut e = c + 1;
                while e < chars.len() && is_word_char(chars[e]) {
                    e += 1;
                }

                // dw behavior: include trailing whitespace
                if !inside {
                    while e < chars.len() && chars[e].is_whitespace() {
                        e += 1;
                    }
                }
                (s, e)
            } else if chars[c].is_whitespace() {
                // On whitespace — select contiguous whitespace
                let mut s = c;
                while s > 0 && chars[s - 1].is_whitespace() {
                    s -= 1;
                }
                let mut e = c + 1;
                while e < chars.len() && chars[e].is_whitespace() {
                    e += 1;
                }
                (s, e)
            } else {
                // On punctuation — select contiguous punctuation
                let ch = chars[c];
                let mut s = c;
                while s > 0 && chars[s - 1] == ch {
                    s -= 1;
                }
                let mut e = c + 1;
                while e < chars.len() && chars[e] == ch {
                    e += 1;
                }

                // dw behavior: include trailing whitespace
                if !inside {
                    while e < chars.len() && chars[e].is_whitespace() {
                        e += 1;
                    }
                }
                (s, e)
            };

            let (win, buf) = self.active_window_and_buf_mut();
            let line_start = buf.rope.line_to_char(row);
            let start_offset = line_start + start;
            let end_offset = line_start + end;

            if end_offset <= start_offset || end_offset > buf.rope.len_chars() {
                self.set_status_msg("No text object found", MessageKind::Error);
                return;
            }

            buf.rope.remove(start_offset..end_offset);
            win.row = row;
            win.col = start;
            win.col = win.col.min(buf.line_char_len(win.row));
            buf.mark_modified();

            if change {
                self.enter_insert();
            }
            return;
        }

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

            buf.rope.remove(start_offset..end_offset);

            win.row = sr;
            win.col = sc;
            // PANIC-FIX: clamp cursor after mutation
            win.col = win.col.min(buf.line_char_len(win.row));
            buf.mark_modified();

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

impl Editor {
    /// Safely changes the active editor mode and automatically manages
    /// the lifecycle of visual selection anchors on the viewports.
    pub fn change_mode(&mut self, new_mode: Mode) {
        let old_mode = self.mode;
        let is_visual = |m: Mode| matches!(m, Mode::Visual | Mode::VisualLine | Mode::VisualBlock);

        // 1. Clear anchors if leaving a visual mode, OR if we are in a non-visual mode
        // and somehow anchors leaked (and we aren't in a block insert).
        let should_clear = (is_visual(old_mode) && !is_visual(new_mode))
            || (!is_visual(new_mode)
                && self.visual_block_insert_state.is_none()
                && self.windows.iter().any(|w| w.visual_anchor.is_some()));

        if should_clear {
            for win in &mut self.windows {
                win.visual_anchor = None;
            }
        }

        // 2. If entering a visual mode from a non-visual mode, ALWAYS initialize anchor
        // to the current cursor. This overwrites any leaked anchors from previous rounds!
        if is_visual(new_mode) && !is_visual(old_mode) {
            let win = self.active_window_mut();
            // REMOVED: if win.visual_anchor.is_none()
            win.visual_anchor = Some((win.row, win.col));
        }

        // 3. Delegate to the underlying mode setters
        if new_mode == Mode::Normal {
            self.enter_normal();
        } else if new_mode == Mode::Brief {
            self.enter_brief();
        } else {
            self.set_mode(new_mode);
        }
    }

    pub fn finalize_visual_block_insert(&mut self, pre_captured_insert: Option<String>) {
        if let Some(state) = self.visual_block_insert_state.take() {
            // Clear anchors across ALL windows to prevent split-screen leaks
            for win in &mut self.windows {
                win.visual_anchor = None;
            }

            if let Some(typed_text) = pre_captured_insert {
                if !typed_text.is_empty() {
                    let cursor_row = self.active_window().row;

                    // REMOVED: self.buf_mut().push_undo(win_row, win_col);
                    // The undo snapshot is already taken when entering Insert mode.
                    // Removing this merges the block duplication into the insert session's undo block.

                    let buf = self.buf_mut();
                    for &r in &state.rows {
                        if r == cursor_row {
                            continue; // Already inserted on this line normally
                        }
                        if r >= buf.len_lines() {
                            continue;
                        }
                        let line_len = buf.line_char_len(r);
                        let col = state.col;
                        if col > line_len {
                            let pad = " ".repeat(col - line_len);
                            let off = buf.rope.line_to_char(r) + line_len;
                            buf.rope.insert(off, &pad);
                        }
                        let off = buf.rope.line_to_char(r) + col;
                        buf.rope.insert(off, &typed_text);
                    }
                    buf.mark_modified();
                    buf.parse_syntax();
                }
            }
            self.insert_buffer = None;
        }
    }
}

impl Editor {
    /// Handles rapid terminal bracketed paste events (`Event::Paste`).
    /// Inserts the text in a single transaction while preventing the cursor
    /// and horizontal scroll from jumping to extreme columns (e.g. 25,000).
    /// Handles rapid terminal bracketed paste events (`Event::Paste`).
    /// Inserts the text in a single transaction while preventing the cursor
    /// and horizontal scroll from jumping to extreme columns (e.g. 25,000).
    pub fn handle_paste(&mut self, text: &str) {
        if text.is_empty() {
            return;
        }

        // 1. Handle Command / Search modes (paste into the prompt, not the buffer)
        if matches!(self.mode, Mode::Command | Mode::Search) {
            for ch in text.chars() {
                self.push_command(ch);
            }
            self.comp.on_edit();
            return;
        }

        // 2. General buffer paste
        let bid = self.buf().id;
        let (start_row, start_col) = {
            let win = self.active_window();
            (win.row, win.col)
        };

        {
            let (win, buf) = self.active_window_and_buf_mut();

            let is_line_paste = text.ends_with('\n') || text.ends_with('\r');
            if is_line_paste {
                crate::ed::editing::paste_line_below(win, buf, text);
            } else {
                crate::ed::editing::paste_text(win, buf, text);
            }

            // ── Accurately calculate the end position of the pasted content ──
            let mut final_row = start_row;
            let mut final_col = start_col;

            if is_line_paste {
                let newlines = text.matches('\n').count();
                final_row = (start_row + newlines).min(buf.len_lines().saturating_sub(1));
                final_col = 0;
            } else {
                let mut current_col = start_col;
                for c in text.chars() {
                    if c == '\n' {
                        final_row += 1;
                        current_col = 0;
                    } else if c != '\r' {
                        current_col += 1;
                    }
                }
                final_col = current_col;
            }

            // Clamp to valid buffer positions to avoid out-of-bounds panics
            final_row = final_row.min(buf.len_lines().saturating_sub(1));
            if final_row < buf.len_lines() {
                final_col = final_col.min(buf.line_char_len(final_row));
            } else {
                final_col = 0;
            }

            // ── Forcefully sync the window cursor AND desired_col ──
            // Setting `desired_col` is crucial to stop the 25,000 column jump.
            win.row = final_row;
            win.col = final_col;
            win.desired_col = final_col;

            buf.parse_syntax();
        }

        // Dismiss ghost/completion overlays, snap viewport, and debounce git gutter
        self.comp.on_edit();
        self.snap_cursor_to_viewport();
        self.git_debounce.notify_edit(bid);
        self.refresh_buffer_words();
    }

    /// Alias for bracketed paste event loop compatibility.
    pub fn insert_str(&mut self, text: &str) {
        self.handle_paste(text);
    }
}

impl Editor {
    /// Returns span information for the innermost function-like node that
    /// contains the cursor, or `None` if tree-sitter is unavailable or no
    /// function node is found.
    pub fn function_around_span_info(&self) -> Option<FunctionSpanInfo> {
        let (row, col) = {
            let win = self.active_window();
            (win.row, win.col)
        };

        let buf = self.buf();

        // Convert (row, col) → byte offset for tree-sitter
        let char_off = buf.rope.line_to_char(row).saturating_add(col);
        let text = buf.rope.slice(..).to_string();
        let byte_off = text.char_indices().nth(char_off).map(|(b, _)| b)?;

        // ── FIX: Tree is inside `buf.syntax` ──
        // If `tree` is not the correct field name, run:
        //   grep -n 'pub struct SyntaxState' src/ed/syntax.rs src/ed/buffer.rs -A 10
        // and replace `.tree` with whatever the `Option<Tree>` field is named.
        let tree = buf.syntax.tree.as_ref()?;
        let root = tree.root_node();

        // Find the deepest node covering the cursor byte
        let cursor_node = root.descendant_for_byte_range(byte_off, byte_off + 1)?;

        // Walk upward until we hit a function-like node (innermost first).
        let func_node = {
            let mut node = cursor_node;
            loop {
                if is_fn_kind(node.kind()) {
                    break;
                }
                node = match node.parent() {
                    Some(p) => p,
                    None => return None,
                };
            }
            node
        };

        let start_row = func_node.start_position().row;
        let end_row = func_node.end_position().row;
        let line_count = end_row.saturating_sub(start_row);

        // Count nested function-like children (excluding func_node itself).
        let nested_fn_count = count_nested_fns(func_node);

        Some(FunctionSpanInfo {
            start_row,
            end_row,
            line_count,
            nested_fn_count,
        })
    }
    pub fn handle_command_palette_key(&mut self, key: KeyEvent) {
        if key.kind != crossterm::event::KeyEventKind::Press {
            return;
        }

        // Ctrl combinations
        if key.modifiers.contains(KeyModifiers::CONTROL) {
            match key.code {
                KeyCode::Char('n') | KeyCode::Char('N') => {
                    if let Some(popup) = &mut self.popup.command_palette {
                        popup.move_down();
                    }
                    return;
                }
                KeyCode::Char('p') | KeyCode::Char('P') => {
                    if let Some(popup) = &mut self.popup.command_palette {
                        popup.move_up();
                    }
                    return;
                }
                KeyCode::Char('u') | KeyCode::Char('U') => {
                    if let Some(popup) = &mut self.popup.command_palette {
                        popup.filter_clear();
                    }
                    return;
                }
                _ => return,
            }
        }

        match key.code {
            KeyCode::Esc => {
                self.popup.close();
            }
            KeyCode::Enter => {
                let action = self
                    .popup
                    .command_palette
                    .as_ref()
                    .and_then(|p| p.selected_entry())
                    .map(|e| e.action);
                if let Some(action) = action {
                    self.popup.close();
                    crate::keybind::bindings::execute_action(self, action);
                }
            }
            KeyCode::Up => {
                if let Some(popup) = &mut self.popup.command_palette {
                    popup.move_up();
                }
            }
            KeyCode::Down => {
                if let Some(popup) = &mut self.popup.command_palette {
                    popup.move_down();
                }
            }
            KeyCode::Backspace => {
                if let Some(popup) = &mut self.popup.command_palette {
                    popup.filter_pop();
                }
            }
            KeyCode::Char(c) => {
                if let Some(popup) = &mut self.popup.command_palette {
                    popup.filter_push(c);
                }
            }
            _ => {}
        }
    }

    /// Helper to inject text at the current command cursor position
    pub fn insert_command_text(&mut self, text: &str) {
        self.command.insert_str(self.command_cursor, text);
        self.command_cursor += text.len();
    }

    /// Returns the configured scroll offset, capped to half the
    /// active viewport height so it can never trap the cursor.
    pub fn effective_scroll_offset(&self) -> usize {
        let offset = self.config.scroll_offset;
        if offset == 0 {
            return 0;
        }
        let half_viewport = self.active_window().position.height / 2;
        offset.min(half_viewport)
    }
}

/// Recurse into `node`'s *direct* children, counting function-like nodes.
/// Does NOT recurse into those children's bodies (so a doubly-nested fn
/// inside a nested fn is NOT counted as a separate top-level nested fn
/// — it is part of the first nested fn).
fn count_nested_fns(node: tree_sitter::Node) -> usize {
    let mut count: usize = 0;
    let mut stack: Vec<tree_sitter::Node> = Vec::new();

    // ── FIX: Explicit `usize` annotation prevents E0282 type inference error ──
    let child_count: usize = node.child_count();
    for i in 0..child_count {
        if let Some(child) = node.child(i) {
            stack.push(child);
        }
    }

    while let Some(n) = stack.pop() {
        if is_fn_kind(n.kind()) {
            count += 1;
            // Do NOT recurse into nested functions — their own inner fns
            // belong to them, not to the outer function we're measuring.
        } else {
            let cc: usize = n.child_count();
            for i in 0..cc {
                if let Some(child) = n.child(i) {
                    stack.push(child);
                }
            }
        }
    }

    count
}

fn is_fn_kind(kind: &str) -> bool {
    matches!(
        kind,
        "function_item"
            | "function_definition"
            | "method_definition"
            | "arrow_function"
            | "function_declaration"
            | "method_declaration"
    )
}
