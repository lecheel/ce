// File: src/popup/mod.rs

pub mod buffer_list;
pub mod file_picker;
pub mod function_list;
pub mod git_hunk;
pub mod marks;
pub mod mru;

pub use buffer_list::{BufferEntry, BufferList};
pub use file_picker::FilePicker;
pub use function_list::FunctionListPopup;
pub use marks::{MarkEntry, MarksPopup};
pub use mru::MruPopup;

// ── Restore the Scrollable helper trait ────────────────────────────────
pub trait Scrollable {
    fn selected(&self) -> usize;
    fn selected_mut(&mut self) -> &mut usize;
    fn scroll_mut(&mut self) -> &mut usize;
    fn len(&self) -> usize;
    fn visible_rows(&self) -> usize;
}

/// Discriminant for the different popup flavours the editor may show.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PopupKind {
    Completion,
    CommandPalette,
    Hover,
    Config,
    Custom,
    Scankey,
    WhichKey,
    FilePicker,
    BufferList,
    Marks,
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

// ── Single, Unified PopupState Definition ─────────────────────────────

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

    // -- FilePicker specific --
    pub file_picker: Option<FilePicker>,
    pub last_file_picker_dir: Option<std::path::PathBuf>,

    // -- FunctionList specific --
    pub function_list: Option<FunctionListPopup>,
    pub git_hunk: Option<crate::popup::git_hunk::GitHunkPopup>,
    pub buffer_list: Option<BufferList>, // Keeps buffer picker state

    // -- Explicit popup content (Config, Scankey) --
    pub content: Option<PopupContent>,
    pub mru: Option<MruPopup>,

    pub marks: Option<MarksPopup>,
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
    }

    // -- Config popup --

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

    // -- Scankey popup --

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

    // -- FilePicker popup --

    pub fn open_file_picker(&mut self, initial_path: &std::path::Path, flat: bool) {
        self.close();
        self.file_picker = Some(FilePicker::new(initial_path, flat));
        self.kind = Some(PopupKind::FilePicker);
    }

    // -- BufferList popup --

    pub fn open_buffer_list(&mut self, entries: Vec<BufferEntry>) {
        self.close();
        self.buffer_list = Some(BufferList::new(entries));
        self.kind = Some(PopupKind::BufferList);
    }
}
