use crate::ed::editor::PositionInfo;
use crate::ed::MessageKind;
use crate::event::KeyEvent;
use crate::Config;
use crate::Editor;
use crossterm::event::KeyCode;

impl Editor {
    // ═══════════════════════════════════════════════════════════════════
    // Ripgrep key handler
    // ═══════════════════════════════════════════════════════════════════

    pub fn handle_ripgrep_key(&mut self, key: KeyEvent) -> bool {
        match key.code {
            KeyCode::Enter => {
                self.ripgrep_goto_result();
                true
            }
            KeyCode::Char('q') => {
                self.ripgrep_close_buffer();
                true
            }
            _ => false, // Fall through to inherit normal navigation
        }
    }
    // -- Handle Marks Key Events --
    pub fn handle_marks_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Esc => {
                self.popup.close();
            }
            KeyCode::Up => {
                if let Some(ref mut p) = self.popup.marks {
                    p.move_up();
                }
            }
            KeyCode::Down => {
                if let Some(ref mut p) = self.popup.marks {
                    p.move_down();
                }
            }
            KeyCode::Enter => {
                if let Some(ref p) = self.popup.marks {
                    if let Some(entry) = p.entries.get(p.selected).cloned() {
                        self.popup.close();
                        self.active_window_mut().save_jump_position();

                        // Switch to the mark's buffer if it's not the current one
                        if self.active_window().buffer_id() != entry.buffer_id {
                            self.switch_window_to_buffer(entry.buffer_id);
                        }

                        let buf_len = self.buf().len_lines();
                        let safe_row = entry.row.min(buf_len.saturating_sub(1));
                        let safe_col = entry.col.min(self.buf().line_char_len(safe_row));

                        let win = self.active_window_mut();
                        win.row = safe_row;
                        win.col = safe_col;
                        win.desired_col = safe_col;
                        self.scroll_active_window_to_cursor();
                    }
                }
            }
            KeyCode::Char(c) if c.is_ascii_lowercase() => {
                // Quick jump by pressing the mark letter directly
                if let Some(ref p) = self.popup.marks {
                    // Find the first mark matching the letter
                    if let Some(entry) = p.entries.iter().find(|e| e.ch == c).cloned() {
                        self.popup.close();
                        self.active_window_mut().save_jump_position();

                        // Switch to the mark's buffer if it's not the current one
                        if self.active_window().buffer_id() != entry.buffer_id {
                            self.switch_window_to_buffer(entry.buffer_id);
                        }

                        let buf_len = self.buf().len_lines();
                        let safe_row = entry.row.min(buf_len.saturating_sub(1));
                        let safe_col = entry.col.min(self.buf().line_char_len(safe_row));

                        let win = self.active_window_mut();
                        win.row = safe_row;
                        win.col = safe_col;
                        win.desired_col = safe_col;
                        self.scroll_active_window_to_cursor();
                    }
                }
            }
            _ => {}
        }
    }

    /// Handles keys specifically when the Buffer List popup is focused.
    pub fn handle_buffer_list_key(&mut self, key: KeyEvent) {
        use crossterm::event::KeyCode;

        // Retrieve selected entry upfront to bypass borrow-checker issues
        let selected_entry = self
            .popup
            .buffer_list
            .as_ref()
            .and_then(|p| p.selected_entry().cloned());

        match key.code {
            KeyCode::Esc => {
                // Fix: use close() to fully clear both buffer_list and kind state
                self.popup.close();
            }

            KeyCode::Up => {
                if let Some(ref mut bl) = self.popup.buffer_list {
                    bl.move_up();
                }
            }

            KeyCode::Down => {
                if let Some(ref mut bl) = self.popup.buffer_list {
                    bl.move_down();
                }
            }

            // [d] or [Delete] to close/kill the highlighted buffer
            KeyCode::Char('d') | KeyCode::Delete => {
                if let Some(entry) = selected_entry {
                    self.close_buffer_by_id(entry.id);
                    // Refresh popup list entries to show remaining buffers
                    self.trigger_buffer_list_popup();
                }
            }

            // [Enter] switches to and opens the selected buffer
            KeyCode::Enter => {
                // Fix: use close() to fully clear both buffer_list and kind state
                self.popup.close();
                if let Some(entry) = selected_entry {
                    self.switch_window_to_buffer(entry.id);
                }
            }

            KeyCode::Backspace => {
                if let Some(ref mut bl) = self.popup.buffer_list {
                    bl.filter_pop();
                }
            }

            KeyCode::Char(c) => {
                if let Some(ref mut bl) = self.popup.buffer_list {
                    bl.filter_push(c);
                }
            }

            _ => {}
        }
    }

    pub fn handle_file_picker_key(&mut self, key: KeyEvent) {
        use crossterm::event::KeyCode;

        // Clone the selected entry up front to avoid borrow conflicts.
        let selected_entry = self
            .popup
            .file_picker
            .as_ref()
            .and_then(|p| p.selected_entry().cloned());

        match key.code {
            KeyCode::Esc => {
                let cwd = self.popup.file_picker.as_ref().map(|p| p.cwd.clone());
                self.popup.last_file_picker_dir = cwd;
                self.popup.close();
            }

            KeyCode::Up => {
                if let Some(picker) = &mut self.popup.file_picker {
                    picker.move_up();
                }
            }

            KeyCode::Down => {
                if let Some(picker) = &mut self.popup.file_picker {
                    picker.move_down();
                }
            }

            KeyCode::Enter | KeyCode::Right => {
                if let Some(entry) = selected_entry {
                    if entry.is_dir {
                        let path = entry.path.clone();
                        if let Some(picker) = &mut self.popup.file_picker {
                            picker.go_into(&path);
                        }
                    } else {
                        // Open the file
                        let cwd = self.popup.file_picker.as_ref().map(|p| p.cwd.clone());
                        self.popup.last_file_picker_dir = cwd;
                        self.popup.close();
                        self.open_buffer(Some(entry.path.to_string_lossy().to_string()));
                    }
                }
            }

            KeyCode::Backspace | KeyCode::Left => {
                if let Some(picker) = &mut self.popup.file_picker {
                    if picker.filter.is_empty() {
                        picker.go_up();
                    } else {
                        picker.filter_pop();
                    }
                }
            }

            KeyCode::Tab => {
                if let Some(picker) = &mut self.popup.file_picker {
                    picker.toggle_flat();
                }
            }

            KeyCode::Char(c) => {
                if let Some(picker) = &mut self.popup.file_picker {
                    picker.filter_push(c);
                }
            }

            _ => {}
        }
    }

    pub fn handle_function_list_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Esc => {
                self.popup.function_list = None;
            }
            KeyCode::Up => {
                if let Some(ref mut p) = self.popup.function_list {
                    p.move_up();
                }
            }
            KeyCode::Down => {
                if let Some(ref mut p) = self.popup.function_list {
                    p.move_down();
                }
            }
            KeyCode::Enter => {
                // Get the line index of the selected function
                let target_line = if let Some(ref p) = self.popup.function_list {
                    p.selected_entry().map(|e| e.line)
                } else {
                    None
                };

                // Close the popup
                self.popup.function_list = None;

                if let Some(line_idx) = target_line {
                    let (win, buf) = self.active_window_and_buf_mut();
                    win.row = line_idx.min(buf.len_lines().saturating_sub(1));
                    win.col = 0;
                    self.scroll_active_window_to_cursor();
                    self.set_status_msg(
                        &format!("Jumped to line {}", line_idx + 1),
                        MessageKind::Info,
                    );
                }
            }
            KeyCode::Backspace => {
                if let Some(ref mut p) = self.popup.function_list {
                    p.filter_pop();
                }
            }
            KeyCode::Char(c) => {
                if let Some(ref mut p) = self.popup.function_list {
                    p.filter_push(c);
                }
            }
            _ => {}
        }
    }
}

impl Editor {
    /// Load position info from positions.json
    pub fn load_positions() -> Vec<PositionInfo> {
        if let Ok(dir) = Config::config_dir() {
            let path = dir.join("positions.json");
            if let Ok(content) = std::fs::read_to_string(path) {
                if let Ok(pos) = serde_json::from_str::<Vec<PositionInfo>>(&content) {
                    return pos;
                }
            }
        }
        Vec::new()
    }

    /// Save positions to positions.json, limited to 100 entries and deduplicated
    pub fn save_positions(&self) {
        if let Ok(dir) = Config::config_dir() {
            let path = dir.join("positions.json");
            let len = self.positions.len();
            let start = len.saturating_sub(100); // Enforce a hard cap of 100 entries on quit
            if let Ok(serialized) = serde_json::to_string_pretty(&self.positions[start..]) {
                let _ = std::fs::write(path, serialized);
            }
        }
    }

    /// Record or update the position of a file and write to disk
    pub fn update_position(&mut self, path: &str, row: usize, col: usize) {
        self.update_position_silent(path, row, col);
        self.save_positions();
    }

    /// Update position database in memory (deduping the target file path)
    pub fn update_position_silent(&mut self, path: &str, row: usize, col: usize) {
        let path_buf = std::path::PathBuf::from(path);
        let canon_path = std::fs::canonicalize(&path_buf)
            .unwrap_or(path_buf)
            .to_string_lossy()
            .to_string();

        // Dedup: remove previous coordinates for this file
        self.positions.retain(|p| p.path != canon_path);

        // Push the fresh position to the end (most recently accessed)
        self.positions.push(PositionInfo {
            path: canon_path,
            row,
            col,
        });

        // Maintain hard 100-entry ceiling
        if self.positions.len() > 100 {
            self.positions.remove(0);
        }
    }

    /// Retrieve the saved position for a file
    pub fn get_saved_position(&self, path: &str) -> Option<(usize, usize)> {
        let path_buf = std::path::PathBuf::from(path);
        let canon_path = std::fs::canonicalize(&path_buf)
            .unwrap_or(path_buf)
            .to_string_lossy()
            .to_string();

        self.positions
            .iter()
            .find(|p| p.path == canon_path)
            .map(|p| (p.row, p.col))
    }

    /// Collect active filenames and positions for all windows and save them
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

    /// Unified transition helper to save the old position and restore the new position
    pub fn switch_window_to_buffer(&mut self, target_bid: usize) {
        // Save current position of the active buffer
        let old_filename = self.active_filename().map(|s| s.to_string());
        let old_row = self.active_window().row;
        let old_col = self.active_window().col;
        if let Some(ref f) = old_filename {
            self.update_position(f, old_row, old_col);
            let path_buf = std::path::PathBuf::from(f);
            if let Ok(canon) = std::fs::canonicalize(&path_buf) {
                self.mru_manager.insert(canon, old_row, old_col);
            }
        }

        // Get target buffer's filename for position lookup BEFORE mutating
        let target_filename = self.buf_by_id(target_bid).and_then(|b| b.filename.clone());

        // 1. Switch the active window's target buffer first
        self.active_window_mut().set_buffer_id(target_bid);

        // 2. Restore saved position
        let mut restored = false;
        if let Some(ref p) = target_filename {
            if let Some((row, col)) = self.get_saved_position(p) {
                // Extract buffer data first to avoid simultaneous mutable+immutable borrows
                let (len_lines, line_char_len) = {
                    let buf = self.buf_by_id(target_bid).unwrap();
                    let safe_row = row.min(buf.len_lines().saturating_sub(1));
                    (buf.len_lines(), buf.line_char_len(safe_row))
                };
                let win = self.active_window_mut();
                win.row = row.min(len_lines.saturating_sub(1));
                win.col = col.min(line_char_len);
                self.scroll_active_window_to_cursor();
                restored = true;
            }
        }

        if !restored {
            let win = self.active_window_mut();
            win.row = 0;
            win.col = 0;
            win.scroll_line = 0;
            win.scroll_col = 0;
        }
    }

    /// Close any specific buffer by its ID and safely clamp remaining viewports.
    pub fn close_buffer_by_id(&mut self, bid_removed: usize) {
        // ── 1. ALWAYS save active buffer position first (even if the close is blocked) ──
        if bid_removed == self.active_window().buffer_id() {
            let old_filename = self.active_filename().map(|s| s.to_string());
            let old_row = self.active_window().row;
            let old_col = self.active_window().col;
            if let Some(ref f) = old_filename {
                self.update_position(f, old_row, old_col);
            }
        }

        if self.buffers.len() <= 1 {
            // Prevent deletion and display an error warning
            self.set_status_msg("Cannot close the last remaining buffer", MessageKind::Error);
            return;
        }

        let buf_idx = match self.buffers.iter().position(|b| b.id == bid_removed) {
            Some(idx) => idx,
            None => return,
        };

        // If we are closing the active buffer, save its position first
        if bid_removed == self.active_window().buffer_id() {
            let old_filename = self.active_filename().map(|s| s.to_string());
            let old_row = self.active_window().row;
            let old_col = self.active_window().col;
            if let Some(ref f) = old_filename {
                self.update_position(f, old_row, old_col);
            }
        }

        self.buffers.remove(buf_idx);

        // Pick fallback defaults
        let first_valid_id = self.buffers.first().map(|b| b.id).unwrap_or(0);
        let valid_ids: Vec<usize> = self.buffers.iter().map(|b| b.id).collect();

        let fallback_pos = if let Some(buf) = self.buf_by_id(first_valid_id) {
            if let Some(ref f) = buf.filename {
                self.get_saved_position(f)
            } else {
                None
            }
        } else {
            None
        };

        for win in &mut self.windows {
            let old_bid = win.buffer_id();
            if old_bid == bid_removed {
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

// ---------------------------------------------------------------------------
// Command history
// ---------------------------------------------------------------------------

impl Editor {
    pub fn load_history() -> Vec<String> {
        if let Ok(dir) = Config::config_dir() {
            let path = dir.join("history.txt");
            if let Ok(content) = std::fs::read_to_string(path) {
                return content
                    .lines()
                    .map(|s| s.to_string())
                    .filter(|s| !s.is_empty())
                    .collect();
            }
        }
        Vec::new()
    }

    pub fn save_history(history: &[String]) {
        if let Ok(dir) = Config::config_dir() {
            let start = history.len().saturating_sub(500);
            let _ = std::fs::write(dir.join("history.txt"), history[start..].join("\n"));
        }
    }

    pub fn append_and_save_history(&mut self, cmd: &str) {
        let trimmed = cmd.trim();
        if trimmed.is_empty() {
            return;
        }
        if self.cmd_history.last().map(|s| s.as_str()) != Some(trimmed) {
            self.cmd_history.push(trimmed.to_string());
            Self::save_history(&self.cmd_history);
        }
    }

    pub fn history_prev(&mut self) {
        if self.cmd_history.is_empty() {
            return;
        }

        // Initialize the prefix search on the first press of the Up arrow
        if self.cmd_history_idx.is_none() {
            self.cmd_temp_input = self.command.clone();
            self.history_search_prefix = Some(self.command.clone());
        }

        let prefix = self.history_search_prefix.as_deref().unwrap_or("");
        let start_idx = match self.cmd_history_idx {
            None => self.cmd_history.len().saturating_sub(1),
            Some(idx) => idx.saturating_sub(1),
        };

        // Scan backward through history for a matching prefix
        let mut found_idx = None;
        for i in (0..=start_idx).rev() {
            if self.cmd_history[i].starts_with(prefix) {
                found_idx = Some(i);
                break;
            }
        }

        if let Some(idx) = found_idx {
            self.cmd_history_idx = Some(idx);
            self.command = self.cmd_history[idx].clone();
        }
    }

    pub fn history_next(&mut self) {
        if self.cmd_history.is_empty() || self.cmd_history_idx.is_none() {
            return;
        }

        let prefix = self.history_search_prefix.as_deref().unwrap_or("");
        let start_idx = self.cmd_history_idx.unwrap().saturating_add(1);

        // Scan forward through history for a matching prefix
        let mut found_idx = None;
        for i in start_idx..self.cmd_history.len() {
            if self.cmd_history[i].starts_with(prefix) {
                found_idx = Some(i);
                break;
            }
        }

        if let Some(idx) = found_idx {
            self.cmd_history_idx = Some(idx);
            self.command = self.cmd_history[idx].clone();
        } else {
            // Reached the end of matching history; restore original typed text
            self.cmd_history_idx = None;
            self.command = self.cmd_temp_input.clone();
        }
    }
}
