//! Hunk-level operations: popup, revert, jump-to-hunk, diff stats.

use crate::ed::buffer::BufferKind;
use crate::ed::misc_helper::{get_head_file_content, group_signed_rows};
use crate::ed::Buffer;
use crate::ed::MessageKind;
use crate::Editor;

// ═══════════════════════════════════════════════════════════════════════
// Free helper functions — pure logic, no &self
// ═══════════════════════════════════════════════════════════════════════

/// Find the hunk index in `patch` that covers `row` (0-based).
fn find_hunk_idx(patch: &git2::Patch, row: usize) -> Option<usize> {
    for i in 0..patch.num_hunks() {
        let hunk = match patch.hunk(i) {
            Ok(h) => h,
            Err(_) => continue,
        };
        let new_start = hunk.0.new_start().max(0) as usize;
        let new_count = hunk.0.new_lines().max(0) as usize;

        let covers = if new_count > 0 {
            let start_0 = new_start.saturating_sub(1);
            row >= start_0 && row < start_0 + new_count
        } else {
            let anchor = new_start.saturating_sub(1);
            row >= anchor.saturating_sub(1) && row <= anchor + 1
        };

        if covers {
            return Some(i);
        }
    }
    None
}

/// Expand tab characters to `width` spaces, tracking column position so
/// tabs align to stops rather than each counting as a single column.
/// Popup box-drawing computes line width from `.chars().count()`, and an
/// un-expanded `\t` renders wider in the terminal than it counts for,
/// which breaks the border alignment.
fn expand_tabs(s: &str, width: usize) -> String {
    let mut out = String::with_capacity(s.len());
    let mut col = 0usize;
    for ch in s.chars() {
        if ch == '\t' {
            let spaces = width - (col % width);
            out.push_str(&" ".repeat(spaces));
            col += spaces;
        } else {
            out.push(ch);
            col += 1;
        }
    }
    out
}
/// Extract formatted diff lines from a specific hunk.
fn extract_hunk_lines(patch: &git2::Patch, hunk_idx: usize) -> Vec<String> {
    let hunk_header = match patch.hunk(hunk_idx) {
        Ok(h) => h.0,
        Err(_) => return Vec::new(),
    };

    let mut lines = vec![format!(
        "@@ -{},{} +{},{} @@",
        hunk_header.old_start(),
        hunk_header.old_lines(),
        hunk_header.new_start(),
        hunk_header.new_lines(),
    )];

    if let Ok(num_lines) = patch.num_lines_in_hunk(hunk_idx) {
        for i in 0..num_lines {
            if let Ok(line) = patch.line_in_hunk(hunk_idx, i) {
                let origin = line.origin();
                let content = String::from_utf8_lossy(line.content()).to_string();
                let trimmed = content.trim_end_matches('\n');
                let expanded = expand_tabs(trimmed, 4);
                match origin {
                    '+' | '-' | ' ' => lines.push(format!("{}{}", origin, expanded)),
                    _ => lines.push(expanded),
                }
            }
        }
    }
    lines
}

// ═══════════════════════════════════════════════════════════════════════

impl Editor {
    /// Open a bottom-up popup showing the unified diff of the git hunk
    /// under the cursor.
    pub fn open_hunk_popup(&mut self) {
        let filename = match self.buf().filename.clone() {
            Some(f) => f,
            None => {
                self.set_status_msg("No file associated with buffer", MessageKind::Error);
                return;
            }
        };

        let current_row = self.active_window().row;

        // ── 1. Check cursor is on a hunk ──────────────────────────────
        if !self.cursor_on_hunk_impl(current_row) {
            return;
        }

        // ── 2. Open repo, compute patch, extract lines ────────────────
        //    Everything is inlined so the Patch borrows from local
        //    Strings rather than from &mut self, avoiding borrow conflicts.
        let hunk_lines = {
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

            let head_content = match get_head_file_content(&repo, &rel_path) {
                Some(c) => c,
                None => {
                    self.set_status_msg("Could not read HEAD content", MessageKind::Error);
                    return;
                }
            };

            let current_content = self.buf().rope.to_string();

            let patch = match git2::Patch::from_buffers(
                head_content.as_bytes(),
                Some(std::path::Path::new(&rel_path)),
                current_content.as_bytes(),
                Some(std::path::Path::new(&rel_path)),
                None,
            ) {
                Ok(p) => p,
                Err(_) => {
                    self.set_status_msg("Could not compute diff", MessageKind::Error);
                    return;
                }
            };

            let hunk_idx = match find_hunk_idx(&patch, current_row) {
                Some(idx) => idx,
                None => {
                    self.set_status_msg("Could not locate hunk in diff", MessageKind::Error);
                    return;
                }
            };

            extract_hunk_lines(&patch, hunk_idx)
        };
        // patch, head_content, current_content all dropped here

        if hunk_lines.is_empty() {
            self.set_status_msg("Empty hunk", MessageKind::Error);
            return;
        }

        self.popup.git_hunk = Some(crate::popup::git_hunk::GitHunkPopup::new(hunk_lines));
    }

    /// Revert the git hunk under the cursor (buffer-only, no disk writes).
    pub fn revert_hunk(&mut self) {
        let filename = match self.buf().filename.clone() {
            Some(f) => f,
            None => {
                self.set_status_msg("No file associated with buffer", MessageKind::Error);
                return;
            }
        };

        let current_row = self.active_window().row;

        // ── 1. Check cursor is on a hunk ──────────────────────────────
        if !self.cursor_on_hunk_impl(current_row) {
            return;
        }

        // ── 2. Compute patch and extract hunk metadata ────────────────
        let (new_start_1, new_count, old_start_1, old_count, reverted_text) = {
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
                    self.set_status_msg("Bare repository (no workdir)", MessageKind::Error);
                    return;
                }
            };

            let canon = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
            let rel_path = match canon.strip_prefix(workdir) {
                Ok(rp) => rp.to_string_lossy().to_string(),
                Err(_) => path.to_string_lossy().to_string(),
            };

            let head_content = match get_head_file_content(&repo, &rel_path) {
                Some(c) => c,
                None => {
                    self.set_status_msg("Could not read HEAD content", MessageKind::Error);
                    return;
                }
            };

            let current_content = self.buf().rope.to_string();

            let patch = match git2::Patch::from_buffers(
                head_content.as_bytes(),
                Some(std::path::Path::new(&rel_path)),
                current_content.as_bytes(),
                Some(std::path::Path::new(&rel_path)),
                None,
            ) {
                Ok(p) => p,
                Err(_) => {
                    self.set_status_msg("Could not compute diff", MessageKind::Error);
                    return;
                }
            };

            let hunk_idx = match find_hunk_idx(&patch, current_row) {
                Some(idx) => idx,
                None => {
                    self.set_status_msg("Could not locate hunk in diff", MessageKind::Error);
                    return;
                }
            };

            let hunk = patch.hunk(hunk_idx).unwrap();
            let new_start_1 = hunk.0.new_start().max(0) as usize;
            let new_count = hunk.0.new_lines().max(0) as usize;
            let old_start_1 = hunk.0.old_start().max(0) as usize;
            let old_count = hunk.0.old_lines().max(0) as usize;

            // Build reverted text from HEAD content
            let head_lines: Vec<&str> = head_content.lines().collect();
            let old_start_0 = old_start_1.saturating_sub(1);
            let old_end = old_start_0 + old_count;

            let mut reverted_text = String::new();
            for line in head_lines.get(old_start_0..old_end).unwrap_or(&[]) {
                reverted_text.push_str(line);
                reverted_text.push('\n');
            }

            (
                new_start_1,
                new_count,
                old_start_1,
                old_count,
                reverted_text,
            )
        };
        // patch, head_content, current_content all dropped here

        // ── 3. Apply reverse edit to the rope ─────────────────────────
        {
            let (win, buf) = self.active_window_and_buf_mut();
            buf.push_undo(win.row, win.col);

            let new_start_0 = new_start_1.saturating_sub(1);
            let new_end_0 = new_start_0 + new_count;

            if new_count > 0 && new_end_0 <= buf.len_lines() {
                let start_char = buf.rope.line_to_char(new_start_0);
                let end_char = buf.rope.line_to_char(new_end_0);
                if end_char > start_char {
                    buf.rope.remove(start_char..end_char);
                }
            }

            if !reverted_text.is_empty() {
                let insert_line = new_start_0.min(buf.len_lines());
                let insert_char = buf.rope.line_to_char(insert_line);
                buf.rope.insert(insert_char, &reverted_text);
            }

            buf.mark_modified();
            buf.parse_syntax();

            let cursor_row = new_start_0.min(buf.len_lines().saturating_sub(1));
            win.row = cursor_row;
            win.col = 0;
            win.desired_col = 0;
            self.scroll_active_window_to_cursor();
        }

        // ── 4. Re-request gutter diff ─────────────────────────────────
        let bid = self.buf().id;
        let rope = self.buf().rope.clone();
        self.async_gutter.request_diff(bid, &rope, Some(&filename));

        self.set_status_msg("Reverted hunk", MessageKind::Success);
    }

    /// Jump the cursor to the start of the next git hunk.
    pub fn jump_to_next_hunk(&mut self) {
        let current_row = self.active_window().row;

        let starts = self.hunk_starts();
        if starts.is_empty() {
            self.set_status_msg("No git hunks found", MessageKind::Info);
            return;
        }

        let target = starts
            .iter()
            .find(|&&r| r > current_row)
            .cloned()
            .or_else(|| starts.first().cloned());

        if let Some(row) = target {
            self.jump_to_hunk_row(row);
        }
    }

    /// Jump the cursor to the start of the previous git hunk.
    pub fn jump_to_prev_hunk(&mut self) {
        let current_row = self.active_window().row;

        let starts = self.hunk_starts();
        if starts.is_empty() {
            self.set_status_msg("No git hunks found", MessageKind::Info);
            return;
        }

        let target = starts
            .iter()
            .rev()
            .find(|&&r| r < current_row)
            .cloned()
            .or_else(|| starts.last().cloned());

        if let Some(row) = target {
            self.jump_to_hunk_row(row);
        }
    }

    /// Returns the current git branch name (fast, no shell spawn).
    pub fn current_git_branch(&self) -> Option<String> {
        let filename = self.buf().filename.as_deref()?;
        let repo = git2::Repository::discover(std::path::Path::new(filename)).ok()?;
        let head = repo.head().ok()?;
        let name = head.shorthand()?.to_string();
        Some(name)
    }

    /// Returns the raw stats for the active buffer's git diffs.
    pub fn git_diff_stats(&self) -> (usize, usize, usize) {
        self.buf().git_diff_stats()
    }

    // ── Private helpers ───────────────────────────────────────────────

    fn jump_to_hunk_row(&mut self, row: usize) {
        self.active_window_mut().save_jump_position();
        let win = self.active_window_mut();
        win.row = row;
        win.col = 0;
        win.desired_col = 0;
        self.scroll_active_window_to_cursor();
        self.set_status_msg(
            &format!("Jumped to hunk at line {}", row + 1),
            MessageKind::Info,
        );
    }

    fn hunk_starts(&self) -> Vec<usize> {
        let mut signed_rows: Vec<usize> = self.buf().git_diffs.keys().cloned().collect();
        if signed_rows.is_empty() {
            return Vec::new();
        }
        signed_rows.sort_unstable();
        group_signed_rows(&signed_rows)
            .into_iter()
            .map(|h| h[0])
            .collect()
    }

    /// Check if `row` falls inside any git diff hunk.  Sets an error
    /// status and returns `false` if not on a hunk or no hunks exist.
    fn cursor_on_hunk_impl(&mut self, row: usize) -> bool {
        let mut signed_rows: Vec<usize> = self.buf().git_diffs.keys().cloned().collect();
        if signed_rows.is_empty() {
            self.set_status_msg("No git hunks found", MessageKind::Info);
            return false;
        }
        signed_rows.sort_unstable();
        let hunks = group_signed_rows(&signed_rows);
        let on_hunk = hunks
            .iter()
            .any(|h| row >= *h.first().unwrap() && row <= *h.last().unwrap());
        if !on_hunk {
            self.set_status_msg("Cursor is not on a git hunk", MessageKind::Error);
        }
        on_hunk
    }

    pub fn toggle_git_blame(&mut self) {
        self.blame_active = !self.blame_active;
        if self.blame_active {
            let filename = match self.buf().filename.clone() {
                Some(f) => f,
                None => {
                    self.set_status_msg("No file associated with buffer", MessageKind::Error);
                    self.blame_active = false;
                    return;
                }
            };
            self.blame_data = fetch_blame_data(&filename);
            self.set_status_msg(
                "Git blame enabled. Press 'c' to view commit.",
                MessageKind::Info,
            );
        } else {
            self.blame_data.clear();
            self.blame_diff_buf_id = None;
            self.blame_orig_buf_id = None;
            self.set_status_msg("Git blame disabled", MessageKind::Info);
        }
    }

    pub fn show_blame_commit(&mut self) {
        let row = self.active_window().row;
        let commit_hash = match self.blame_data.get(row).and_then(|o| o.as_ref()) {
            Some((_, hash)) => hash.clone(),
            None => {
                self.set_status_msg("No blame info at cursor", MessageKind::Error);
                return;
            }
        };

        let filename = self.buf().filename.clone().unwrap_or_default();
        let repo_root = match std::path::Path::new(&filename)
            .parent()
            .and_then(|p| git2::Repository::discover(p).ok())
            .and_then(|r| r.workdir().map(|w| w.to_path_buf()))
        {
            Some(root) => root,
            None => std::path::PathBuf::from("."),
        };

        let diff_text = crate::git::log::load_commit_diff(&repo_root, &commit_hash)
            .unwrap_or_else(|| "Failed to load commit diff".to_string());

        let buf_id = self.next_buf_id;
        self.next_buf_id += 1;
        let mut buf = match Buffer::new(buf_id, None) {
            Ok(b) => b,
            Err(_) => return,
        };
        buf.kind = BufferKind::GitDiff;
        buf.rope = ropey::Rope::from_str(&diff_text);
        buf.parse_syntax();

        self.buffers.push(buf);
        self.blame_orig_buf_id = Some(self.active_window().buffer_id());
        self.blame_diff_buf_id = Some(buf_id);
        self.windows[self.active_window_idx].set_buffer_id(buf_id);
        self.active_window_mut().row = 0;
        self.active_window_mut().col = 0;
        self.active_window_mut().scroll_line = 0;
    }

    pub fn close_blame_commit(&mut self) {
        if let Some(orig_id) = self.blame_orig_buf_id.take() {
            self.windows[self.active_window_idx].set_buffer_id(orig_id);
            self.blame_diff_buf_id = None;
            self.scroll_active_window_to_cursor();
        }
    }
}

fn fetch_blame_data(filename: &str) -> Vec<Option<(String, String)>> {
    let path = std::path::Path::new(filename);
    let repo = match git2::Repository::discover(path) {
        Ok(r) => r,
        Err(_) => return Vec::new(),
    };
    let workdir = match repo.workdir() {
        Some(wd) => wd,
        None => return Vec::new(),
    };
    let canon = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    let rel_path = match canon.strip_prefix(workdir) {
        Ok(rp) => rp.to_string_lossy().to_string(),
        Err(_) => path.to_string_lossy().to_string(),
    };

    let output = std::process::Command::new("git")
        .args(["blame", "--date=format:%d%b%y %H:%M", "--", &rel_path])
        .current_dir(workdir)
        .output();

    let output = match output {
        Ok(o) if o.status.success() => o,
        _ => return Vec::new(),
    };

    let text = String::from_utf8_lossy(&output.stdout);
    let mut result = Vec::new();
    let mut prev_key: Option<String> = None;
    let blame_width = 36;

    for line in text.lines() {
        if let Some((hash, rest)) = line.split_once(' ') {
            let hash = hash.trim_start_matches('^');
            if let Some(details) = rest.strip_prefix('(') {
                if let Some(end) = details.find(')') {
                    let inner = &details[..end];
                    let mut iter = inner.rsplitn(4, ' ');
                    let _line_num = iter.next().unwrap_or("");
                    let time = iter.next().unwrap_or("");
                    let day = iter.next().unwrap_or("");
                    let author = iter.next().unwrap_or("");
                    let full_date = format!("{} {}", day, time);

                    let short_hash = &hash[..hash.len().min(5)];
                    let key = format!("{}{}", hash, author);

                    if Some(&key) == prev_key.as_ref() {
                        result.push(None);
                    } else {
                        let short_author = if author.chars().count() > 8 {
                            author.chars().take(8).collect::<String>()
                        } else {
                            author.to_string()
                        };
                        let display = format!("{:<5} {:<13} {:<8} │ ", short_hash, full_date.trim(), short_author);
                        let display = if display.len() > blame_width {
                            display[..blame_width].to_string()
                        } else {
                            format!("{:<width$}", display, width = blame_width)
                        };
                        result.push(Some((display, hash.to_string())));
                        prev_key = Some(key);
                    }
                }
            }
        }
    }
    result
}
