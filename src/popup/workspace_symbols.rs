//! Popup for `workspace_symbols` results from the `ctagd` daemon.
//!
//! Follows the same UI pattern as `FunctionListPopup`.

use crate::lsp::ctagd::SymbolResult;

/// A popup listing symbol search results.
#[derive(Debug, Clone)]
pub struct WorkspaceSymbolsPopup {
    entries: Vec<SymbolResult>,
    selected: usize,
    scroll: usize,
}

impl WorkspaceSymbolsPopup {
    pub fn new(entries: Vec<SymbolResult>) -> Self {
        Self {
            entries,
            selected: 0,
            scroll: 0,
        }
    }

    pub fn entries(&self) -> &[SymbolResult] {
        &self.entries
    }

    pub fn selected(&self) -> usize {
        self.selected
    }

    pub fn selected_entry(&self) -> Option<&SymbolResult> {
        self.entries.get(self.selected)
    }

    pub fn scroll(&self) -> usize {
        self.scroll
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

    /// Recalculate scroll offset so `selected` is visible given `visible_height`.
    pub fn adjust_scroll(&mut self, visible_height: usize) {
        if visible_height == 0 {
            return;
        }
        if self.selected < self.scroll {
            self.scroll = self.selected;
        } else if self.selected >= self.scroll + visible_height {
            self.scroll = self.selected - visible_height + 1;
        }
    }
}
