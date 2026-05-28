//! Position persistence: load, save, and restore cursor positions across
//! sessions.  Also contains window↔buffer switch and close helpers.

use crate::ed::editor::PositionInfo;
use crate::ed::MessageKind;
use crate::Config;
use crate::Editor;

impl Editor {
    // ═══════════════════════════════════════════════════════════════════
    // Persistence
    // ═══════════════════════════════════════════════════════════════════

    pub fn load_positions() -> Vec<PositionInfo> {
        Config::config_dir()
            .ok()
            .and_then(|dir| std::fs::read_to_string(dir.join("positions.json")).ok())
            .and_then(|content| serde_json::from_str(&content).ok())
            .unwrap_or_default()
    }

    pub fn save_positions(&self) {
        if let Ok(dir) = Config::config_dir() {
            let start = self.positions.len().saturating_sub(100);
            if let Ok(serialized) = serde_json::to_string_pretty(&self.positions[start..]) {
                let _ = std::fs::write(dir.join("positions.json"), serialized);
            }
        }
    }

    pub fn update_position(&mut self, path: &str, row: usize, col: usize) {
        self.update_position_silent(path, row, col);
        self.save_positions();
    }

    pub fn update_position_silent(&mut self, path: &str, row: usize, col: usize) {
        let canon = std::fs::canonicalize(std::path::PathBuf::from(path))
            .unwrap_or_else(|_| std::path::PathBuf::from(path))
            .to_string_lossy()
            .to_string();

        self.positions.retain(|p| p.path != canon);
        self.positions.push(PositionInfo {
            path: canon,
            row,
            col,
        });

        if self.positions.len() > 100 {
            self.positions.remove(0);
        }
    }

    pub fn get_saved_position(&self, path: &str) -> Option<(usize, usize)> {
        let canon = std::fs::canonicalize(std::path::PathBuf::from(path))
            .unwrap_or_else(|_| std::path::PathBuf::from(path))
            .to_string_lossy()
            .to_string();
        self.positions
            .iter()
            .find(|p| p.path == canon)
            .map(|p| (p.row, p.col))
    }

    pub fn save_all_window_positions(&mut self) {
        let mut to_update = Vec::new();
        for win in &self.windows {
            if let Some(buf) = self.buf_by_id(win.buffer_id()) {
                if let Some(ref filename) = buf.filename {
                    to_update.push((filename.clone(), win.row, win.col));
                }
            }
        }
        for (f, row, col) in to_update {
            self.update_position_silent(&f, row, col);
        }
        self.save_positions();
    }

    // ═══════════════════════════════════════════════════════════════════
    // Window ↔ Buffer helpers
    // ═══════════════════════════════════════════════════════════════════

    pub fn switch_window_to_buffer(&mut self, target_bid: usize) {
        // Save current position
        if let Some(ref f) = self.active_filename().map(|s| s.to_string()) {
            self.update_position(f, self.active_window().row, self.active_window().col);
            if let Ok(canon) = std::fs::canonicalize(std::path::PathBuf::from(f)) {
                self.mru_manager
                    .insert(canon, self.active_window().row, self.active_window().col);
            }
        }

        let target_filename = self.buf_by_id(target_bid).and_then(|b| b.filename.clone());
        self.active_window_mut().set_buffer_id(target_bid);

        // Restore saved position or reset
        if let Some(ref p) = target_filename {
            if let Some((row, col)) = self.get_saved_position(p) {
                let (len_lines, line_char_len) = {
                    let buf = self.buf_by_id(target_bid).unwrap();
                    let safe_row = row.min(buf.len_lines().saturating_sub(1));
                    (buf.len_lines(), buf.line_char_len(safe_row))
                };
                let win = self.active_window_mut();
                win.row = row.min(len_lines.saturating_sub(1));
                win.col = col.min(line_char_len);
                self.scroll_active_window_to_cursor();
                return;
            }
        }

        let win = self.active_window_mut();
        win.row = 0;
        win.col = 0;
        win.scroll_line = 0;
        win.scroll_col = 0;
    }

    pub fn close_buffer_by_id(&mut self, bid_removed: usize) {
        if bid_removed == self.active_window().buffer_id() {
            if let Some(ref f) = self.active_filename().map(|s| s.to_string()) {
                self.update_position(f, self.active_window().row, self.active_window().col);
            }
        }

        if self.buffers.len() <= 1 {
            self.set_status_msg("Cannot close the last remaining buffer", MessageKind::Error);
            return;
        }

        let buf_idx = match self.buffers.iter().position(|b| b.id == bid_removed) {
            Some(idx) => idx,
            None => return,
        };

        self.buffers.remove(buf_idx);

        let first_valid_id = self.buffers.first().map(|b| b.id).unwrap_or(0);
        let valid_ids: Vec<usize> = self.buffers.iter().map(|b| b.id).collect();

        let fallback_pos = self
            .buf_by_id(first_valid_id)
            .and_then(|buf| buf.filename.as_ref())
            .and_then(|f| self.get_saved_position(f));

        for win in &mut self.windows {
            if win.buffer_id() == bid_removed {
                win.set_buffer_id(first_valid_id);
                if let Some((row, col)) = fallback_pos {
                    win.row = row;
                    win.col = col;
                } else {
                    win.row = 0;
                    win.col = 0;
                }
                win.scroll_line = 0;
                win.scroll_col = 0;
                win.desired_col = 0;
            } else {
                win.clamp_buffer_id(&valid_ids);
            }
        }

        if self.windows.is_empty() {
            self.should_quit = true;
            return;
        }
        if self.active_window_idx >= self.windows.len() {
            self.active_window_idx = self.windows.len() - 1;
        }

        self.comp.on_leave_insert();
        self.set_status(
            &format!(
                "Closed buffer [{}/{}]",
                self.active_window_idx + 1,
                self.windows.len()
            ),
            MessageKind::Info,
        );
        self.refresh_buffer_words();
    }
}
