//! A generic filtered, scrollable list used by every popup that shows
//! a searchable collection of entries.

use crate::popup::Scrollable;
use crossterm::event::{KeyCode, KeyEvent};

// ── Entry trait ────────────────────────────────────────────────────────

/// How an entry type declares itself matchable.
pub trait EntryFilter {
    /// Return `Some(highlight_char_indices)` if the entry matches `query`,
    /// `None` if it should be hidden.  An empty query must always return
    /// `Some(vec![])`.
    fn match_query(&self, query: &str) -> Option<Vec<usize>>;

    /// Pinned entries are always visible regardless of the filter.
    fn is_pinned(&self) -> bool {
        false
    }
}

// ── The list ───────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct FilteredList<E> {
    pub entries: Vec<E>,
    pub filtered: Vec<usize>,
    pub match_indices: Vec<Vec<usize>>,
    pub selected: usize,
    pub scroll: usize,
    pub filter: String,
    pub visible_height: usize,
    /// When true, move_up/move_down wrap around.
    pub wraps: bool,
}

impl<E: EntryFilter> FilteredList<E> {
    pub fn new(entries: Vec<E>) -> Self {
        let mut list = Self {
            entries,
            filtered: Vec::new(),
            match_indices: Vec::new(),
            selected: 0,
            scroll: 0,
            filter: String::new(),
            visible_height: 20,
            wraps: false,
        };
        list.apply_filter();
        list
    }

    /// Replace the entire entry set (e.g. after directory change in FilePicker).
    pub fn set_entries(&mut self, entries: Vec<E>) {
        self.entries = entries;
        self.selected = 0;
        self.scroll = 0;
        self.apply_filter();
    }

    /// Re-run the filter against the current `entries` and `filter`.
    pub fn apply_filter(&mut self) {
        self.filtered.clear();
        self.match_indices.clear();

        let query = self.filter.trim();

        for (i, entry) in self.entries.iter().enumerate() {
            if entry.is_pinned() {
                self.filtered.push(i);
                self.match_indices.push(Vec::new());
                continue;
            }

            if query.is_empty() {
                self.filtered.push(i);
                self.match_indices.push(Vec::new());
                continue;
            }

            if let Some(indices) = entry.match_query(query) {
                self.filtered.push(i);
                self.match_indices.push(indices);
            }
        }

        self.clamp_selected();
        self.clamp_scroll();
    }

    // ── Filter manipulation ─────────────────────────────────────────

    pub fn filter_push(&mut self, c: char) {
        self.filter.push(c);
        self.selected = 0;
        self.scroll = 0;
        self.apply_filter();
    }

    pub fn filter_pop(&mut self) {
        self.filter.pop();
        self.selected = 0;
        self.scroll = 0;
        self.apply_filter();
    }

    pub fn filter_clear(&mut self) {
        self.filter.clear();
        self.selected = 0;
        self.scroll = 0;
        self.apply_filter();
    }

    // ── Navigation ──────────────────────────────────────────────────

    pub fn move_up(&mut self) {
        if self.filtered.is_empty() {
            return;
        }
        if self.selected > 0 {
            self.selected -= 1;
        } else if self.wraps {
            self.selected = self.filtered.len() - 1;
        }
        self.clamp_scroll();
    }

    pub fn move_down(&mut self) {
        if self.filtered.is_empty() {
            return;
        }
        if self.selected + 1 < self.filtered.len() {
            self.selected += 1;
        } else if self.wraps {
            self.selected = 0;
        }
        self.clamp_scroll();
    }

    /// Handle Up/Down/Backspace/Char. Returns `true` if consumed.
    pub fn dispatch_nav(&mut self, key: &KeyEvent) -> bool {
        match key.code {
            KeyCode::Up => {
                self.move_up();
                true
            }
            KeyCode::Down => {
                self.move_down();
                true
            }
            KeyCode::Backspace => {
                self.filter_pop();
                true
            }
            KeyCode::Char(c) => {
                self.filter_push(c);
                true
            }
            _ => false,
        }
    }

    // ── Accessors ───────────────────────────────────────────────────

    pub fn selected_entry(&self) -> Option<&E> {
        self.filtered
            .get(self.selected)
            .and_then(|&i| self.entries.get(i))
    }

    pub fn selected_entry_mut(&mut self) -> Option<&mut E> {
        let idx = *self.filtered.get(self.selected)?;
        self.entries.get_mut(idx)
    }

    /// Index into `entries` for the current selection.
    pub fn selected_entry_idx(&self) -> Option<usize> {
        self.filtered.get(self.selected).copied()
    }

    pub fn is_empty(&self) -> bool {
        self.filtered.is_empty()
    }

    pub fn len(&self) -> usize {
        self.filtered.len()
    }

    pub fn filter_is_empty(&self) -> bool {
        self.filter.is_empty()
    }

    // ── Scroll helpers ──────────────────────────────────────────────

    fn clamp_selected(&mut self) {
        if self.filtered.is_empty() {
            self.selected = 0;
        } else if self.selected >= self.filtered.len() {
            self.selected = self.filtered.len() - 1;
        }
    }

    fn clamp_scroll(&mut self) {
        if self.scroll > self.selected {
            self.scroll = self.selected;
        } else if self.selected >= self.scroll + self.visible_height {
            self.scroll = self.selected - self.visible_height + 1;
        }
    }
}

// ── Blanket Scrollable impl ────────────────────────────────────────────

impl<E: EntryFilter> Scrollable for FilteredList<E> {
    fn selected(&self) -> usize {
        self.selected
    }
    fn selected_mut(&mut self) -> &mut usize {
        &mut self.selected
    }
    fn scroll_mut(&mut self) -> &mut usize {
        &mut self.scroll
    }
    fn len(&self) -> usize {
        self.filtered.len()
    }
    fn visible_rows(&self) -> usize {
        self.visible_height
    }
}
