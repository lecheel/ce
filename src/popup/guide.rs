// popup/guide.rs
//! Guide popup overlay for searchable checkpoints navigation.

use crate::ed::guide::GuideEntry;
use crate::popup::Scrollable;

/// Popup that lists all guide checkpoints for quick navigation and filtering.
#[derive(Debug, Clone)]
pub struct GuidePopup {
    pub all_entries: Vec<GuideEntry>,
    pub filtered: Vec<usize>,
    pub selected: usize,
    pub scroll: usize,
    pub filter: String,
}

impl GuidePopup {
    pub fn new(entries: Vec<GuideEntry>) -> Self {
        let filtered: Vec<usize> = (0..entries.len()).collect();
        Self {
            all_entries: entries,
            filtered,
            selected: 0,
            scroll: 0,
            filter: String::new(),
        }
    }

    pub fn selected_entry(&self) -> Option<&GuideEntry> {
        self.filtered
            .get(self.selected)
            .and_then(|&i| self.all_entries.get(i))
    }

    fn apply_filter(&mut self) {
        self.filtered.clear();
        let query = self.filter.to_lowercase();
        for (i, entry) in self.all_entries.iter().enumerate() {
            if query.is_empty()
                || entry.label.to_lowercase().contains(&query)
                || entry.desc.to_lowercase().contains(&query)
                || entry.file.to_lowercase().contains(&query)
                || entry.kind.to_lowercase().contains(&query)
                || entry.tags.iter().any(|t| t.contains(&query))
                || entry.anchor.to_lowercase().contains(&query)
            {
                self.filtered.push(i);
            }
        }
        if self.selected >= self.filtered.len() && !self.filtered.is_empty() {
            self.selected = self.filtered.len() - 1;
        }
        self.clamp_scroll(20);
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

    pub fn filter_clear(&mut self) {
        self.filter.clear();
        self.selected = 0;
        self.scroll = 0;
        self.apply_filter();
    }

    pub fn clamp_scroll(&mut self, visible_height: usize) {
        if self.scroll > self.selected {
            self.scroll = self.selected;
        }
        if self.selected >= self.scroll + visible_height {
            self.scroll = self.selected - visible_height + 1;
        }
    }

    pub fn move_up(&mut self) {
        if self.selected > 0 {
            self.selected -= 1;
            self.clamp_scroll(20);
        }
    }

    pub fn move_down(&mut self) {
        if self.selected + 1 < self.filtered.len() {
            self.selected += 1;
            self.clamp_scroll(20);
        }
    }
}

impl Scrollable for GuidePopup {
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
        20
    }
}
