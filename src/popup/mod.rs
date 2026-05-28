// File: src/popup/mod.rs

pub mod buffer_list;
pub mod command_palette;
pub mod file_picker;
pub mod function_list;
pub mod git_hunk;
pub mod guide;
pub mod marks;
pub mod mru;

pub use buffer_list::{BufferEntry, BufferList};
pub use command_palette::CommandPalettePopup;
pub use file_picker::FilePicker;
pub use function_list::FunctionListPopup;
pub use marks::{MarkEntry, MarksPopup};
pub use mru::MruPopup;

use crossterm::event::{KeyCode, KeyEvent};

// ═══════════════════════════════════════════════════════════════════════
// Shared traits
// ═══════════════════════════════════════════════════════════════════════

/// Primitive scroll-position abstraction used by the renderer.
pub trait Scrollable {
    fn selected(&self) -> usize;
    fn selected_mut(&mut self) -> &mut usize;
    fn scroll_mut(&mut self) -> &mut usize;
    fn len(&self) -> usize;
    fn visible_rows(&self) -> usize;
}

/// Common navigation + incremental-filter behaviour for list popups.
///
/// Implementing this trait lets any popup use [`dispatch_list_nav`] to
/// handle **Up / Down / Backspace / Char** keys automatically, cutting
/// the per-handler boilerplate from ~16 lines to one.
///
/// # Example
///
/// ```ignore
/// // In popup/mru.rs:
/// impl FilterableList for MruPopup {
///     fn move_up(&mut self)     { /* … */ }
///     fn move_down(&mut self)   { /* … */ }
///     fn filter_pop(&mut self)  { /* … */ }
///     fn filter_push(&mut self, c: char) { /* … */ }
/// }
///
/// // In the key handler:
/// if dispatch_list_nav(&mut self.popup.mru, &key) { return; }
/// // …handle popup-specific keys (Esc, Enter, Tab, etc.)…
/// ```
pub trait FilterableList {
    fn move_up(&mut self);
    fn move_down(&mut self);
    fn filter_pop(&mut self);
    fn filter_push(&mut self, c: char);
}

/// Dispatch common navigation keys (Up, Down, Backspace, Char) to a
/// filterable list popup.  Returns `true` if the key was consumed.
///
/// Returns `false` for any other key, so the caller can handle
/// popup-specific bindings (Esc, Enter, Tab, Delete, …) in its own
/// match.
pub fn dispatch_list_nav<L: FilterableList>(list: &mut Option<L>, key: &KeyEvent) -> bool {
    let inner = match list {
        Some(l) => l,
        None => return false,
    };
    match key.code {
        KeyCode::Up => {
            inner.move_up();
            true
        }
        KeyCode::Down => {
            inner.move_down();
            true
        }
        KeyCode::Backspace => {
            inner.filter_pop();
            true
        }
        KeyCode::Char(c) => {
            inner.filter_push(c);
            true
        }
        _ => false,
    }
}

// ═══════════════════════════════════════════════════════════════════════
// Popup discriminated union
// ═══════════════════════════════════════════════════════════════════════

/// Discriminant for the different popup flavours the editor may show.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PopupKind {
    Completion,
    CommandPalette,
    Hover,
    Config,
    Custom,
    Scankey,
    Whichkey,
    FilePicker,
    BufferList,
    Marks,
    Mru,
    Guide,
    GitHunk,
    FunctionList,
}

#[derive(Debug, Clone)]
pub struct PopupItem {
    pub label: String,
    pub detail: Option<String>,
    pub data: usize,
}

/// Explicit popup content — opened and closed by deliberate user actions.
#[derive(Debug, Clone)]
pub enum PopupContent {
    Config {
        items: Vec<PopupItem>,
        selected: usize,
    },
    Scankey {
        key_label: String,
        action_label: String,
        raw_label: String,
    },
}

// ═══════════════════════════════════════════════════════════════════════
// Single, unified PopupState
// ═══════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone, Default)]
pub struct PopupState {
    /// `None` means no popup is visible.
    pub kind: Option<PopupKind>,
    /// Items in the currently visible popup (used for completion etc.).
    pub items: Vec<PopupItem>,
    /// 0-based index of the selected item.
    pub selected: usize,

    // -- WhichKey specific --
    pub wk_pending: String,
    pub wk_suggestions: Vec<(String, String)>,

    // -- Scankey specific --
    pub sk_key_label: String,
    pub sk_action_label: String,
    pub sk_raw_label: String,

    // -- Typed popup states --
    pub file_picker: Option<FilePicker>,
    pub last_file_picker_dir: Option<std::path::PathBuf>,
    pub function_list: Option<FunctionListPopup>,
    pub git_hunk: Option<crate::popup::git_hunk::GitHunkPopup>,
    pub buffer_list: Option<BufferList>,
    pub mru: Option<MruPopup>,
    pub marks: Option<MarksPopup>,
    pub guide: Option<guide::GuidePopup>,
    pub command_palette: Option<CommandPalettePopup>,

    // -- Explicit popup content (Config, Scankey) --
    pub content: Option<PopupContent>,
}

impl PopupState {
    pub fn new() -> Self {
        Self {
            kind: None,
            items: Vec::new(),
            selected: 0,
            wk_pending: String::new(),
            wk_suggestions: Vec::new(),
            sk_key_label: String::new(),
            sk_action_label: String::new(),
            sk_raw_label: String::new(),
            file_picker: None,
            buffer_list: None,
            last_file_picker_dir: None,
            function_list: None,
            git_hunk: None,
            mru: None,
            content: None,
            marks: None,
            guide: None,
            command_palette: None,
        }
    }

    /// Whether a popup is currently visible.
    pub fn is_open(&self) -> bool {
        self.kind.is_some()
            || self.file_picker.is_some()
            || self.content.is_some()
            || self.function_list.is_some()
            || self.mru.is_some()
            || self.git_hunk.is_some()
            || self.buffer_list.is_some()
            || self.marks.is_some()
            || self.guide.is_some()
            || self.command_palette.is_some()
    }

    /// Close the popup and clear all specific data.
    pub fn close(&mut self) {
        self.kind = None;
        self.items.clear();
        self.selected = 0;
        self.wk_pending.clear();
        self.wk_suggestions.clear();
        self.sk_key_label.clear();
        self.sk_action_label.clear();
        self.sk_raw_label.clear();
        self.file_picker = None;
        self.function_list = None;
        self.mru = None;
        self.content = None;
        self.git_hunk = None;
        self.buffer_list = None;
        self.marks = None;
        self.guide = None;
        self.command_palette = None;
    }

    // ── Config popup ───────────────────────────────────────────────

    pub fn open_config(&mut self, items: Vec<PopupItem>, selected: usize) {
        self.close();
        self.kind = Some(PopupKind::Config);
        self.items = items.clone();
        self.selected = selected;
        self.content = Some(PopupContent::Config { items, selected });
    }

    pub fn config_next(&mut self) {
        if let Some(PopupContent::Config { items, selected }) = &mut self.content {
            if !items.is_empty() {
                *selected = (*selected + 1) % items.len();
                self.selected = *selected;
            }
        }
    }

    pub fn config_prev(&mut self) {
        if let Some(PopupContent::Config { items, selected }) = &mut self.content {
            if !items.is_empty() {
                *selected = selected.checked_sub(1).unwrap_or(items.len() - 1);
                self.selected = *selected;
            }
        }
    }

    pub fn current_config_item(&self) -> Option<&PopupItem> {
        if let Some(PopupContent::Config { items, selected }) = &self.content {
            items.get(*selected)
        } else {
            None
        }
    }

    // ── Scankey popup ──────────────────────────────────────────────

    pub fn open_scankey(&mut self, key_label: String, action_label: String, raw_label: String) {
        self.close();
        self.kind = Some(PopupKind::Scankey);
        self.sk_key_label = key_label.clone();
        self.sk_action_label = action_label.clone();
        self.sk_raw_label = raw_label.clone();
        self.content = Some(PopupContent::Scankey {
            key_label,
            action_label,
            raw_label,
        });
    }

    // ── FilePicker popup ───────────────────────────────────────────

    pub fn open_file_picker(&mut self, initial_path: &std::path::Path, flat: bool) {
        self.close();
        self.file_picker = Some(FilePicker::new(initial_path, flat));
        self.kind = Some(PopupKind::FilePicker);
    }

    // ── BufferList popup ───────────────────────────────────────────

    pub fn open_buffer_list(&mut self, entries: Vec<BufferEntry>) {
        self.close();
        self.buffer_list = Some(BufferList::new(entries));
        self.kind = Some(PopupKind::BufferList);
    }

    // ── Command Palette popup ──────────────────────────────────────

    pub fn open_command_palette(
        &mut self,
        entries: Vec<crate::popup::command_palette::CommandEntry>,
    ) {
        self.close();
        self.command_palette = Some(CommandPalettePopup::new(entries));
        self.kind = Some(PopupKind::CommandPalette);
    }

    // ── MRU popup ──────────────────────────────────────────────────

    pub fn open_mru(
        &mut self,
        entries: Vec<crate::popup::mru::MruEntry>,
        repo_root: Option<std::path::PathBuf>,
        repo_only: bool,
    ) {
        self.close();
        let mut popup = MruPopup::new(entries, repo_root, repo_only);
        popup.apply_filter();
        self.mru = Some(popup);
        self.kind = Some(PopupKind::Mru);
    }

    // ── Guide popup ────────────────────────────────────────────────

    pub fn open_guide(&mut self, entries: Vec<crate::ed::guide::GuideEntry>) {
        self.close();
        self.guide = Some(guide::GuidePopup::new(entries));
        self.kind = Some(PopupKind::Guide);
    }

    // ── GitHunk popup ──────────────────────────────────────────────

    pub fn open_git_hunk(&mut self, lines: Vec<String>) {
        self.close();
        self.git_hunk = Some(crate::popup::git_hunk::GitHunkPopup::new(lines));
        self.kind = Some(PopupKind::GitHunk);
    }

    // ── FunctionList popup ─────────────────────────────────────────

    pub fn open_function_list(
        &mut self,
        functions: Vec<crate::popup::function_list::FunctionEntry>,
    ) {
        self.close();
        self.function_list = Some(FunctionListPopup::new(functions));
        self.kind = Some(PopupKind::FunctionList);
    }

    // ── Marks popup ────────────────────────────────────────────────

    pub fn open_marks(&mut self, entries: Vec<MarkEntry>) {
        self.close();
        self.marks = Some(MarksPopup::new(entries));
        self.kind = Some(PopupKind::Marks);
    }
}
