use crate::ed::buffer::BufferKind;
use crate::ed::editor::PendingInput;
use crate::ed::misc_helper::get_head_file_content;
use crate::ed::window::LayoutNode;
use crate::ed::window::SplitDir;
use crate::ed::window::Window;
use crate::ed::window::WindowPosition;
use crate::ed::Buffer;
use crate::ed::MessageKind;
use crate::ed::Mode;
use crate::render::helpers::digit_count;
use crate::Editor;
use anyhow::Result;

const MAX_WINDOWS: usize = 8;

impl Editor {
    // ── Set bookmark ────────────────────────────────────────────

    /// Set (or overwrite) named bookmark `ch` at the current cursor.
    pub fn set_named_bookmark(&mut self, ch: char) {
        let (row, col) = {
            let win = self.active_window();
            (win.row, win.col)
        };

        let buf = self.buf_mut();
        buf.named_bookmarks.insert(ch, (row, col));
        buf.sync_bookmark_rows();

        self.set_status_msg(
            &format!("Mark '{}' set at line {}", ch, row + 1),
            MessageKind::Success,
        );
    }

    // ── Goto bookmark ───────────────────────────────────────────

    /// Jump to named bookmark `ch`.  Saves current position for
    /// ping-pong before jumping.
    pub fn goto_named_bookmark(&mut self, ch: char) {
        let target = self.buf().named_bookmarks.get(&ch).copied();

        match target {
            Some((row, col)) => {
                // Save current position for ping-pong *before* jumping
                self.active_window_mut().save_jump_position();

                let buf_len = self.buf().len_lines();
                let safe_row = row.min(buf_len.saturating_sub(1));
                let safe_col = col.min(self.buf().line_char_len(safe_row));

                let win = self.active_window_mut();
                win.row = safe_row;
                win.col = safe_col;
                win.desired_col = safe_col;

                self.scroll_active_window_to_cursor();
                self.set_status_msg(
                    &format!("Jumped to mark '{}' (line {})", ch, safe_row + 1),
                    MessageKind::Info,
                );
            }
            None => {
                self.set_status_msg(&format!("Mark '{}' not set", ch), MessageKind::Error);
            }
        }
    }

    // ── Ping-pong jump (``) ────────────────────────────────────

    /// Swap cursor with `last_jump`.  Pressing `` twice returns to
    /// the original position.
    pub fn jump_last_position(&mut self) {
        let prev = self.active_window().last_jump;

        match prev {
            Some((row, col)) => {
                // Save *current* position (becomes the new ping-pong target)
                self.active_window_mut().save_jump_position();
                // Overwrite with where we *were* (the save above captured
                // current, now set cursor to previous)
                let buf_len = self.buf().len_lines();
                let safe_row = row.min(buf_len.saturating_sub(1));
                let safe_col = col.min(self.buf().line_char_len(safe_row));

                let win = self.active_window_mut();
                win.row = safe_row;
                win.col = safe_col;
                win.desired_col = safe_col;

                self.scroll_active_window_to_cursor();
                self.set_status_msg(
                    &format!("Jumped back to line {}", safe_row + 1),
                    MessageKind::Info,
                );
            }
            None => {
                self.set_status_msg("No previous jump position", MessageKind::Info);
            }
        }
    }

    // ── Delete named bookmark ───────────────────────────────────

    /// Remove a named bookmark.  Returns true if it existed.
    pub fn delete_named_bookmark(&mut self, ch: char) -> bool {
        let removed = self.buf_mut().named_bookmarks.remove(&ch).is_some();
        if removed {
            self.buf_mut().sync_bookmark_rows();
            self.set_status_msg(&format!("Mark '{}' removed", ch), MessageKind::Info);
        }
        removed
    }

    /// Synchronize scrolling and cursor positions for side-by-side linked diff windows.
    // In editor.rs — replace sync_diff_windows entirely

    pub fn sync_diff_windows(&mut self) {
        let active_win_idx = self.active_window_idx;
        let active_win = &self.windows[active_win_idx];
        let sibling_id = active_win.diff_sibling;

        let scroll_line = active_win.scroll_line;
        let scroll_col = active_win.scroll_col;
        let cursor_row = active_win.row;

        let Some(sib_id) = sibling_id else { return };
        let Some(sib_idx) = self.windows.iter().position(|w| w.id == sib_id) else {
            return;
        };
        if sib_idx == active_win_idx {
            return;
        }

        // 1. Sync scroll unconditionally
        self.windows[sib_idx].scroll_line = scroll_line;
        self.windows[sib_idx].scroll_col = scroll_col;

        let sibling_bid = self.windows[sib_idx].buffer_id();
        let active_bid = self.windows[active_win_idx].buffer_id();

        // 2. Use the alignment map to translate the active virtual row
        //    into the sibling's real rope row.
        //
        //    The alignment maps are stored on BOTH buffers identically.
        //    If the active window is on the working-copy side (right),
        //    we look up cursor_row in align.right to find the real rope
        //    row, then find the matching position in align.left for the
        //    sibling, and vice-versa.
        //
        //    Fallback: if no alignment exists, mirror the row directly.

        let sibling_real_row: usize = {
            // Try to get the alignment from either buffer (they're identical)
            let alignment = self
                .buf_by_id(active_bid)
                .and_then(|b| b.diff_alignment.as_ref())
                .or_else(|| {
                    self.buf_by_id(sibling_bid)
                        .and_then(|b| b.diff_alignment.as_ref())
                });

            if let Some(align) = alignment {
                let active_is_head = self
                    .buf_by_id(active_bid)
                    .and_then(|b| b.filename.as_deref())
                    .map(|f| f.starts_with("git://head/"))
                    .unwrap_or(false);

                // active_map: the side the active window is on
                // sibling_map: the side we want to sync TO
                let (active_map, sibling_map) = if active_is_head {
                    (&align.left, &align.right)
                } else {
                    (&align.right, &align.left)
                };

                // Walk virtual rows to find one whose active-side real row
                // is >= cursor_row, then return the sibling real row there.
                // This keeps both cursors visually aligned.
                let mut best_sibling_row = 0usize;
                let mut found = false;

                for vrow in 0..active_map.len() {
                    use crate::ed::buffer::VirtualLine;
                    if let Some(VirtualLine::Real(r)) = active_map.get(vrow) {
                        if *r >= cursor_row {
                            // Find the nearest real row on the sibling side
                            // at or after this virtual row
                            for sv in vrow..sibling_map.len() {
                                if let Some(VirtualLine::Real(sr)) = sibling_map.get(sv) {
                                    best_sibling_row = *sr;
                                    found = true;
                                    break;
                                }
                            }
                            if !found {
                                // cursor is past last real row on sibling — clamp to last
                                for sv in (0..vrow).rev() {
                                    if let Some(VirtualLine::Real(sr)) = sibling_map.get(sv) {
                                        best_sibling_row = *sr;
                                        found = true;
                                        break;
                                    }
                                }
                            }
                            break;
                        }
                    }
                }
                best_sibling_row
            } else {
                // No alignment — mirror directly, clamped to rope length
                let max = self
                    .buf_by_id(sibling_bid)
                    .map(|b| b.len_lines().saturating_sub(1))
                    .unwrap_or(0);
                cursor_row.min(max)
            }
        };

        // 3. Clamp to the sibling rope's actual line count and apply
        let (max_row, line_char_len) = self
            .buf_by_id(sibling_bid)
            .map(|b| {
                let max = b.len_lines().saturating_sub(1);
                let safe = sibling_real_row.min(max);
                (max, b.line_char_len(safe))
            })
            .unwrap_or((0, 0));

        let clamped_row = sibling_real_row.min(max_row);
        self.windows[sib_idx].row = clamped_row;
        self.windows[sib_idx].col = self.windows[sib_idx].col.min(line_char_len);
    }

    /// Setup side-by-side diff comparing the active file buffer against its HEAD revision.
    pub fn open_diffthis(&mut self) {
        let filename = match self.buf().filename.clone() {
            Some(f) => f,
            None => {
                self.set_status_msg("No file associated with buffer", MessageKind::Error);
                return;
            }
        };

        // 1. Discover Git repository
        let path = std::path::Path::new(&filename);
        let repo = match git2::Repository::discover(path) {
            Ok(r) => r,
            Err(_) => {
                self.set_status_msg("Not in a git repository", MessageKind::Error);
                return;
            }
        };

        let workdir = match repo.workdir() {
            Some(wd) => wd,
            None => {
                self.set_status_msg("Bare repository", MessageKind::Error);
                return;
            }
        };

        let canon = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
        let rel_path = match canon.strip_prefix(workdir) {
            Ok(rp) => rp.to_string_lossy().to_string(),
            Err(_) => path.to_string_lossy().to_string(),
        };

        // 2. Fetch HEAD content from Git ODB (reusing existing helper)
        let head_content = match get_head_file_content(&repo, &rel_path) {
            Some(c) => c,
            None => {
                // Brand new/untracked file: HEAD version is empty
                String::new()
            }
        };

        // Create a unique URI-style filename for the HEAD version
        let target_filename = format!("git://head/{}", rel_path);

        // 3. Create or update the HEAD buffer
        let existing_id = self
            .buffers
            .iter()
            .find(|buf| buf.filename.as_deref() == Some(&target_filename))
            .map(|buf| buf.id);

        let head_bid = if let Some(id) = existing_id {
            if let Some(buf) = self.buf_mut_by_id(id) {
                buf.rope = ropey::Rope::from_str(&head_content);
                buf.parse_syntax();
            }
            id
        } else {
            let id = self.next_buf_id;
            self.next_buf_id += 1;

            let buf = Buffer {
                id,
                rope: ropey::Rope::from_str(&head_content),
                filename: Some(target_filename.clone()),
                modified: false,
                undo_stack: Vec::new(),
                syntax: crate::ed::syntax::SyntaxState::new(),
                bookmarks: std::collections::HashSet::new(),
                git_diffs: std::collections::HashMap::new(),
                named_bookmarks: std::collections::HashMap::new(),
                kind: BufferKind::GitDiffHead,
                git_log_state: None,
                git_status_state: None,
                ripgrep_results: Vec::new(),
                ripgrep_line_map: Vec::new(),
                search_pattern: None,
                diff_alignment: None,
            };

            self.buffers.push(buf);
            self.buffers.last_mut().unwrap().parse_syntax(); // Inherits tree-sitter highlighting
            id
        };

        // 4. Split the active window vertically
        let original_win_id = self.active_window().id;
        let (original_row, original_col, original_scroll_line) = {
            let w = self.active_window();
            (w.row, w.col, w.scroll_line)
        };

        self.split_vertical(); // Focuses the new split window

        let new_win_id = self.active_window().id;

        // 5. Point the new viewport to our HEAD buffer
        self.active_window_mut().set_buffer_id(head_bid);

        // Initialize matching cursor and scroll position
        {
            let win = self.active_window_mut();
            win.row = original_row;
            win.col = original_col;
            win.scroll_line = original_scroll_line;
        }

        // 6. Link both windows together as diff siblings
        if let Some(original_win) = self.windows.iter_mut().find(|w| w.id == original_win_id) {
            original_win.diff_sibling = Some(new_win_id);
        }
        if let Some(new_win) = self.windows.iter_mut().find(|w| w.id == new_win_id) {
            new_win.diff_sibling = Some(original_win_id);
        }

        // --- Build alignment ---
        let old_line_count = {
            let buf = self.buf_by_id(head_bid).unwrap();
            buf.len_lines()
        };
        let new_line_count = {
            // The "active" buffer is the original working-copy side
            let working_bid = self
                .windows
                .iter()
                .find(|w| w.id == original_win_id)
                .map(|w| w.buffer_id())
                .unwrap_or(0);
            self.buf_by_id(working_bid)
                .map(|b| b.len_lines())
                .unwrap_or(0)
        };

        if let Ok(patch) = git2::Patch::from_buffers(
            head_content.as_bytes(),
            Some(std::path::Path::new(&rel_path)),
            self.buf_by_id(
                self.windows
                    .iter()
                    .find(|w| w.id == original_win_id)
                    .unwrap()
                    .buffer_id(),
            )
            .unwrap()
            .rope
            .to_string()
            .as_bytes(),
            Some(std::path::Path::new(&rel_path)),
            None,
        ) {
            let alignment = crate::ed::diff_align::DiffAlignment::from_patch(
                &patch,
                old_line_count,
                new_line_count,
            );
            // Store the SAME struct on both buffers
            // (clone since each buffer owns its copy)
            let working_bid = self
                .windows
                .iter()
                .find(|w| w.id == original_win_id)
                .unwrap()
                .buffer_id();

            if let Some(buf) = self.buf_mut_by_id(head_bid) {
                buf.diff_alignment = Some(alignment.clone()); // needs #[derive(Clone)]
            }
            if let Some(buf) = self.buf_mut_by_id(working_bid) {
                buf.diff_alignment = Some(alignment);
            }
        }

        // Return focus to the original active window so user can edit right away
        if let Some(idx) = self.windows.iter().position(|w| w.id == original_win_id) {
            self.active_window_idx = idx;
        }

        self.set_status_msg(
            &format!("Comparing against HEAD version ({})", rel_path),
            MessageKind::Info,
        );
    }
}

// ---------------------------------------------------------------------------
// Read-only accessors  (render / main)
// ---------------------------------------------------------------------------

impl Editor {
    pub fn mode(&self) -> Mode {
        self.mode
    }
    pub fn should_quit(&self) -> bool {
        self.should_quit
    }
    pub fn command(&self) -> &str {
        &self.command
    }
    pub fn ghost_text(&self) -> Option<&str> {
        self.comp.ghost_text()
    }
    pub fn completions(&self) -> &[String] {
        self.comp.completions()
    }
    pub fn completion_idx(&self) -> usize {
        self.comp.completion_idx()
    }
    pub fn status_msg(&self) -> &str {
        &self.status_msg
    }
    pub fn status_kind(&self) -> MessageKind {
        self.status_kind
    }
    pub fn lsp_loading(&self) -> bool {
        self.lsp_loading
    }
    pub fn spinner_frame(&self) -> usize {
        self.spinner_frame
    }

    pub fn active_row(&self) -> usize {
        self.active_window().row
    }
    pub fn active_col(&self) -> usize {
        self.active_window().col
    }
    pub fn active_scroll(&self) -> usize {
        self.active_window().scroll_line
    }
    pub fn active_modified(&self) -> bool {
        self.buf().modified
    }
    pub fn active_filename(&self) -> Option<&str> {
        self.buf().filename.as_deref()
    }

    pub fn line_count(&self) -> usize {
        self.buf().len_lines()
    }
    pub fn line_text(&self, idx: usize) -> String {
        self.buf().line_text(idx)
    }

    pub fn buffer_count(&self) -> usize {
        self.buffers.len()
    }
    pub fn window_count(&self) -> usize {
        self.windows.len()
    }
    pub fn active_idx(&self) -> usize {
        self.active_window_idx
    }

    pub fn buffer_tabs(&self) -> Vec<(String, bool, bool)> {
        let active_bid = self.active_window().buffer_id();
        self.buffers
            .iter()
            .enumerate()
            .map(|(_, b)| {
                let is_active = b.id == active_bid;
                (b.display_name(), b.modified, is_active)
            })
            .collect()
    }

    pub fn command_ghost_text(&self) -> Option<String> {
        if self.mode != Mode::Command {
            return None;
        }
        let full = self.comp.ghost_text()?;
        if full.starts_with(&self.command) {
            Some(full[self.command.len()..].to_string())
        } else {
            None
        }
    }

    pub fn get_current_line_text(&self) -> String {
        self.buf().line_text(self.active_window().row)
    }

    pub fn get_current_word_prefix(&self) -> String {
        let win = self.active_window();
        let buf = self.buf();
        let chars: Vec<char> = buf.line_text(win.row).chars().collect();
        let col = win.col.min(chars.len());
        let mut start = col;
        while start > 0 && (chars[start - 1].is_alphanumeric() || chars[start - 1] == '_') {
            start -= 1;
        }
        chars[start..col].iter().collect()
    }

    /// Returns `true` if any window *other than* the active one is
    /// viewing the same buffer.
    pub fn is_buffer_viewed_elsewhere(&self) -> bool {
        let active_id = self.active_window().id;
        let bid = self.active_window().buffer_id();
        self.windows
            .iter()
            .any(|w| w.id != active_id && w.buffer_id() == bid)
    }

    /// Return a reference to all windows (for rendering).
    pub fn all_windows(&self) -> &[Window] {
        &self.windows
    }

    /// Return the index of the currently focused window.
    pub fn active_window_index(&self) -> usize {
        self.active_window_idx
    }
}

// ---------------------------------------------------------------------------
// Layout computation (called each frame by the render loop)
// ---------------------------------------------------------------------------

impl Editor {
    /// Compute and assign `WindowPosition` to every window based on
    /// the layout tree and the available terminal `area`.
    ///
    /// `separator` is the number of rows/cols taken by a divider line
    /// between split panes (typically 1).
    pub fn layout_windows(&mut self, area: WindowPosition, separator: usize) {
        let positions = self.layout.compute_positions(area, separator);
        for (window_id, pos) in positions {
            if let Some(win) = self.windows.iter_mut().find(|w| w.id == window_id) {
                win.set_position(pos);
            }
        }
    }

    /// Convenience: layout with a 1-row/col separator.
    pub fn layout_windows_default(&mut self, area: WindowPosition) {
        self.layout_windows(area, 1);
        if self.needs_initial_scroll {
            self.scroll_active_window_to_cursor();
            self.needs_initial_scroll = false;
        }
    }
}

// ---------------------------------------------------------------------------
// Status helpers
// ---------------------------------------------------------------------------

impl Editor {
    pub fn set_status(&mut self, msg: &str, kind: MessageKind) {
        self.status_msg = msg.to_string();
        self.status_kind = kind;
        self.status_time = std::time::Instant::now();
    }

    pub fn set_status_msg(&mut self, msg: &str, kind: MessageKind) {
        self.set_status(msg, kind);
    }

    pub fn clear_status_msg(&mut self) {
        self.status_msg.clear();
    }
}

// ---------------------------------------------------------------------------
// Mode transitions
// ---------------------------------------------------------------------------

impl Editor {
    pub fn enter_insert(&mut self) {
        self.mode = Mode::Insert;
        self.comp.on_enter_insert();
        self.cmd_history_idx = None;
        self.pending_input = PendingInput::None;
        self.clear_pending_keys();
    }

    pub fn enter_normal(&mut self) {
        if self.mode == Mode::Brief {
            self.set_status_msg(
                "Normal mode — gi or :brief to switch back",
                MessageKind::Info,
            );
        }
        self.mode = Mode::Normal;
        self.active_window_mut().visual_anchor = None; // Clear the selection anchor
        self.comp.on_leave_insert();
        self.command.clear();
        self.cmd_history_idx = None;
        self.clear_pending_keys();
    }

    pub fn enter_command(&mut self) {
        self.prev_mode = self.mode;
        self.mode = Mode::Command;
        self.command.clear();
        self.command_cursor = 0;
        self.comp.on_leave_insert();
        self.cmd_history_idx = None;
        self.pending_input = PendingInput::None;
        self.clear_pending_keys();
    }

    pub fn enter_brief(&mut self) {
        self.mode = Mode::Brief;
        self.comp.on_enter_insert();
        self.cmd_history_idx = None;
        self.clear_pending_keys();
        self.pending_input = PendingInput::None;
        self.set_status_msg("Brief mode — F9 :vim to switch back", MessageKind::Info);
    }

    pub fn set_mode(&mut self, mode: Mode) {
        match mode {
            Mode::Insert => self.enter_insert(),
            Mode::Normal => self.enter_normal(),
            Mode::Command => self.enter_command(),
            Mode::Brief => self.enter_brief(),
            Mode::Visual => {
                self.mode = Mode::Visual;
                let cur = (self.active_window().row, self.active_window().col);
                self.active_window_mut().visual_anchor = Some(cur);
            }
            Mode::VisualLine => {
                self.mode = Mode::VisualLine;
                let row = self.active_window().row;
                self.active_window_mut().visual_anchor = Some((row, 0));
            }
            Mode::VisualBlock => {
                self.prev_mode = self.mode;
                self.mode = Mode::VisualBlock;
                let cur = (self.active_window().row, self.active_window().col);
                self.active_window_mut().visual_anchor = Some(cur);
            }
            Mode::Search => {
                self.prev_mode = self.mode;
                self.mode = Mode::Search;
                self.command.clear();
                self.command_cursor = 0;
                self.comp.on_leave_insert();
                self.cmd_history_idx = None;
                self.clear_pending_keys();
            }
            Mode::LlmPrompt => {
                self.prev_mode = self.mode;
                self.mode = Mode::LlmPrompt;
                self.llm.prompt.clear();
                self.comp.on_leave_insert();
                self.cmd_history_idx = None;
                self.clear_pending_keys();
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Completion
// ---------------------------------------------------------------------------

impl Editor {
    pub fn ingest_completion_response(&mut self, id: usize, items: Vec<String>) {
        let (row, col) = {
            let win = self.active_window();
            (win.row, win.col)
        };
        let mode = self.mode();
        // Extract what context_allows needs without holding a borrow into self
        let rope_len = self.buf().rope.len_chars();
        let line_chars: Vec<char> = self.buf().line_text(row).chars().collect();
        let filename = self.buf().filename.clone();

        // Re-implement the context check inline to avoid passing &Buffer
        // while self.comp is mutably borrowed
        let context_ok = {
            if rope_len <= 1 {
                false
            } else {
                let c = col.min(line_chars.len());
                if c < line_chars.len() {
                    let next = line_chars[c];
                    if next.is_alphanumeric() || next == '_' || next == ')' {
                        false
                    } else if c > 0 && line_chars[c - 1] == ')' {
                        false
                    } else {
                        let min_prefix = 4;
                        let mut prefix_len = 0;
                        let mut i = c;
                        while i > 0 {
                            let ch = line_chars[i - 1];
                            if ch.is_alphanumeric() || ch == '_' {
                                prefix_len += 1;
                                i -= 1;
                            } else {
                                break;
                            }
                        }
                        prefix_len >= min_prefix
                    }
                } else if c > 0 && line_chars[c - 1] == ')' {
                    false
                } else {
                    let min_prefix = 4;
                    let mut prefix_len = 0;
                    let mut i = c;
                    while i > 0 {
                        let ch = line_chars[i - 1];
                        if ch.is_alphanumeric() || ch == '_' {
                            prefix_len += 1;
                            i -= 1;
                        } else {
                            break;
                        }
                    }
                    prefix_len >= min_prefix
                }
            }
        };

        if id != self.comp.request_id {
            return;
        }
        if !context_ok {
            self.comp.reset_to_idle();
            return;
        }
        if items.is_empty() {
            self.comp.reset_to_idle();
            return;
        }
        self.comp.set_active(items);
    }

    pub fn on_edit(&mut self) {
        self.comp.on_edit();
    }

    pub fn cycle_completion(&mut self, dir: i32) {
        self.comp.cycle(dir);
    }

    pub fn clear_completions(&mut self) {
        self.comp.on_edit();
    }

    pub fn cancel_pending_request(&mut self) {
        let id = self.comp.request_id;
        self.comp.on_cancel(id);
    }

    pub fn touch_completion(&mut self) {
        self.comp.last_edit_time = std::time::Instant::now();
    }

    // set_completions — extract buf data before calling comp:
    pub fn set_completions(&mut self, items: Vec<String>) {
        let id = self.comp.request_id;
        let (row, col) = {
            let win = self.active_window();
            (win.row, win.col)
        };
        let mode = self.mode();
        let rope_len = self.buf().rope.len_chars();
        let line_chars: Vec<char> = self.buf().line_text(row).chars().collect();
        // drop buf borrow here, then call on_response with owned data
        // but since set_completions bypasses context check, just use set_active:
        self.comp.set_active(items);
    }

    /// Poll the completion machine on every tick.
    pub fn poll_completion(&mut self) -> Option<(usize, String, usize, String)> {
        let (row, col, rope_str, offset, filename) = {
            let win = self.active_window();
            let buf = self.buf();

            if win.row >= buf.len_lines() {
                return None;
            }

            (
                win.row,
                win.col,
                buf.rope.to_string(),
                win.cursor_char_offset(buf),
                buf.filename.clone(),
            )
        };
        let mode = self.mode;

        let comp = &mut self.comp;
        use crate::ed::buffer::detect_language;

        if mode != Mode::Insert && mode != Mode::Brief {
            return None;
        }
        if !comp.is_throttling() {
            return None;
        }
        if comp.last_edit_time.elapsed() < std::time::Duration::from_millis(comp.throttle_ms) {
            return None;
        }

        if rope_str.chars().count() <= 1 {
            return None;
        }
        let line: Vec<char> = rope_str.lines().nth(row).unwrap_or("").chars().collect();
        let c = col.min(line.len());
        if c < line.len() {
            let next = line[c];
            if next.is_alphanumeric() || next == '_' || next == ')' {
                return None;
            }
        }
        if c > 0 && line[c - 1] == ')' {
            return None;
        }

        comp.start_pending(row, col);

        Some((
            comp.request_id,
            rope_str,
            offset,
            detect_language(filename.as_deref()),
        ))
    }

    /// Accept the current ghost text and insert it.
    pub fn accept_completion(&mut self) {
        let (row, col, before_str, after, line_to_char_row) = {
            let win = self.active_window();
            let buf = self.buf();
            if win.row >= buf.len_lines() {
                self.comp.reset_to_idle();
                return;
            }
            let line_text = buf.line_text(win.row);
            let before_str: String = line_text.chars().take(win.col).collect();
            let after: String = line_text.chars().skip(win.col).collect();
            (
                win.row,
                win.col,
                before_str,
                after,
                buf.rope.line_to_char(win.row),
            )
        };

        let ghost = match self.comp.ghost_text.take() {
            Some(t) => t,
            None => return,
        };

        // Dynamically compute the prefix overlap with the full ghost word relative to what is currently typed
        let prefix_overlap = crate::comp::state::find_prefix_overlap(&before_str, &ghost);
        let ghost_suffix: String = ghost.chars().skip(prefix_overlap).collect();

        // Calculate overlap with the existing text ahead of the cursor to prevent duplicates
        let overlap = after
            .chars()
            .zip(ghost_suffix.chars())
            .take_while(|(a, b)| a == b)
            .count();

        let to_insert: String = ghost_suffix.chars().skip(overlap).collect();
        let insert_offset = line_to_char_row + col;

        self.comp.reset_to_idle();

        {
            let buf = self.buf_mut();
            buf.push_undo(row, col);
            if !to_insert.is_empty() {
                buf.rope.insert(insert_offset, &to_insert);
            }
            buf.modified = true;
            buf.parse_syntax();
        }
        {
            let win = self.active_window_mut();
            win.col += to_insert.chars().count() + overlap;
        }

        self.refresh_buffer_words();
    }

    /// Locates the word under (or immediately after) the active window's cursor position on the line.
    /// Matches standard Vim behavior by scanning forward for a word boundary first.
    pub fn get_word_under_cursor(&self) -> Option<String> {
        let win = self.active_window();
        let buf = self.buf();

        if win.row >= buf.len_lines() {
            return None;
        }

        let line_text = buf.line_text(win.row);
        let chars: Vec<char> = line_text.chars().collect();
        if chars.is_empty() {
            return None;
        }

        let col = win.col.min(chars.len().saturating_sub(1));
        let mut start = col;

        // 1. If starting on a non-word character, scan forward to find the next word
        while start < chars.len() && !chars[start].is_alphanumeric() && chars[start] != '_' {
            start += 1;
        }

        if start >= chars.len() {
            return None; // No word found forward on this line
        }

        let mut end = start;

        // 2. Scan backward to locate the word start boundary
        while start > 0 && (chars[start - 1].is_alphanumeric() || chars[start - 1] == '_') {
            start -= 1;
        }

        // 3. Scan forward to locate the word end boundary
        while end < chars.len() && (chars[end].is_alphanumeric() || chars[end] == '_') {
            end += 1;
        }

        if start < end {
            Some(chars[start..end].iter().collect())
        } else {
            None
        }
    }
}

// ---------------------------------------------------------------------------
// Command string helpers
// ---------------------------------------------------------------------------

impl Editor {
    pub fn clear_command(&mut self) {
        self.command.clear();
        self.command_cursor = 0;
        self.comp.on_leave_insert();
        self.cmd_history_idx = None;
    }
    pub fn push_command(&mut self, ch: char) {
        self.command.insert(self.command_cursor, ch);
        self.command_cursor += 1;
    }
    pub fn pop_command(&mut self) {
        if self.command_cursor > 0 {
            self.command_cursor -= 1;
            self.command.remove(self.command_cursor);
        }
    }
    pub fn set_command(&mut self, cmd: String) {
        self.command = cmd;
        self.command_cursor = self.command.len();
    }
    /// Move the command-line cursor to an absolute position (clamped).
    pub fn set_command_cursor(&mut self, pos: usize) {
        self.command_cursor = pos.min(self.command.len());
    }
}

// ---------------------------------------------------------------------------
// Spinner / LSP
// ---------------------------------------------------------------------------

impl Editor {
    pub fn set_lsp_loading(&mut self, loading: bool) {
        self.lsp_loading = loading;
    }
    pub fn tick_spinner(&mut self) {
        self.spinner_frame = self.spinner_frame.wrapping_add(1);
    }

    pub fn active_virtual_line_count(&self) -> usize {
        if let Some(ref a) = self.buf().diff_alignment {
            a.len()
        } else {
            self.buf().len_lines()
        }
    }

    // -----------------------------------------------------------------------
    // Scroll helpers
    // -----------------------------------------------------------------------
    pub fn ensure_cursor_visible(&mut self, viewport_height: usize) {
        let w = self.active_window().position.width;
        let gutter = self.active_gutter_width();
        self.active_window_mut()
            .ensure_cursor_visible(viewport_height, w, 2, gutter);
    }

    /// Re-snap the active window's scroll so the cursor is visible
    /// within its current layout rectangle.
    pub fn snap_cursor_to_viewport(&mut self) {
        let h = self.active_window().position.height;
        let w = self.active_window().position.width;
        let gutter = self.active_gutter_width();
        if h > 0 && w > 0 {
            self.active_window_mut()
                .ensure_cursor_visible(h, w, 2, gutter);
        }
    }

    /// Compute the gutter width for the active buffer based on config.
    pub fn active_gutter_width(&self) -> usize {
        let mut width = 0;

        // Git signs column
        if self.config.git_gutter_enabled {
            width += 1;
        }

        // Line numbers
        if self.config.line_numbers_enabled {
            let total_lines = self.buf().len_lines().max(1);
            let digits = digit_count(total_lines);
            width += digits + 1; // digits + trailing space
        }

        // Bookmarks column
        if self.config.bookmarks_enabled {
            width += 1;
        }

        // Separator between gutter and text
        if width > 0 {
            width += 1;
        }

        width
    }

    /// Helper: number of decimal digits in `n`.
    fn _digit_count(n: usize) -> usize {
        if n == 0 {
            return 1;
        }
        let mut count = 0;
        let mut val = n;
        while val > 0 {
            val /= 10;
            count += 1;
        }
        count
    }

    pub fn scroll_active_window_to_cursor(&mut self) {
        let h = self.active_window().position.height;
        let w = self.active_window().position.width;
        let gutter = self.active_gutter_width();
        self.active_window_mut().scroll_to_cursor(h, w, gutter);

        let max_scroll = self.buf().len_lines().saturating_sub(h.saturating_sub(1));
        let win = self.active_window_mut();
        if win.scroll_line > max_scroll {
            win.scroll_line = max_scroll;
        }
    }

    // ---------------------------------------------------------------------------
    // Buffer management
    // ---------------------------------------------------------------------------

    /// Split the active window horizontally (top / bottom).
    ///
    /// The new pane initially shows the same buffer at the same cursor
    /// position.
    pub fn split_horizontal(&mut self) {
        if self.windows.len() >= MAX_WINDOWS {
            self.set_status("Maximum number of windows reached", MessageKind::Error);
            return;
        }

        let cur_win_id = self.active_window().id;
        let bid = self.active_window().buffer_id();
        let (row, col, scroll_line, scroll_col) = {
            let w = self.active_window();
            (w.row, w.col, w.scroll_line, w.scroll_col)
        };

        let new_win_id = self.next_win_id;
        self.next_win_id += 1;

        // Update layout tree.
        self.layout
            .split_leaf(cur_win_id, SplitDir::Horizontal, new_win_id);

        // Create new window sharing the same buffer.
        let mut new_win = Window::new(new_win_id, bid);
        new_win.row = row;
        new_win.col = col;
        new_win.scroll_line = scroll_line;
        new_win.scroll_col = scroll_col;

        // Append and focus the new window.
        let new_idx = self.windows.len();
        self.windows.push(new_win);
        self.active_window_idx = new_idx;

        let old_height = self
            .windows
            .iter()
            .find(|w| w.id == cur_win_id)
            .map(|w| w.position.height)
            .unwrap_or(40);
        let est_height = old_height.saturating_sub(1) / 2;
        let est_width = self.active_window().position.width; // Preserve width
        let gutter = self.active_gutter_width();
        if est_height > 0 {
            for win in &mut self.windows {
                if win.id == cur_win_id || win.id == new_win_id {
                    win.ensure_cursor_visible(est_height, est_width, 2, gutter);
                }
            }
        }

        self.set_status("Split horizontally", MessageKind::Info);
    }

    /// Split the active window vertically (left / right).
    pub fn split_vertical(&mut self) {
        if self.windows.len() >= MAX_WINDOWS {
            self.set_status("Maximum number of windows reached", MessageKind::Error);
            return;
        }

        let cur_win_id = self.active_window().id;
        let bid = self.active_window().buffer_id();
        let (row, col, scroll_line, scroll_col) = {
            let w = self.active_window();
            (w.row, w.col, w.scroll_line, w.scroll_col)
        };

        let new_win_id = self.next_win_id;
        self.next_win_id += 1;

        self.layout
            .split_leaf(cur_win_id, SplitDir::Vertical, new_win_id);

        let mut new_win = Window::new(new_win_id, bid);
        new_win.row = row;
        new_win.col = col;
        new_win.scroll_line = scroll_line;
        new_win.scroll_col = scroll_col;

        let new_idx = self.windows.len();
        self.windows.push(new_win);
        self.active_window_idx = new_idx;

        let old_width = self
            .windows
            .iter()
            .find(|w| w.id == cur_win_id)
            .map(|w| w.position.width)
            .unwrap_or(80);
        let est_width = old_width.saturating_sub(1) / 2;
        let est_height = self.active_window().position.height; // Preserve height
        let gutter = self.active_gutter_width();
        if est_width > 0 {
            for win in &mut self.windows {
                if win.id == cur_win_id || win.id == new_win_id {
                    win.ensure_cursor_visible(est_height, est_width, 2, gutter);
                }
            }
        }

        self.set_status("Split vertically", MessageKind::Info);
    }

    /// Close the active window.
    ///
    /// If `force` is false and the window is the last viewer of a
    /// modified buffer, the close is refused with a status message.
    /// If this is the last window the editor quits (or prompts).
    pub fn close_window(&mut self, force: bool) {
        if self.windows.len() <= 1 {
            if force {
                self.should_quit = true;
            } else {
                self.quit_check();
            }
            return;
        }

        // Warn if the buffer is modified and no other window is viewing it.
        if !force && !self.is_buffer_viewed_elsewhere() && self.buf().modified {
            self.set_status_msg(
                "No write since last change (use :q! to force)",
                MessageKind::Error,
            );
            return;
        }

        let win_id = self.active_window().id;

        // Clear sibling's link before discarding this window
        let sibling_id = self.active_window().diff_sibling;
        if let Some(sib_id) = sibling_id {
            if let Some(sib) = self.windows.iter_mut().find(|w| w.id == sib_id) {
                sib.diff_sibling = None;
            }
        }

        // Remove from the layout tree.
        self.layout.remove_leaf(win_id);

        // Calculate the index to focus after removal.
        let remove_idx = self.active_window_idx;
        let next_idx = if remove_idx + 1 < self.windows.len() {
            remove_idx // the window after this one slides down
        } else {
            remove_idx.saturating_sub(1)
        };

        self.windows.remove(remove_idx);
        self.active_window_idx = next_idx.min(self.windows.len() - 1);

        self.comp.on_leave_insert();
        self.set_status("Window closed", MessageKind::Info);
    }

    /// Close every window except the active one.
    pub fn only_window(&mut self) {
        if self.windows.len() <= 1 {
            return;
        }

        let keep_id = self.active_window().id;

        // Reset the layout tree to a single leaf.
        self.layout = LayoutNode::Leaf(keep_id);

        // Remove all other windows.
        self.windows.retain(|w| w.id == keep_id);
        self.active_window_idx = 0;

        self.comp.on_leave_insert();
        self.set_status("Closed other windows", MessageKind::Info);
    }

    // ---- Focus cycling ----

    /// Focus the next window (wraps around).
    pub fn focus_next_window(&mut self) {
        if self.windows.len() <= 1 {
            return;
        }
        self.active_window_idx = (self.active_window_idx + 1) % self.windows.len();
    }

    /// Focus the previous window (wraps around).
    pub fn focus_prev_window(&mut self) {
        if self.windows.len() <= 1 {
            return;
        }
        self.active_window_idx = if self.active_window_idx == 0 {
            self.windows.len() - 1
        } else {
            self.active_window_idx - 1
        };
    }

    // ---- Directional focus ----

    /// Focus the nearest window to the left.
    pub fn focus_window_left(&mut self) {
        self.focus_window_directional(-1, 0);
    }

    /// Focus the nearest window to the right.
    pub fn focus_window_right(&mut self) {
        self.focus_window_directional(1, 0);
    }

    /// Focus the nearest window above.
    pub fn focus_window_up(&mut self) {
        self.focus_window_directional(0, -1);
    }

    /// Focus the nearest window below.
    pub fn focus_window_down(&mut self) {
        self.focus_window_directional(0, 1);
    }

    /// Generic directional focus.
    ///
    /// `(dx, dy)` indicates the direction: `(-1, 0)` = left, `(1, 0)` =
    /// right, `(0, -1)` = up, `(0, 1)` = down.
    fn focus_window_directional(&mut self, dx: isize, dy: isize) {
        if self.windows.len() <= 1 {
            return;
        }

        let cur = self.active_window().position;
        if !cur.is_visible() {
            // Positions not yet computed; fall back to cycling.
            if dx > 0 || dy > 0 {
                self.focus_next_window();
            } else {
                self.focus_prev_window();
            }
            return;
        }

        let cur_cx = cur.x as isize + cur.width as isize / 2;
        let cur_cy = cur.y as isize + cur.height as isize / 2;

        let mut best_idx: Option<usize> = None;
        let mut best_dist: isize = isize::MAX;

        for (i, win) in self.windows.iter().enumerate() {
            if i == self.active_window_idx {
                continue;
            }
            let p = win.position;
            if !p.is_visible() {
                continue;
            }

            let o_cx = p.x as isize + p.width as isize / 2;
            let o_cy = p.y as isize + p.height as isize / 2;

            let diff_x = o_cx - cur_cx;
            let diff_y = o_cy - cur_cy;

            let in_dir = if dx != 0 {
                diff_x * dx > 0
            } else {
                diff_y * dy > 0
            };

            if !in_dir {
                continue;
            }

            // Must overlap along the perpendicular axis
            let overlaps_perp = if dx != 0 {
                cur.overlaps_vertically(&p)
            } else {
                cur.overlaps_horizontally(&p)
            };

            let dist = if dx != 0 { diff_x.abs() } else { diff_y.abs() };

            let score = if overlaps_perp { dist } else { dist + 10000 };

            if score < best_dist {
                best_dist = score;
                best_idx = Some(i);
            }
        }

        if let Some(idx) = best_idx {
            self.active_window_idx = idx;
        }
    }

    pub fn open_buffer(&mut self, path: Option<String>) {
        // Save old active buffer position
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

        let old_bid = self.active_window().buffer_id();
        let mut should_clean_old = false;

        // Check if the current buffer is an unmodified, empty "No Name" buffer
        if let Some(old_buf) = self.buf_by_id(old_bid) {
            if old_buf.filename.is_none() && !old_buf.modified && old_buf.rope.len_chars() <= 1 {
                should_clean_old = true;
            }
        }

        // ── Re-use existing buffer if the file is already open ─────────
        if let Some(ref p) = path {
            let file_path = std::path::Path::new(p);
            if let Some(existing_id) =
                crate::ed::buffer::find_buffer_by_filename(&self.buffers, file_path)
            {
                if existing_id == old_bid {
                    return; // Already viewing this file
                }

                // Switch window and restore position
                self.active_window_mut().set_buffer_id(existing_id);

                let mut restored = false;
                if let Some((row, col)) = self.get_saved_position(p) {
                    // FIX: extract buffer data first to avoid simultaneous mutable+immutable borrows
                    let (len_lines, line_char_len) = {
                        let buf = self.buf_by_id(existing_id).unwrap();
                        let safe_row = row.min(buf.len_lines().saturating_sub(1));
                        (buf.len_lines(), buf.line_char_len(safe_row))
                    };
                    let win = self.active_window_mut();
                    win.row = row.min(len_lines.saturating_sub(1));
                    win.col = col.min(line_char_len);
                    self.scroll_active_window_to_cursor();
                    restored = true;
                }

                if !restored {
                    let win = self.active_window_mut();
                    win.row = 0;
                    win.col = 0;
                    win.scroll_line = 0;
                    win.scroll_col = 0;
                }

                self.comp.on_leave_insert();
                self.refresh_buffer_words();

                // Clean up the old empty "No Name" buffer if no other window views it
                if should_clean_old && !self.windows.iter().any(|w| w.buffer_id() == old_bid) {
                    self.buffers.retain(|b| b.id != old_bid);
                }

                // Sync into MRU with correct restored coordinates
                let path_buf = std::path::PathBuf::from(p);
                if let Ok(canon_path) = std::fs::canonicalize(&path_buf) {
                    let win = self.active_window();
                    self.mru_manager.insert(canon_path, win.row, win.col);
                }
                return;
            }
        }

        // ── No existing buffer — create a new one ─────────────────────
        match Buffer::new(self.next_buf_id, path.clone()) {
            Ok(buf) => {
                let bid = buf.id;
                self.next_buf_id += 1;
                self.buffers.push(buf);

                // Switch window and restore position
                self.active_window_mut().set_buffer_id(bid);

                let mut restored = false;
                if let Some(ref p) = path {
                    if let Some((row, col)) = self.get_saved_position(p) {
                        // FIX: extract buffer data first to avoid simultaneous mutable+immutable borrows
                        let (len_lines, line_char_len) = {
                            let buf = self.buf_by_id(bid).unwrap();
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

                self.comp.on_leave_insert();
                self.refresh_buffer_words();

                // Clean up the old empty "No Name" buffer if no other window views it
                if should_clean_old && !self.windows.iter().any(|w| w.buffer_id() == old_bid) {
                    self.buffers.retain(|b| b.id != old_bid);
                }

                // Sync into MRU with correct coordinates
                if let Some(ref p) = path {
                    let path_buf = std::path::PathBuf::from(p);
                    if let Ok(canon_path) = std::fs::canonicalize(&path_buf) {
                        let win = self.active_window();
                        self.mru_manager.insert(canon_path, win.row, win.col);
                    }
                }
            }
            Err(e) => {
                self.set_status(&format!("Failed to open: {}", e), MessageKind::Error);
            }
        }
    }

    pub fn switch_next_buffer(&mut self) {
        if self.buffers.len() <= 1 {
            self.set_status("Only one buffer", MessageKind::Info);
            return;
        }
        let bid = self.active_window().buffer_id();
        let idx = self.buffers.iter().position(|b| b.id == bid).unwrap_or(0);
        let next_idx = (idx + 1) % self.buffers.len();
        let next_bid = self.buffers[next_idx].id;

        self.switch_window_to_buffer(next_bid);

        self.comp.on_leave_insert();
        self.set_status(
            &format!(
                "[{}/{}] {}",
                next_idx + 1,
                self.buffers.len(),
                self.buf().display_name()
            ),
            MessageKind::Info,
        );
        self.refresh_buffer_words();
    }

    pub fn switch_prev_buffer(&mut self) {
        if self.buffers.len() <= 1 {
            self.set_status("Only one buffer", MessageKind::Info);
            return;
        }
        let bid = self.active_window().buffer_id();
        let idx = self.buffers.iter().position(|b| b.id == bid).unwrap_or(0);
        let prev_idx = if idx == 0 {
            self.buffers.len() - 1
        } else {
            idx - 1
        };
        let prev_bid = self.buffers[prev_idx].id;

        self.switch_window_to_buffer(prev_bid);

        self.comp.on_leave_insert();
        self.set_status(
            &format!(
                "[{}/{}] {}",
                prev_idx + 1,
                self.buffers.len(),
                self.buf().display_name()
            ),
            MessageKind::Info,
        );
        self.refresh_buffer_words();
    }

    pub fn switch_buffer_by_index(&mut self, idx: usize) {
        if idx > 0 && idx <= self.buffers.len() {
            let target_bid = self.buffers[idx - 1].id;
            self.switch_window_to_buffer(target_bid);

            self.comp.on_leave_insert();
            self.set_status(
                &format!("Switched to buffer {} ({})", idx, self.buf().display_name()),
                MessageKind::Info,
            );
            self.refresh_buffer_words();
        } else {
            self.set_status(
                &format!("Invalid buffer number: {}", idx),
                MessageKind::Error,
            );
        }
    }

    pub fn switch_buffer_by_name_or_index(&mut self, arg: &str) {
        let arg = arg.trim();
        if arg.is_empty() {
            self.set_status_msg("Usage: :b <index_or_name>", MessageKind::Error);
            return;
        }

        // 1. Try parsing as a numeric index first
        if let Ok(idx) = arg.parse::<usize>() {
            if idx > 0 && idx <= self.buffers.len() {
                let target_bid = self.buffers[idx - 1].id;
                self.switch_window_to_buffer(target_bid);

                self.comp.on_leave_insert();
                let disp_name = self.buf().display_name();
                self.set_status(
                    &format!("Switched to buffer {} ({})", idx, disp_name),
                    MessageKind::Info,
                );
                self.refresh_buffer_words();
                return;
            }
        }

        // 2. Try exact display name or path match (case-sensitive)
        let mut target_bid = None;
        for buf in &self.buffers {
            let disp = buf.display_name();
            if disp == arg {
                target_bid = Some(buf.id);
                break;
            }
            if let Some(ref filename) = buf.filename {
                if filename == arg {
                    target_bid = Some(buf.id);
                    break;
                }
            }
        }

        // 3. Try case-insensitive substring/partial match
        if target_bid.is_none() {
            let lower_arg = arg.to_lowercase();
            let mut matches = Vec::new();
            for buf in &self.buffers {
                let disp = buf.display_name().to_lowercase();
                if disp.contains(&lower_arg) {
                    matches.push(buf.id);
                    continue;
                }
                if let Some(ref filename) = buf.filename {
                    if filename.to_lowercase().contains(&lower_arg) {
                        matches.push(buf.id);
                    }
                }
            }

            if matches.len() == 1 {
                target_bid = Some(matches[0]);
            } else if matches.len() > 1 {
                // Smart prefix prioritization if multiple match
                let mut best_match = None;
                for &bid in &matches {
                    if let Some(buf) = self.buf_by_id(bid) {
                        if buf.display_name().to_lowercase().starts_with(&lower_arg) {
                            best_match = Some(bid);
                            break;
                        }
                    }
                }
                target_bid = best_match.or(Some(matches[0]));
            }
        }

        if let Some(bid) = target_bid {
            self.switch_window_to_buffer(bid);
            self.comp.on_leave_insert();
            let disp_name = self.buf().display_name();
            self.set_status(
                &format!("Switched to buffer ({})", disp_name),
                MessageKind::Info,
            );
            self.refresh_buffer_words();
        } else {
            self.set_status_msg(&format!("No buffer matching '{}'", arg), MessageKind::Error);
        }
    }

    pub fn close_buffer(&mut self) {
        let bid = self.active_window().buffer_id();
        self.close_buffer_by_id(bid);
    }

    pub fn list_buffers(&mut self) {
        let mut msg = String::from("Buffers:");
        for (i, buf) in self.buffers.iter().enumerate() {
            let active = if buf.id == self.active_window().buffer_id() {
                " %"
            } else {
                "  "
            };
            let modified = if buf.modified { " [+]" } else { "" };
            let name = buf.filename.as_deref().unwrap_or("[No Name]");
            msg.push_str(&format!("\n  {} {} {}{}", active, i + 1, name, modified));
        }
        self.set_status(&msg, MessageKind::Info);
    }

    pub fn save_active_buffer(&mut self) -> Result<()> {
        let format_on_save = self.config.format_on_save;
        self.buf_mut().save_file(format_on_save)?;
        self.refresh_buffer_words();
        Ok(())
    }

    pub fn set_active_filename(&mut self, path: String) {
        let bid = self.active_window().buffer_id();
        if let Some(buf) = self.buffers.iter_mut().find(|b| b.id == bid) {
            buf.filename = Some(path);
        }
    }
}
