// popup/marks.rs
//! Marks popup overlay for viewing and jumping to named bookmarks across all open buffers.

use crate::popup::Scrollable;

#[derive(Debug, Clone)]
pub struct MarkEntry {
    pub ch: char,
    pub row: usize,
    pub col: usize,
    pub buffer_id: usize,
    pub buffer_name: String,
}

#[derive(Debug, Clone)]
pub struct MarksPopup {
    pub entries: Vec<MarkEntry>,
    pub selected: usize,
    pub scroll: usize,
}

impl MarksPopup {
    pub fn new(mut entries: Vec<MarkEntry>) -> Self {
        // Sort by mark character, then by buffer name for consistent ordering
        entries.sort_by(|a, b| {
            a.ch.cmp(&b.ch)
                .then_with(|| a.buffer_name.cmp(&b.buffer_name))
        });
        Self {
            entries,
            selected: 0,
            scroll: 0,
        }
    }

    pub fn move_up(&mut self) {
        if self.selected > 0 {
            self.selected -= 1;
            self.clamp_scroll();
        }
    }

    pub fn move_down(&mut self) {
        if !self.entries.is_empty() && self.selected < self.entries.len() - 1 {
            self.selected += 1;
            self.clamp_scroll();
        }
    }

    fn clamp_scroll(&mut self) {
        let visible = 20;
        if self.selected < self.scroll {
            self.scroll = self.selected;
        } else if self.selected >= self.scroll + visible {
            self.scroll = self.selected - visible + 1;
        }
    }
}

impl Scrollable for MarksPopup {
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
        self.entries.len()
    }
    fn visible_rows(&self) -> usize {
        20
    }
}
