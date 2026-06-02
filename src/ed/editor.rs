//! Central editor state and key dispatch.
use crate::comp::state::CompletionMachine;
use crate::comp::state::CompletionSource;
use crate::config::app_config::Config;
use crate::ed::buffer::{Buffer, BufferKind};
use crate::ed::handle::tag::LSP_GOTO_TIMEOUT_MS;
use crate::ed::misc_helper::{count_nested_fns, is_fn_kind, is_valid_register_char};
use crate::ed::mode::{MessageKind, Mode};
use crate::ed::syntax::TextObject;
use crate::ed::window::{LayoutNode, Window};
use crate::keybind::bindings::Action;
use crate::keybind::bindings::FunctionSpanInfo;
use crate::lsp::TextEdit;
use crate::lsp::{
    path_to_uri, uri_to_path, CompletionItem, FormattingOptions, InlayHint, Location, LspManager,
    LspMessage, OffsetEncoding, SignatureHelpState,
};
use crate::msgbox::AppMessage;
use crate::msgbox::AppMessage as LspAppMessage;
use crate::popup::{PopupItem, PopupKind, PopupState};
use crate::render::statusbar_state::StatusBarState;
use anyhow::Result;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use std::collections::HashSet;

/// In-flight LSP go-to-definition request.
#[derive(Debug)]
pub struct PendingLspGoto {
    pub symbol: String,
    pub pushed_tag_stack: bool,
    pub created_at: std::time::Instant,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EasyMotionPhase {
    Collecting,
    Selecting,
}

#[derive(Debug, Clone)]
pub struct EasyMotionTarget {
    pub row: usize,
    pub col: usize, // char-offset within the line (matches win.col semantics)
    pub label: String,
}

#[derive(Debug, Clone)]
pub struct EasyMotionState {
    pub prefix: String,
    pub phase: EasyMotionPhase,
    pub targets: Vec<EasyMotionTarget>,
    pub partial_label: String,
    pub label_len: usize, // 1 if ≤26 targets, 2 if ≤676
}

#[derive(Debug, Clone, Default)]
pub struct RegisterBank {
    pub named: std::collections::HashMap<char, String>,
}

#[derive(Debug, Clone)]
pub struct VisualBlockInsertState {
    pub rows: Vec<usize>,
    pub col: usize,
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QuitPrompt {
    None,
    BufferQuit,
    QuitAllConfirm,
}

pub struct Editor {
    pub config: Config,
    pub buffers: Vec<Buffer>,
    pub windows: Vec<Window>,
    pub layout: LayoutNode,
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
    pub pending_keys_time: Option<std::time::Instant>, // tag_whichkey_pending.d
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
    pub search_history: Vec<String>,
    pub search_history_idx: Option<usize>,
    pub search_temp_input: String,
    pub search_history_prefix: Option<String>,
    pub status_state: StatusBarState,
    pub prev_mode: Mode,
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
    pub tag_manager: crate::ed::tag::TagManager,
    pub substitution_state: Option<crate::ed::handle::subst::SubstitutionState>,
    pub prev_search_query: Option<String>,
    pub saved_visual_range: Option<(usize, usize)>,
    pub hunk_cache: Option<crate::ed::implex::HunkCache>,
    pub window_nav_pending: bool,
    pub close_window_nav_pending: bool,
    pub config_cycle_keys: Vec<(String, Vec<String>)>,
    pub registers: RegisterBank,
    pub normal_register_prefix: Option<char>, // Tracks '"' prefix in Normal/Visual mode
    /// Last yank text (register 0 — only set by yank, not delete).
    pub yank_register_0: Option<String>,
    /// Last small delete/change text (register -).
    pub small_delete_register: Option<String>,
    pub ctagd: crate::lsp::RustLspClient,

    /// Channel to send requests INTO the LSP task.
    pub lsp_tx: Option<tokio::sync::mpsc::UnboundedSender<LspMessage>>,
    /// Channel to receive responses FROM the LSP task.
    pub lsp_rx: Option<std::sync::mpsc::Receiver<LspAppMessage>>,
    /// Handle to the background tokio runtime (keeps it alive).
    _lsp_runtime: Option<std::sync::Arc<tokio::runtime::Runtime>>,
    /// Cached LSP offset encoding (set after initialization).
    pub lsp_offset_encoding: OffsetEncoding,
    /// Current inlay hints keyed by URI.
    pub inlay_hints: std::collections::HashMap<String, Vec<InlayHint>>,
    /// Current signature help state.
    pub signature_help: Option<SignatureHelpState>,
    /// Per-URI open file version tracking (for incremental sync).
    pub lsp_file_versions: std::collections::HashMap<String, i32>,
    /// Whether the full LSP (not just ctagd) is active.
    pub lsp_full_active: bool,
    pub easymotion: Option<EasyMotionState>,
    pub pending_lsp_goto: Option<PendingLspGoto>,

    //-- struct Editor (anchor dont removed) --//
    pub quit_prompt: QuitPrompt,
}

const MAX_WINDOWS: usize = 8;

// ═══════════════════════════════════════════════════════════════════════════
// Construction
// ═══════════════════════════════════════════════════════════════════════════

impl Editor {
    pub fn new(filename: Option<String>) -> Result<Self> {
        let first_buf = Buffer::new(0, filename.clone())?;
        let config = Config::load().unwrap_or_default();
        let cmd_history = Self::load_history();
        let vocab_words = Self::preload_vocabulary();
        let positions = Self::load_positions();

        let mut first_win = Window::new(0, first_buf.id);

        if let Some(ref name) = filename {
            let path_buf = std::path::PathBuf::from(name);
            let canon_path = std::fs::canonicalize(&path_buf)
                .unwrap_or(path_buf)
                .to_string_lossy()
                .to_string();
            if let Some(pos) = positions.iter().find(|p| p.path == canon_path) {
                first_win.row = pos.row.min(first_buf.len_lines().saturating_sub(1));
                first_win.col = pos.col.min(crate::ed::editing::line_display_width(
                    &first_buf,
                    first_win.row,
                ));
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
            pending_keys_time: None,
            popup: PopupState::new(),
            config_bool_keys: Vec::new(),
            config_cycle_keys: Vec::new(),
            vocab_words,
            buffer_words: Vec::new(),
            clipboard: None,
            clipboard_is_block: false,
            visual_block_insert_state: None,
            cmd_history,
            cmd_history_idx: None,
            cmd_temp_input: String::new(),
            history_search_prefix: None,
            search_history: Self::load_search_history(),
            search_history_idx: None,
            search_temp_input: String::new(),
            search_history_prefix: None,
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
            tag_manager: crate::ed::tag::TagManager::new(),
            substitution_state: None,
            prev_search_query: None,
            saved_visual_range: None,
            hunk_cache: None,
            window_nav_pending: false,
            close_window_nav_pending: false,
            registers: RegisterBank::default(),
            normal_register_prefix: None,
            yank_register_0: None,
            small_delete_register: None,
            ctagd: crate::lsp::RustLspClient::new(),

            lsp_tx: None,
            lsp_rx: None,
            _lsp_runtime: None,
            lsp_offset_encoding: OffsetEncoding::default(),
            inlay_hints: std::collections::HashMap::new(),
            signature_help: None,
            lsp_file_versions: std::collections::HashMap::new(),
            lsp_full_active: false,
            easymotion: None,
            pending_lsp_goto: None,

            //-- Editor fn new() (anchor dont removed) --//
            last_action: crate::ed::repeat::LastAction::default(),
            repeat_pending: false,
            current_count: 0,
            insert_buffer: None,
            quit_prompt: QuitPrompt::None,
        };

        // ── Spawn LSP background task ────────────────────────────
        if editor.config.lsp_enabled {
            editor.spawn_lsp_task();

            // ── Open initial file with LSP ───────────────────────
            if let Some(ref name) = filename {
                editor.lsp_notify_open(std::path::PathBuf::from(name));
            }
        }

        if let Some(name) = editor.active_filename() {
            let path_buf = std::path::PathBuf::from(name);
            let canon_path = std::fs::canonicalize(&path_buf).unwrap_or(path_buf);
            let win = editor.active_window();
            editor.mru_manager.insert(canon_path, win.row, win.col);
        }

        if editor.config.init_mode == "brief" {
            editor.enter_brief();
        }

        // tag_vocab
        if !editor.config.vocab_wordlist {
            editor.vocab_words = HashSet::new();
        }

        if editor.config.show_startup_hints {
            let init_msg = if editor.config.init_mode == "brief" {
                "Brief mode | F9 for :commands, :vim to vim mode"
            } else {
                "Type i to insert mode, gi to brief mode, :q to quit, :e <path> to open another file"
            };
            editor.set_status(init_msg, MessageKind::Info);
        }

        if editor.config.buffer_word_scan {
            editor.maybe_refresh_buffer_words();
        }

        if let Some(ref name) = filename {
            let bid = editor.active_window().buffer_id();
            let rope = editor.buf().rope.clone();
            log::debug!("Editor::new: Queueing initial startup diff for {}", name);
            editor.async_gutter.request_diff(bid, &rope, Some(name));
        }

        editor.maybe_refresh_buffer_words();
        Ok(editor)
    }

    /// Spawn the LspManager in a background tokio runtime.
    fn spawn_lsp_task(&mut self) {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("Failed to create LSP tokio runtime");

        let (async_tx, mut async_rx) = tokio::sync::mpsc::unbounded_channel::<LspAppMessage>();
        let (sync_tx, sync_rx): (
            std::sync::mpsc::Sender<LspAppMessage>,
            std::sync::mpsc::Receiver<LspAppMessage>,
        ) = std::sync::mpsc::channel();

        runtime.spawn(async move {
            while let Some(msg) = async_rx.recv().await {
                if sync_tx.send(msg).is_err() {
                    break;
                }
            }
        });

        let mut lsp_manager = LspManager::new(async_tx);
        let lsp_tx = lsp_manager.get_sender();

        std::thread::spawn(move || {
            runtime.block_on(async {
                lsp_manager.run().await;
            });
        });

        self.lsp_tx = Some(lsp_tx);
        self.lsp_rx = Some(sync_rx);
        self._lsp_runtime = None;
        self.lsp_full_active = true;
    }

    /// Current scope (impl::function) at the active cursor position.
    pub fn current_scope(&self) -> Option<String> {
        let win = self.active_window();
        let buf = self.buf();
        buf.current_scope(win.row, win.col)
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// Accessors — active window / buffer lookups
// ═══════════════════════════════════════════════════════════════════════════

impl Editor {
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

    /// Find a buffer by its ID. Returns `None` only for a state bug
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
            let first_id = self.buffers.first().map(|b| b.id).unwrap_or(0);
            self.windows[self.active_window_idx].set_buffer_id(first_id);
        }
        let target_id = self.active_window().buffer_id();
        self.buffers
            .iter_mut()
            .find(|b| b.id == target_id)
            .expect("buffer must exist after repair")
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

// ═══════════════════════════════════════════════════════════════════════════
// Mode Management
// ═══════════════════════════════════════════════════════════════════════════

impl Editor {
    /// Safely changes the active editor mode and automatically manages
    /// the lifecycle of visual selection anchors on the viewports.
    pub fn change_mode(&mut self, new_mode: Mode) {
        let old_mode = self.mode;
        let is_visual = |m: Mode| matches!(m, Mode::Visual | Mode::VisualLine | Mode::VisualBlock);

        // 1. Clear anchors if leaving a visual mode, OR if we are in a non-visual mode
        //    and somehow anchors leaked (and we aren't in a block insert).
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
        //    to the current cursor. This overwrites any leaked anchors from previous rounds!
        if is_visual(new_mode) && !is_visual(old_mode) {
            let win = self.active_window_mut();
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
}

// ═══════════════════════════════════════════════════════════════════════════
// Viewport
// ═══════════════════════════════════════════════════════════════════════════

impl Editor {
    /// Center the viewport so that the cursor row is in the middle of
    /// the visible area (Vim's `zz` behaviour).
    pub fn center_viewport_on_cursor(&mut self) {
        let gutter = self.active_gutter_width();
        let (win, buf) = self.active_window_and_buf_mut();
        let viewport_h = win.position.height;
        let viewport_w = win.position.width;
        let half = viewport_h / 2;

        let ideal = if win.row >= half { win.row - half } else { 0 };

        let max_scroll = buf.len_lines().saturating_sub(viewport_h.saturating_sub(1));
        win.scroll_line = ideal.min(max_scroll);

        win.scroll_to_cursor(viewport_h, viewport_w, gutter);
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

// ═══════════════════════════════════════════════════════════════════════════
// Visual Selection
// ═══════════════════════════════════════════════════════════════════════════

impl Editor {
    /// Returns the `(start_row, end_row)` of the active visual selection.
    /// Returns `None` if not in a visual mode or no anchor is set.
    pub fn get_visual_line_range(&self) -> Option<(usize, usize)> {
        let win = self.active_window();
        let anchor = win.visual_anchor?;
        let r1 = anchor.0.min(win.row);
        let r2 = anchor.0.max(win.row);
        Some((r1, r2))
    }

    pub fn finalize_visual_block_insert(&mut self, pre_captured_insert: Option<String>) {
        if let Some(state) = self.visual_block_insert_state.take() {
            for win in &mut self.windows {
                win.visual_anchor = None;
            }

            if let Some(typed_text) = pre_captured_insert {
                if !typed_text.is_empty() {
                    let cursor_row = self.active_window().row;

                    let buf = self.buf_mut();
                    for &r in &state.rows {
                        if r == cursor_row {
                            continue;
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

// ═══════════════════════════════════════════════════════════════════════════
// Pending State
// ═══════════════════════════════════════════════════════════════════════════

impl Editor {
    pub fn clear_pending_keys(&mut self) {
        self.pending_keys.clear();
        self.pending_keys_time = None;
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// Quit & Exit
// ═══════════════════════════════════════════════════════════════════════════

impl Editor {
    pub fn force_quit(&mut self) {
        self.save_all_window_positions();
        self.should_quit = true;
    }

    pub fn quit_all_check(&mut self) {
        let first_dirty_bid = self.buffers.iter().find(|b| b.modified).map(|b| b.id);

        if let Some(bid) = first_dirty_bid {
            self.switch_window_to_buffer(bid);

            self.quit_prompt = QuitPrompt::QuitAllConfirm;
            let name = self.buf().display_name();
            self.set_status_msg(
                &format!("Save changes to {}? (y/n/c)", name),
                MessageKind::Error,
            );
        } else {
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

// ═══════════════════════════════════════════════════════════════════════════
// Key Dispatch
// ═══════════════════════════════════════════════════════════════════════════

impl Editor {
    pub fn handle_key(&mut self, key: KeyEvent) {
        use crossterm::event::KeyCode;

        // ── Register Mode Intercept ────────────────────────────────
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
                _ => Some(Action::CommandCancelRegister),
            };

            if let Some(act) = action {
                crate::keybind::bindings::execute_action(self, act);
            }
            return;
        }

        // ── EasyMotion intercept ─────────────────────────────────────
        if self.easymotion.is_some() {
            self.handle_easymotion_key(key);
            return;
        }

        //-- handle_key (anchor dont removed) --//
        // ── Window Navigation Mode Intercept ───────────────────────
        if self.window_nav_pending {
            let action = match key.code {
                KeyCode::Char('h') | KeyCode::Left => Some(Action::FocusWindowLeft),
                KeyCode::Char('j') | KeyCode::Down => Some(Action::FocusWindowDown),
                KeyCode::Char('k') | KeyCode::Up => Some(Action::FocusWindowUp),
                KeyCode::Char('l') | KeyCode::Right => Some(Action::FocusWindowRight),
                KeyCode::Char('w') => Some(Action::FocusNextWindow),
                KeyCode::Char('q') => Some(Action::CloseWindow),
                KeyCode::Char('o') => Some(Action::OnlyWindow),
                KeyCode::Char('s') => Some(Action::SplitHorizontal),
                KeyCode::Char('v') => Some(Action::SplitVertical),
                KeyCode::Esc => None,
                _ => None,
            };

            self.window_nav_pending = false;
            self.clear_status_msg();

            if let Some(act) = action {
                crate::keybind::bindings::execute_action(self, act);
            }
            return;
        }

        // ── Close Window Navigation Mode Intercept ─────────────────
        if self.close_window_nav_pending {
            let action = match key.code {
                KeyCode::Char('h') | KeyCode::Left => Some(Action::CloseWindowLeft),
                KeyCode::Char('j') | KeyCode::Down => Some(Action::CloseWindowDown),
                KeyCode::Char('k') | KeyCode::Up => Some(Action::CloseWindowUp),
                KeyCode::Char('l') | KeyCode::Right => Some(Action::CloseWindowRight),
                KeyCode::Char('d') | KeyCode::Char('q') => Some(Action::CloseWindow),
                KeyCode::Esc => None,
                _ => None,
            };

            self.close_window_nav_pending = false;
            self.clear_status_msg();

            if let Some(act) = action {
                crate::keybind::bindings::execute_action(self, act);
            }
            return;
        }

        if key.kind != crossterm::event::KeyEventKind::Press {
            return;
        }
        if matches!(key.code, crossterm::event::KeyCode::Modifier(_)) {
            return;
        }

        // ── Register prefix for Normal / Visual modes ──────────────
        if matches!(
            self.mode,
            Mode::Normal | Mode::Visual | Mode::VisualLine | Mode::VisualBlock
        ) {
            match self.normal_register_prefix {
                None => {
                    // Check for the initial `"` key press
                    if let KeyCode::Char('"') = key.code {
                        if key.modifiers.is_empty() {
                            self.normal_register_prefix = Some('"');
                            self.open_registers_popup(); // Show the informational popup
                            return;
                        }
                    }
                }
                Some('"') => {
                    // Fallback: If the popup was empty and didn't open,
                    // we still intercept the register character here.
                    match key.code {
                        KeyCode::Char(c) if is_valid_register_char(c) => {
                            self.normal_register_prefix = Some(c);
                            self.popup.close(); // Clear both data and kind
                            self.clear_status_msg();
                            return;
                        }
                        KeyCode::Esc => {
                            self.normal_register_prefix = None;
                            self.popup.close();
                            self.clear_status_msg();
                            return;
                        }
                        _ => {
                            self.normal_register_prefix = None;
                            self.popup.close();
                            self.clear_status_msg();
                            return;
                        }
                    }
                }
                Some(_) => {
                    // Register name is already set (e.g. `+`, `a`),
                    // fall through to normal key processing so the command (y/d/c) can execute
                }
            }
        }

        // ── Count prefix for Normal / Visual modes ─────────────────
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
                        return;
                    }
                }
            }
        }

        // ── Intercept for Pending Git Actions ──────────────────────
        if self.pending_git_action != PendingGitAction::None {
            self.handle_git_action_prompt_key(key);
            return;
        }

        // ── Intercept for Error Popup ──────────────────────────────
        if self.popup.error.is_some() {
            if key.code == KeyCode::Esc || key.code == KeyCode::Enter {
                self.popup.error = None;
                self.popup.kind = None;
            }
            return;
        }

        if self.popup.tag_candidates.is_some() {
            self.handle_tag_candidates_key(key);
            return;
        }

        if self.popup.workspace_symbols.is_some() {
            self.handle_workspace_symbols_key(key);
            return;
        }

        if self.substitution_state.is_some() {
            self.handle_substitution_key(key);
            return;
        }

        if self.popup.file_picker.is_some() {
            self.handle_file_picker_key(key);
            return;
        }

        if self.popup.buffer_list.is_some() {
            self.handle_buffer_list_key(key);
            return;
        }

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

        // tag_fd_handle_key
        if self.popup.fd.is_some() {
            self.handle_fd_key(key);
            return;
        }

        if self.popup.marks.is_some() {
            self.handle_marks_key(key);
            return;
        }

        if self.popup.quickfix.is_some() {
            self.handle_quickfix_key(key);
            return;
        }

        if self.popup.registers.is_some() {
            self.handle_registers_key(key);
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
        if self.popup.is_open() && self.popup.kind != Some(PopupKind::Whichkey) {
            self.handle_popup_key(key);
            return;
        }

        // ── Pending bookmark / quickmark input ─────────────────────
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

        // ── Normal / Insert / Command / Search / Visual handling ───
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

        // 2. Special-buffer key overrides (GitLog / GitDiff / …)
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
                BufferKind::Llm | BufferKind::LlmInput => self.handle_llm_buffer_key(key),
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

            if self.normal_register_prefix.is_some() {
                self.normal_register_prefix = None;
                handled = true;
            }

            if !self.pending_keys.is_empty() {
                self.clear_pending_keys();
                handled = true;
            }

            if self.current_count > 0 {
                self.current_count = 0;
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
                if self.pending_keys_time.is_none() {
                    self.pending_keys_time = Some(std::time::Instant::now());
                }
            }
            crate::keybind::binding_ex::ResolveResult::None => {
                self.clear_pending_keys();

                // Fallback for Brief Mode single-key presses
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
        self.sync_diff_windows();
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// Popup Handlers
// ═══════════════════════════════════════════════════════════════════════════

impl Editor {
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
                        label: format!("{:<25} : [{}]", human_key, status),
                        detail: Some(key.clone()),
                        data: bool_keys.len(),
                        active: val,
                    });
                    bool_keys.push(key);
                }
            }
        }

        self.config_bool_keys = bool_keys;

        // ── Cycle / selectable items ──────────────────────────────
        let base_offset = self.config_bool_keys.len();
        let cycle_defs: Vec<(&str, Vec<&str>)> = vec![
            ("init_mode", vec!["vim", "brief"]),
            ("llm_backend", vec!["llamacpp", "ollama"]),
            ("commit_backend", vec!["llamacpp", "ollama"]),
        ];

        self.config_cycle_keys = cycle_defs
            .iter()
            .map(|(key, opts)| {
                (
                    (*key).to_string(),
                    opts.iter().map(|s| (*s).to_string()).collect(),
                )
            })
            .collect();

        for (i, (key, opts)) in cycle_defs.iter().enumerate() {
            let current =
                if let Ok(serde_json::Value::Object(map)) = serde_json::to_value(&self.config) {
                    map.get(*key)
                        .and_then(|v| v.as_str())
                        .unwrap_or(opts[0])
                        .to_string()
                } else {
                    opts[0].to_string()
                };

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

            let hint = format!("({})", opts.join("/"));

            items.push(PopupItem {
                label: format!("{:<25} : [{:<8}]", human_key, current),
                detail: Some(hint),
                data: base_offset + i,
                active: true,
            });
        }

        self.popup.open_config(items, 0);
    }

    pub fn handle_command_palette_key(&mut self, key: KeyEvent) {
        if key.kind != crossterm::event::KeyEventKind::Press {
            return;
        }

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

    pub fn trigger_buffer_list_popup(&mut self) {
        let active_bid = self.active_window().buffer_id();

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

        let positions = &self.positions;
        entries.sort_by_key(|entry| {
            if entry.id == active_bid {
                return std::cmp::Reverse(usize::MAX);
            }

            if let Some(ref path_buf) = entry.path {
                let canon_path = std::fs::canonicalize(path_buf)
                    .unwrap_or_else(|_| path_buf.clone())
                    .to_string_lossy()
                    .to_string();

                if let Some(idx) = positions.iter().position(|p| p.path == canon_path) {
                    return std::cmp::Reverse(idx);
                }
            }

            std::cmp::Reverse(0)
        });

        self.popup.open_buffer_list(entries);
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// Text Editing & Objects
// ═══════════════════════════════════════════════════════════════════════════

impl Editor {
    pub fn edit_text_object(&mut self, obj: TextObject, inside: bool, change: bool) {
        let (row, col) = {
            let win = self.active_window();
            (win.row, win.col)
        };

        // ── Word text object: use traditional boundary scanning ─────
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
                let mut s = c;
                while s > 0 && is_word_char(chars[s - 1]) {
                    s -= 1;
                }
                let mut e = c + 1;
                while e < chars.len() && is_word_char(chars[e]) {
                    e += 1;
                }

                if !inside {
                    while e < chars.len() && chars[e].is_whitespace() {
                        e += 1;
                    }
                }
                (s, e)
            } else if chars[c].is_whitespace() {
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
                let ch = chars[c];
                let mut s = c;
                while s > 0 && chars[s - 1] == ch {
                    s -= 1;
                }
                let mut e = c + 1;
                while e < chars.len() && chars[e] == ch {
                    e += 1;
                }

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

            if sr >= buf.len_lines() || er >= buf.len_lines() {
                self.set_status_msg("Invalid text object range", MessageKind::Error);
                return;
            }

            let start_offset = buf.rope.line_to_char(sr).saturating_add(sc);
            let end_offset = buf.rope.line_to_char(er).saturating_add(ec);

            if end_offset <= start_offset || end_offset > buf.rope.len_chars() {
                self.set_status_msg("Invalid text object range", MessageKind::Error);
                return;
            }

            buf.rope.remove(start_offset..end_offset);

            win.row = sr;
            win.col = sc;
            win.col = win.col.min(buf.line_char_len(win.row));
            buf.mark_modified();

            if change {
                self.enter_insert();
            }
        } else {
            self.set_status_msg("No text object found", MessageKind::Error);
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// Paste Handling
// ═══════════════════════════════════════════════════════════════════════════

impl Editor {
    /// Handles rapid terminal bracketed paste events (`Event::Paste`).
    /// Inserts the text in a single transaction while preventing the cursor
    /// and horizontal scroll from jumping to extreme columns.
    pub fn handle_paste(&mut self, text: &str) {
        if text.is_empty() {
            return;
        }

        if matches!(self.mode, Mode::Command | Mode::Search) {
            for ch in text.chars() {
                self.push_command(ch);
            }
            self.comp.on_edit();
            return;
        }

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

            final_row = final_row.min(buf.len_lines().saturating_sub(1));
            if final_row < buf.len_lines() {
                final_col = final_col.min(crate::ed::editing::line_display_width(buf, final_row));
            } else {
                final_col = 0;
            }

            // ── FIX: enforce trailing-newline invariant ───────────
            let len = buf.rope.len_chars();
            if len == 0 || buf.rope.char(len - 1) != '\n' {
                buf.rope.insert(len, "\n");
                buf.mark_modified();
            }

            win.row = final_row;
            win.col = final_col;
            win.desired_col = final_col;

            buf.parse_syntax();
        }

        self.comp.on_edit();
        self.snap_cursor_to_viewport();
        self.git_debounce.notify_edit(bid);
        self.maybe_refresh_buffer_words();
    }

    /// Alias for bracketed paste event loop compatibility.
    pub fn insert_str(&mut self, text: &str) {
        self.handle_paste(text);
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// Completion
// ═══════════════════════════════════════════════════════════════════════════

impl Editor {
    /// Called by the main loop on every edit while in Insert/Brief mode.
    /// Called by the main loop on every edit while in Insert/Brief mode.
    pub fn on_completion_edit(&mut self) {
        self.comp.on_edit();

        let prefix = self.get_current_word_prefix();

        let is_path_prefix = prefix.starts_with("./");

        if is_path_prefix && prefix.len() < 2 {
            return;
        }
        if !is_path_prefix && prefix.len() < 4 {
            return;
        }

        self.comp.set_prefix(prefix.clone());

        let (row, col) = {
            let win = self.active_window();
            (win.row, win.col)
        };

        // ── Source 1: FilePaths (Inline Ghost Text) ────────────────
        if is_path_prefix {
            let (_req_id, version) = self.comp.start_source_request(CompletionSource::FilePaths);
            let matches = crate::comp::path_complete::complete_path(&prefix);
            self.comp
                .merge_source(CompletionSource::FilePaths, matches, version);
        } else {
            let (_req_id, version) = self.comp.start_source_request(CompletionSource::FilePaths);
            self.comp
                .merge_source(CompletionSource::FilePaths, Vec::new(), version);
        }

        // ── Source 2: Buffer words (fast, sync) ────────────────────
        if self.config.buffer_word_scan && !self.buffer_words.is_empty() {
            let (_req_id, version) = self
                .comp
                .start_source_request(CompletionSource::BufferWords);
            let matches: Vec<String> = self
                .buffer_words
                .iter()
                .filter(|w| w.starts_with(&prefix) && w.as_str() != prefix)
                .take(30)
                .cloned()
                .collect();
            self.comp
                .merge_source(CompletionSource::BufferWords, matches, version);
        }

        // ── Source 3: Vocab words (fast, sync) ─────────────────────
        if !self.vocab_words.is_empty() {
            let (_req_id, version) = self.comp.start_source_request(CompletionSource::VocabWords);
            let matches: Vec<String> = self
                .vocab_words
                .iter()
                .filter(|w| w.starts_with(&prefix) && w.as_str() != prefix)
                .take(30)
                .cloned()
                .collect();
            self.comp
                .merge_source(CompletionSource::VocabWords, matches, version);
        }

        // ── Source 4: LSP (async — response arrives in poll_lsp_responses) ──
        if self.lsp_full_active && self.config.lsp_completion_enabled {
            let (_req_id, version) = self.comp.start_source_request(CompletionSource::Lsp);
            if let Some(filename) = self.active_filename() {
                let path = std::path::PathBuf::from(filename);
                let lsp_line = row as u32;
                let lsp_col = col as u32;
                self.lsp_request_completion(&path, lsp_line, lsp_col, version);
            } else {
                self.comp
                    .merge_source(CompletionSource::Lsp, Vec::new(), version);
            }
        }
    }

    /// Handle LSP completion response — called from handle_lsp_message.
    pub fn apply_lsp_completion(&mut self, items: Option<Vec<CompletionItem>>, version: u64) {
        let prefix = self.comp.prefix().to_string();
        let prefix_lower = prefix.to_lowercase();
        let labels = match items {
            Some(items) => items
                .into_iter()
                .filter_map(|item| {
                    // Use filter_text if the server provides it, else label
                    let filter_key = item.filter_text.as_deref().unwrap_or(&item.label);
                    if !prefix.is_empty() && !filter_key.to_lowercase().starts_with(&prefix_lower) {
                        return None;
                    }
                    Some(item.get_insert_text().unwrap_or_else(|| item.label.clone()))
                })
                .collect(),
            None => Vec::new(),
        };
        self.comp
            .merge_source(CompletionSource::Lsp, labels, version);
    }

    /// LSP response handler — called when the LSP server replies.
    pub fn ingest_lsp_completion(&mut self, request_id: usize, version: u64, items: Vec<String>) {
        let expected = self.comp.get_pending_request_id(CompletionSource::Lsp);

        match expected {
            Some(id) if id == request_id => {
                self.comp
                    .merge_source(CompletionSource::Lsp, items, version);
            }
            _ => {}
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// Vocabulary & Word Index
// ═══════════════════════════════════════════════════════════════════════════

impl Editor {
    // tag_vocab
    pub fn refresh_buffer_words(&mut self) {
        if !self.config.buffer_word_scan {
            self.buffer_words.clear();
            return;
        }
        let total = self.buf().len_lines();
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

    /// Call `refresh_buffer_words()` only when the config allows it.
    // tag_vocab
    pub fn maybe_refresh_buffer_words(&mut self) {
        if self.config.buffer_word_scan {
            self.refresh_buffer_words();
        }
    }

    fn preload_vocabulary() -> HashSet<String> {
        if let Ok(config) = Config::load() {
            if !config.vocab_wordlist {
                return HashSet::new();
            }
        }
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

// ═══════════════════════════════════════════════════════════════════════════
// Messages & Display
// ═══════════════════════════════════════════════════════════════════════════

impl Editor {
    /// Display a message: single-line → status bar, multi-line → error popup (max 5 lines).
    pub fn show_message(&mut self, msg: impl Into<String>) {
        let msg = msg.into();
        if msg.lines().count() > 1 {
            self.popup.open_error(&msg);
        } else {
            self.set_status_msg(&msg, MessageKind::Error);
        }
    }

    /// Display an error: multi-line → error popup, single-line → status bar.
    pub fn show_error(&mut self, err: anyhow::Error) {
        self.show_message(err.to_string());
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// Command Line Helpers
// ═══════════════════════════════════════════════════════════════════════════

impl Editor {
    /// Helper to inject text at the current command cursor position.
    pub fn insert_command_text(&mut self, text: &str) {
        self.command.insert_str(self.command_cursor, text);
        self.command_cursor += text.len();
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// Function Analysis
// ═══════════════════════════════════════════════════════════════════════════

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

        let char_off = buf.rope.line_to_char(row).saturating_add(col);
        let text = buf.rope.slice(..).to_string();
        let byte_off = text.char_indices().nth(char_off).map(|(b, _)| b)?;

        let tree = buf.syntax.tree.as_ref()?;
        let root = tree.root_node();

        let cursor_node = root.descendant_for_byte_range(byte_off, byte_off + 1)?;

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

        let nested_fn_count = count_nested_fns(func_node);

        Some(FunctionSpanInfo {
            start_row,
            end_row,
            line_count,
            nested_fn_count,
        })
    }
}

impl Editor {
    /// Yank text into the specified register (or unnamed if None).
    /// Routes to system clipboard for `+`/`*`, named register for `a`-`z`,
    /// and always populates the unnamed register.
    pub fn yank_to_register(&mut self, text: String, register: Option<char>) {
        match register {
            Some('+') | Some('*') => {
                if crate::ed::clipboard::write_system_clipboard(&text) {
                    self.set_status_msg("Yanked to system clipboard", MessageKind::Success);
                } else {
                    self.set_status_msg(
                        "Yanked internally (system clipboard unavailable)",
                        MessageKind::Info,
                    );
                }
            }
            Some(c) if c.is_ascii_lowercase() => {
                self.registers.named.insert(c, text.clone());
                self.set_status_msg(&format!("Yanked to register \"{}", c), MessageKind::Info);
            }
            _ => {}
        }

        // Always populate the unnamed register
        self.clipboard = Some(text);
        self.clipboard_is_block = false;
    }

    /// Get text from the specified register (or unnamed if None).
    /// Routes from system clipboard for `+`/`*`, named register for `a`-`z`,
    /// and falls back to the unnamed register.
    pub fn paste_from_register(&mut self, register: Option<char>) -> Option<String> {
        match register {
            Some('+') | Some('*') => crate::ed::clipboard::read_system_clipboard(),
            Some(c) if c.is_ascii_lowercase() => self.registers.named.get(&c).cloned(),
            _ => self.clipboard.clone(),
        }
    }
    /// Open the registers popup showing all populated registers.
    pub fn open_registers_popup(&mut self) {
        let mut entries = Vec::new();
        let truncate = |s: &str, max: usize| -> String {
            let single_line = s.replace('\n', "↵");
            if single_line.chars().count() > max {
                let end = single_line
                    .char_indices()
                    .nth(max)
                    .map(|(i, _)| i)
                    .unwrap_or(single_line.len());
                format!("{}…", &single_line[..end])
            } else {
                single_line
            }
        };

        // ── Unnamed register (") ─────────────────────────────────
        if let Some(ref text) = self.clipboard {
            entries.push(crate::popup::registers::RegisterEntry {
                name: '"',
                label: "Unnamed (last d/y)".into(),
                preview: truncate(text, 50),
            });
        }

        // ── Yank register (0) ────────────────────────────────────
        if let Some(ref text) = self.yank_register_0 {
            entries.push(crate::popup::registers::RegisterEntry {
                name: '0',
                label: "Last yank".into(),
                preview: truncate(text, 50),
            });
        }

        // ── System clipboard (+) ─────────────────────────────────
        if let Some(text) = crate::ed::clipboard::read_system_clipboard() {
            entries.push(crate::popup::registers::RegisterEntry {
                name: '+',
                label: "System clipboard".into(),
                preview: truncate(&text, 50),
            });
        }

        // ── Small delete register (-) ────────────────────────────
        if let Some(ref text) = self.small_delete_register {
            entries.push(crate::popup::registers::RegisterEntry {
                name: '-',
                label: "Last small delete".into(),
                preview: truncate(text, 50),
            });
        }

        // ── Current file (%) ─────────────────────────────────────
        // Always show this register (Vim behavior), even for [No Name] buffers.
        {
            let filename = self
                .active_filename()
                .map(|s| s.to_string())
                .unwrap_or_else(|| "[No Name]".to_string());
            entries.push(crate::popup::registers::RegisterEntry {
                name: '%',
                label: "Current file".into(),
                preview: filename,
            });
        }

        // ── Last search (/) ─────────────────────────────────────
        if let Some(ref pattern) = self.last_search_query {
            entries.push(crate::popup::registers::RegisterEntry {
                name: '/',
                label: "Last search".into(),
                preview: pattern.clone(),
            });
        }

        // ── Named registers (a-z) ────────────────────────────────
        for c in 'a'..='z' {
            if let Some(text) = self.registers.named.get(&c) {
                entries.push(crate::popup::registers::RegisterEntry {
                    name: c,
                    label: format!("Named register '{}'", c),
                    preview: truncate(text, 50),
                });
            }
        }

        if entries.is_empty() {
            self.set_status_msg("All registers are empty", MessageKind::Info);
            return;
        }

        self.popup.registers = Some(crate::popup::registers::RegistersPopup::new(entries));
        self.popup.kind = Some(crate::popup::PopupKind::Registers);
    }
}

impl Editor {
    /// Get the LSP sender, if available.
    fn lsp_sender(&self) -> Option<&tokio::sync::mpsc::UnboundedSender<LspMessage>> {
        self.lsp_tx.as_ref()
    }

    // ── File lifecycle ────────────────────────────────────────────

    pub fn lsp_notify_open(&mut self, path: std::path::PathBuf) {
        if let Some(tx) = self.lsp_sender() {
            let _ = tx.send(LspMessage::OpenFile(path));
        }
    }

    pub fn lsp_notify_close(&mut self, path: std::path::PathBuf) {
        if let Some(tx) = self.lsp_sender() {
            let _ = tx.send(LspMessage::CloseFile(path));
        }
    }

    pub fn lsp_notify_change(&mut self, path: std::path::PathBuf) {
        if self.lsp_tx.is_none() {
            return;
        }
        let uri = path_to_uri(&path);
        let version = {
            let v = self.lsp_file_versions.entry(uri).or_insert(1);
            *v += 1;
            *v
        };
        let text = self.buf().rope.to_string();
        if let Some(tx) = self.lsp_tx.as_ref() {
            let _ = tx.send(LspMessage::ChangeFile(path, String::new(), text, version));
        }
    }

    pub fn lsp_notify_save(&mut self, path: std::path::PathBuf) {
        if let Some(tx) = self.lsp_sender() {
            let _ = tx.send(LspMessage::SaveFile(path));
        }
    }

    pub fn lsp_notify_change_incremental(
        &mut self,
        path: std::path::PathBuf,
        start_line: u32,
        start_char: u32,
        end_line: u32,
        end_char: u32,
        new_text: String,
    ) {
        if self.lsp_tx.is_none() {
            return;
        }
        let uri = path_to_uri(&path);
        let version = {
            let v = self.lsp_file_versions.entry(uri).or_insert(1);
            *v += 1;
            *v
        };
        if let Some(tx) = self.lsp_tx.as_ref() {
            let _ = tx.send(LspMessage::ChangeFileIncremental {
                path,
                version,
                start_line,
                start_char,
                end_line,
                end_char,
                new_text,
            });
        }
    }

    // ── Completion request ────────────────────────────────────────
    pub fn lsp_request_completion(
        &mut self,
        path: &std::path::PathBuf,
        line: u32,
        col: u32,
        version: u64,
    ) {
        if let Some(tx) = self.lsp_sender() {
            let _ = tx.send(LspMessage::RequestCompletion(
                path.clone(),
                line,
                col,
                None,
                version,
            ));
        }
    }

    // ── Signature help ────────────────────────────────────────────

    pub fn lsp_request_signature_help(&mut self, path: &std::path::PathBuf, line: u32, col: u32) {
        if let Some(tx) = self.lsp_sender() {
            let _ = tx.send(LspMessage::RequestSignatureHelp(path.clone(), line, col));
        }
    }

    // ── Formatting ────────────────────────────────────────────────
    pub fn lsp_request_formatting(
        &mut self,
        path: std::path::PathBuf,
        buffer_idx: usize,
        save_after: bool,
    ) {
        if let Some(tx) = self.lsp_sender() {
            let win = self.active_window();
            let cursor = Some((win.row, win.col));
            let text = self.buf().rope.to_string();
            let tab_size = self.config.tab_size as u32;
            let options = FormattingOptions {
                tab_size,
                insert_spaces: self.config.insert_spaces,
                trim_trailing_whitespace: Some(true),
                insert_final_newline: Some(true),
                trim_final_newlines: Some(true),
            };
            let _ = tx.send(LspMessage::RequestFormatting(
                path, text, options, buffer_idx, cursor, save_after,
            ));
        }
    }

    // ── Inlay hints ───────────────────────────────────────────────

    pub fn lsp_request_inlay_hints(&mut self, path: &std::path::PathBuf, version: i32) {
        if let Some(tx) = self.lsp_sender() {
            let total_lines = self.buf().len_lines();
            let _ = tx.send(LspMessage::RequestInlayHintsRange(
                path.clone(),
                0,
                total_lines as usize,
                version,
            ));
        }
    }

    fn apply_lsp_diagnostics(
        &mut self,
        uri: &str,
        diagnostics: Vec<crate::ed::buffer::Diagnostic>,
    ) {
        let path = uri_to_path(uri);
        for buf in &mut self.buffers {
            if buf.filename.as_deref() == path.to_str() {
                buf.diagnostics = diagnostics;
                return;
            }
        }
    }

    fn apply_lsp_format(
        &mut self,
        result: Result<Option<Vec<TextEdit>>, String>,
        buffer_idx: usize,
        cursor_state: Option<(usize, usize)>,
        save_after: bool,
    ) {
        let edits = match result {
            Ok(Some(edits)) => edits,
            Ok(None) => return,
            Err(e) => {
                self.set_status_msg(&format!("Format error: {}", e), MessageKind::Error);
                return;
            }
        };

        if edits.is_empty() {
            return;
        }

        if buffer_idx >= self.buffers.len() {
            return;
        }

        // ── Apply edits in a scoped block to release the borrow ──────
        let (line_count, filename) = {
            let buf = &mut self.buffers[buffer_idx];

            let mut sorted_edits = edits.clone();
            sorted_edits.sort_by(|a, b| {
                let a_start =
                    a.range.start.line as usize * 1_000_000 + a.range.start.character as usize;
                let b_start =
                    b.range.start.line as usize * 1_000_000 + b.range.start.character as usize;
                b_start.cmp(&a_start)
            });

            for edit in &sorted_edits {
                let start_line = edit.range.start.line as usize;
                let start_char = edit.range.start.character as usize;
                let end_line = edit.range.end.line as usize;
                let end_char = edit.range.end.character as usize;

                if start_line >= buf.len_lines() || end_line >= buf.len_lines() {
                    continue;
                }

                let start_offset = buf.rope.line_to_char(start_line)
                    + start_char.min(buf.line_char_len(start_line));
                let end_offset =
                    buf.rope.line_to_char(end_line) + end_char.min(buf.line_char_len(end_line));

                if end_offset > start_offset {
                    buf.rope.remove(start_offset..end_offset);
                }
                if !edit.new_text.is_empty() {
                    buf.rope.insert(start_offset, &edit.new_text);
                }
            }

            buf.mark_modified();
            buf.parse_syntax();

            (buf.len_lines(), buf.filename.clone())
        };

        // ── Restore cursor (borrow on buffers is now released) ───────
        if let Some((row, col)) = cursor_state {
            let win = self.active_window_mut();
            win.row = row.min(line_count.saturating_sub(1));
            win.col = col;
        }

        // ── Optionally save after formatting ─────────────────────────
        if save_after && filename.is_some() {
            let _ = self.save_active_buffer();
        }
    }

    fn apply_lsp_inlay_hints(&mut self, uri: String, hints: Vec<InlayHint>, _version: i32) {
        self.inlay_hints.insert(uri, hints);
    }

    // ── Shutdown ──────────────────────────────────────────────────

    pub fn lsp_shutdown(&mut self) {
        if let Some(tx) = self.lsp_sender() {
            let _ = tx.send(LspMessage::Shutdown);
        }
    }
}

impl Editor {
    pub fn poll_lsp_responses(&mut self) {
        let messages: Vec<_> = {
            let Some(rx) = &mut self.lsp_rx else { return };
            std::iter::from_fn(|| rx.try_recv().ok()).collect()
        };
        for msg in messages {
            self.handle_lsp_message(msg);
        }

        self.check_lsp_goto_timeout();
    }

    fn handle_lsp_message(&mut self, msg: AppMessage) {
        match msg {
            AppMessage::LspDiagnostics {
                uri, diagnostics, ..
            } => {
                self.apply_lsp_diagnostics(&uri, diagnostics);
            }

            AppMessage::LspFormatResult {
                result,
                buffer_idx,
                cursor_state,
                save_after,
            } => {
                self.apply_lsp_format(result, buffer_idx, cursor_state, save_after);
            }

            AppMessage::LspInlayHints {
                uri,
                hints,
                version,
            } => {
                self.apply_lsp_inlay_hints(uri, hints, version);
            }

            AppMessage::LspSignatureHelp(state) => {
                self.signature_help = state;
            }

            AppMessage::LspCompletion { items, version } => {
                self.apply_lsp_completion(items, version);
            }

            AppMessage::LspCompletionResolved(item) => {
                log::debug!("Resolved completion item: {:?}", item.label);
            }

            AppMessage::LspError(e) => {
                log::warn!("LSP error: {}", e);
            }

            AppMessage::LspGotoDefinitionResult { locations } => {
                self.handle_lsp_goto_result(locations);
            }
        }
    }

    /// LSP responded to go-to-definition.
    fn handle_lsp_goto_result(&mut self, locations: Vec<Location>) {
        let pending = match self.pending_lsp_goto.take() {
            Some(p) => p,
            None => return,
        };

        if locations.is_empty() {
            // LSP says no definition — fall through to ctagd / ctags
            if pending.pushed_tag_stack {
                self.tag_manager.pop();
            }
            self.tag_goto_fallback(&pending.symbol);
            return;
        }

        if locations.len() == 1 {
            self.lsp_do_goto_jump(&locations[0], &pending.symbol, 1);
        } else {
            self.lsp_goto_picker(locations, &pending.symbol);
        }
    }

    /// LSP timeout — fall back to ctagd / ctags.
    pub fn check_lsp_goto_timeout(&mut self) {
        let timed_out = self.pending_lsp_goto.as_ref().map_or(false, |p| {
            p.created_at.elapsed() > std::time::Duration::from_millis(LSP_GOTO_TIMEOUT_MS)
        });

        if !timed_out {
            return;
        }

        let pending = self.pending_lsp_goto.take().unwrap();
        if pending.pushed_tag_stack {
            self.tag_manager.pop();
        }

        self.set_status_msg(
            &format!("LSP timeout — falling back for '{}'", pending.symbol),
            MessageKind::Info,
        );

        self.tag_goto_fallback(&pending.symbol);
    }

    /// Fall back to ctagd → ctags (used when LSP times out or returns empty).
    fn tag_goto_fallback(&mut self, symbol: &str) {
        if self.ctagd.is_available() {
            if let Some(()) = self.ctagd_definition(symbol) {
                return;
            }
        }
        self.ctags_jump(symbol);
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// Selection Text Extraction
// ═══════════════════════════════════════════════════════════════════════════

impl Editor {
    /// Extract text from an inclusive line range in the active buffer.
    /// Returns `None` if the range is invalid.
    pub fn extract_line_range_text(&self, start_row: usize, end_row: usize) -> Option<String> {
        let buf = self.buf();
        if end_row >= buf.len_lines() || start_row > end_row {
            return None;
        }
        let mut result = String::new();
        for r in start_row..=end_row {
            let line = buf.line_text(r);
            let trimmed = line.trim_end_matches('\n').trim_end_matches('\r');
            result.push_str(trimmed);
            result.push('\n');
        }
        Some(result)
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// EasyMotion
// ═══════════════════════════════════════════════════════════════════════════

impl Editor {
    /// Enter EasyMotion mode (triggered by `s` in Normal mode).
    pub fn enter_easymotion(&mut self) {
        self.easymotion = Some(EasyMotionState {
            prefix: String::new(),
            phase: EasyMotionPhase::Collecting,
            targets: Vec::new(),
            partial_label: String::new(),
            label_len: 1,
        });
        self.set_status_msg("EasyMotion: _", MessageKind::Info);
    }

    /// Handle a key event while EasyMotion state is active.
    fn handle_easymotion_key(&mut self, key: KeyEvent) {
        if key.kind != crossterm::event::KeyEventKind::Press {
            return;
        }

        let phase = self
            .easymotion
            .as_ref()
            .map(|em| em.phase.clone())
            .unwrap_or(EasyMotionPhase::Collecting);

        match phase {
            EasyMotionPhase::Collecting => match key.code {
                KeyCode::Esc => {
                    self.easymotion = None;
                    self.clear_status_msg();
                }
                KeyCode::Backspace => {
                    // Copy out the prefix before mutating self further
                    let should_cancel = {
                        let em = self.easymotion.as_mut().unwrap();
                        em.prefix.pop();
                        em.prefix.is_empty()
                    };
                    if should_cancel {
                        self.easymotion = None;
                        self.clear_status_msg();
                    } else {
                        let prefix_display = self.easymotion.as_ref().unwrap().prefix.clone();
                        self.set_status_msg(
                            &format!("EasyMotion: {}_", prefix_display),
                            MessageKind::Info,
                        );
                    }
                }
                KeyCode::Char(c) => {
                    let should_scan = {
                        let em = self.easymotion.as_mut().unwrap();
                        em.prefix.push(c);
                        em.prefix.len() >= 2
                    };
                    if should_scan {
                        self.easymotion_scan();
                    } else {
                        let prefix_display = self.easymotion.as_ref().unwrap().prefix.clone();
                        self.set_status_msg(
                            &format!("EasyMotion: {}_", prefix_display),
                            MessageKind::Info,
                        );
                    }
                }
                _ => {}
            },

            EasyMotionPhase::Selecting => match key.code {
                KeyCode::Esc => {
                    self.easymotion = None;
                    self.clear_status_msg();
                }
                KeyCode::Backspace => {
                    let em = self.easymotion.as_mut().unwrap();
                    em.partial_label.pop();
                    // Stay in selecting; the next render will show the wider set.
                }
                KeyCode::Char(c) => {
                    let action = {
                        let em = self.easymotion.as_mut().unwrap();
                        em.partial_label.push(c);

                        if em.partial_label.len() >= em.label_len {
                            let label = em.partial_label.clone();
                            match em.targets.iter().find(|t| t.label == label) {
                                Some(target) => Some((target.row, target.col)),
                                None => None, // no match → cancel below
                            }
                        } else {
                            None // not yet complete, keep waiting
                        }
                    }; // em borrow ends here

                    // Now decide outside the borrow
                    let em_ref = self.easymotion.as_ref();
                    let label_len = em_ref.as_ref().map(|e| e.label_len).unwrap_or(1);
                    let partial_len = em_ref.as_ref().map(|e| e.partial_label.len()).unwrap_or(0);
                    let done = partial_len >= label_len;

                    if done {
                        if let Some((target_row, target_col)) = action {
                            self.easymotion = None;
                            self.clear_status_msg();
                            self.active_window_mut().save_jump_position();
                            let win = self.active_window_mut();
                            win.row = target_row;
                            win.col = target_col;
                            win.desired_col = target_col;
                            self.snap_cursor_to_viewport();
                        } else {
                            self.easymotion = None;
                            self.set_status_msg("No such EasyMotion target", MessageKind::Error);
                        }
                    }
                }
                _ => {}
            },
        }
    }

    /// Scan the visible viewport for the 2-char prefix and assign labels.
    fn easymotion_scan(&mut self) {
        let prefix = self.easymotion.as_ref().unwrap().prefix.clone();
        let (scroll, viewport_h) = {
            let win = self.active_window();
            (win.scroll_line, win.position.height)
        };
        let buf = self.buf();

        let start_line = scroll;
        let end_line = (scroll + viewport_h).min(buf.len_lines());

        let mut targets = Vec::new();

        for row in start_line..end_line {
            let line = buf.line_text(row);
            let line_trimmed = line.trim_end_matches('\n').trim_end_matches('\r');
            let mut search_start = 0;

            while let Some(byte_pos) = line_trimmed[search_start..].find(&prefix) {
                let abs_byte = search_start + byte_pos;
                // Convert byte offset → char offset (win.col is char-based)
                let char_col = line_trimmed[..abs_byte].chars().count();
                targets.push(EasyMotionTarget {
                    row,
                    col: char_col,
                    label: String::new(),
                });
                search_start = abs_byte + prefix.len();
            }
        }

        if targets.is_empty() {
            self.set_status_msg(&format!("No matches for '{}'", prefix), MessageKind::Error);
            self.easymotion = None;
            return;
        }

        // ── Assign labels ──────────────────────────────────────────
        let count = targets.len();
        let label_len = if count <= 26 { 1 } else { 2 };

        for (i, target) in targets.iter_mut().enumerate() {
            target.label = if label_len == 1 {
                let c = (b'a' + i as u8) as char;
                c.to_string()
            } else {
                let first = (b'a' + (i / 26) as u8) as char;
                let second = (b'a' + (i % 26) as u8) as char;
                format!("{}{}", first, second)
            };
        }

        // ── Single target → auto-jump ──────────────────────────────
        if targets.len() == 1 {
            let target = targets.into_iter().next().unwrap();
            self.easymotion = None;
            self.clear_status_msg();
            self.active_window_mut().save_jump_position();
            let win = self.active_window_mut();
            win.row = target.row;
            win.col = target.col;
            win.desired_col = target.col;
            self.snap_cursor_to_viewport();
            return;
        }

        let em = self.easymotion.as_mut().unwrap();
        em.targets = targets;
        em.phase = EasyMotionPhase::Selecting;
        em.label_len = label_len;
        em.partial_label = String::new();

        self.set_status_msg(
            &format!("EasyMotion: {} — type label to jump", prefix),
            MessageKind::Info,
        );
    }
}
