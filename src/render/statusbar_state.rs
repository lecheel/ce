//! Debounced cursor-position cache for the status bar.

use std::time::{Duration, Instant};

const SETTLE_MS: u64 = 120;

pub struct StatusBarState {
    pub display_row: usize,
    pub display_col: usize,
    pub is_moving: bool,
    last_moved: Instant,
}

impl Default for StatusBarState {
    fn default() -> Self {
        Self {
            display_row: 0,
            display_col: 0,
            is_moving: false,
            last_moved: Instant::now(),
        }
    }
}

impl StatusBarState {
    /// Call once per render frame with the live cursor position.
    pub fn tick(&mut self, row: usize, col: usize) {
        let now = Instant::now();
        if row != self.display_row || col != self.display_col {
            self.last_moved = now;
            self.is_moving = true;
        }
        if self.is_moving
            && now.saturating_duration_since(self.last_moved) >= Duration::from_millis(SETTLE_MS)
        {
            self.display_row = row;
            self.display_col = col;
            self.is_moving = false;
        }
    }
}
