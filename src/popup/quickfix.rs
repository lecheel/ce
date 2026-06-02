// popup/quickfix.rs
//! Quickfix list popup for navigating ripgrep search results.

use crate::ed::ripgrep::RipgrepResult;
use crate::popup::Scrollable;

#[derive(Debug, Clone)]
pub struct QuickfixPopup {
    pub entries: Vec<RipgrepResult>,
    pub selected: usize,
    pub scroll: usize,
    /// The root directory for computing relative paths.
    pub root_dir: std::path::PathBuf,
}

impl QuickfixPopup {
    pub fn new(entries: Vec<RipgrepResult>, root_dir: std::path::PathBuf) -> Self {
        Self {
            entries,
            selected: 0,
            scroll: 0,
            root_dir,
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

impl Scrollable for QuickfixPopup {
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
