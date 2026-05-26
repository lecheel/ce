//! Buffer list popup for browsing, switching, and closing active buffers.

use crate::popup::Scrollable;
use std::path::PathBuf;
#[derive(Debug, Clone)]
pub struct BufferEntry {
    pub id: usize,             // Buffer unique identifier
    pub name: String,          // Display name (e.g. filename)
    pub path: Option<PathBuf>, // Full path if available
    pub is_modified: bool,     // Unsaved changes state
    pub line_count: usize,     // Line count for context
}

#[derive(Debug, Clone)]
pub struct BufferList {
    pub entries: Vec<BufferEntry>,
    pub filtered: Vec<usize>,
    pub selected: usize,
    pub scroll: usize,
    pub filter: String,
    /// Indices of the matched query characters in the name (for highlighting)
    pub match_indices: Vec<Vec<usize>>,
    pub visible_height: usize,
}

impl BufferList {
    pub fn new(entries: Vec<BufferEntry>) -> Self {
        let mut list = BufferList {
            entries,
            filtered: Vec::new(),
            selected: 0,
            scroll: 0,
            filter: String::new(),
            match_indices: Vec::new(),
            visible_height: 20,
        };
        list.apply_filter();
        list
    }

    pub fn apply_filter(&mut self) {
        self.filtered.clear();
        self.match_indices.clear();

        let query = self.filter.trim().to_lowercase();

        for (i, entry) in self.entries.iter().enumerate() {
            if query.is_empty() {
                self.filtered.push(i);
                self.match_indices.push(Vec::new());
                continue;
            }

            if let Some(indices) = Self::fuzzy_match(&entry.name, &query) {
                self.filtered.push(i);
                self.match_indices.push(indices);
            }
        }

        if self.filtered.is_empty() {
            self.selected = 0;
        } else if self.selected >= self.filtered.len() {
            self.selected = self.filtered.len().saturating_sub(1);
        }

        self.clamp_scroll();
    }

    fn fuzzy_match(text: &str, query: &str) -> Option<Vec<usize>> {
        let text_lower: Vec<char> = text.to_lowercase().chars().collect();
        let query_chars: Vec<char> = query.chars().collect();

        let mut indices = Vec::with_capacity(query_chars.len());
        let mut qi = 0;

        for (ti, tc) in text_lower.iter().enumerate() {
            if qi < query_chars.len() && *tc == query_chars[qi] {
                indices.push(ti);
                qi += 1;
            }
        }

        if qi == query_chars.len() {
            Some(indices)
        } else {
            None
        }
    }

    fn clamp_scroll(&mut self) {
        if self.scroll > self.selected {
            self.scroll = self.selected;
        } else if self.selected >= self.scroll + self.visible_height {
            self.scroll = self.selected - self.visible_height + 1;
        }
    }

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

    pub fn move_up(&mut self) {
        if self.selected > 0 {
            self.selected -= 1;
            self.clamp_scroll();
        }
    }

    pub fn move_down(&mut self) {
        if !self.filtered.is_empty() && self.selected < self.filtered.len() - 1 {
            self.selected += 1;
            self.clamp_scroll();
        }
    }

    pub fn selected_entry(&self) -> Option<&BufferEntry> {
        self.filtered
            .get(self.selected)
            .and_then(|&i| self.entries.get(i))
    }
}

impl Scrollable for BufferList {
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
