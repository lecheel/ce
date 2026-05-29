//! Popup for selecting between multiple tag candidates.

use crate::ed::tag::TagEntry;

#[derive(Debug, Clone)]
pub struct TagCandidatesPopup {
    pub entries: Vec<TagEntry>,
    pub selected: usize,
}

impl TagCandidatesPopup {
    pub fn new(entries: Vec<TagEntry>) -> Self {
        Self {
            entries,
            selected: 0,
        }
    }

    pub fn move_up(&mut self) {
        if self.selected > 0 {
            self.selected -= 1;
        }
    }

    pub fn move_down(&mut self) {
        if self.selected + 1 < self.entries.len() {
            self.selected += 1;
        }
    }

    pub fn selected_entry(&self) -> Option<&TagEntry> {
        self.entries.get(self.selected)
    }
}
