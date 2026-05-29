//! Tag jump / tag-back handlers.
//!
//! - `C-]`  → `tag_under_cursor` — jump to the definition of the word
//!   under the cursor using the tags file.
//! - `:tag <name>` → `jump_to_tag` — jump to a tag by name.
//! - `C-t`  → `tag_back` — return to the previous position.

use crate::ed::tag::{TagEntry, TagStackEntry};
use crate::ed::MessageKind;
use crate::Editor;
use crossterm::event::KeyCode;

impl Editor {
    /// `C-]` — jump to the definition of the word under the cursor.
    pub fn tag_under_cursor(&mut self) {
        let word = match self.get_word_under_cursor() {
            Some(w) => w,
            None => {
                self.set_status_msg("No word under cursor", MessageKind::Error);
                return;
            }
        };
        self.jump_to_tag(&word);
    }

    /// `:tag <name>` — jump to a named tag.
    pub fn jump_to_tag(&mut self, name: &str) {
        let repo_root = match self.resolve_repo_root() {
            Some(r) => r,
            None => {
                self.set_status_msg("Not in a git repository", MessageKind::Error);
                return;
            }
        };

        if !self.tag_manager.ensure_loaded(&repo_root) {
            self.set_status_msg(
                "No tags file found. Install gentag or ctags, then run :retag",
                MessageKind::Error,
            );
            return;
        }

        let matches = self.tag_manager.lookup(name).to_vec();
        if matches.is_empty() {
            self.set_status_msg(&format!("Tag not found: {}", name), MessageKind::Error);
            return;
        }

        // Save current position on the tag stack before ANY jump or popup selection
        let win = self.active_window();
        self.tag_manager.push(TagStackEntry {
            buffer_id: win.buffer_id(),
            row: win.row,
            col: win.col,
            filename: self.active_filename().map(|s| s.to_string()),
        });

        if matches.len() == 1 {
            // Single match: jump immediately
            self.do_tag_jump(&matches[0], &repo_root, name, 1);
        } else {
            // Multiple candidates: open selection popup
            self.popup.tag_candidates = Some(
                crate::popup::tag_candidates::TagCandidatesPopup::new(matches),
            );
            self.set_status_msg(
                &format!("{} matching tags found. Select one.", name),
                MessageKind::Info,
            );
        }
    }

    /// Key handler for the Tag Candidates popup
    pub fn handle_tag_candidates_key(&mut self, key: crate::event::KeyEvent) {
        match key.code {
            KeyCode::Up | KeyCode::Char('k') => {
                if let Some(ref mut p) = self.popup.tag_candidates {
                    p.move_up();
                }
            }
            KeyCode::Down | KeyCode::Char('j') => {
                if let Some(ref mut p) = self.popup.tag_candidates {
                    p.move_down();
                }
            }
            KeyCode::Enter => {
                let selected_entry = self
                    .popup
                    .tag_candidates
                    .as_ref()
                    .and_then(|p| p.selected_entry().cloned());

                let repo_root = self.resolve_repo_root();

                // Close popup first
                self.popup.tag_candidates = None;

                if let (Some(entry), Some(root)) = (selected_entry, repo_root) {
                    self.do_tag_jump(&entry, &root, &entry.name, 1);
                }
            }
            KeyCode::Esc => {
                self.popup.tag_candidates = None;
                // Roll back the stack push since the user cancelled
                self.tag_manager.pop();
                self.set_status_msg("Tag jump cancelled", MessageKind::Info);
            }
            _ => {}
        }
    }

    /// `C-t` — return to the previous position from the tag stack.
    pub fn tag_back(&mut self) {
        let entry = match self.tag_manager.pop() {
            Some(e) => e,
            None => {
                self.set_status_msg("Tag stack empty", MessageKind::Info);
                return;
            }
        };

        let depth = self.tag_manager.stack_depth();

        // If the target buffer is already open, switch to it.
        let target_bid = self
            .buffers
            .iter()
            .find(|b| b.filename.as_deref() == entry.filename.as_deref())
            .map(|b| b.id);

        if let Some(bid) = target_bid {
            self.switch_window_to_buffer(bid);
        } else if let Some(ref filename) = entry.filename {
            // Buffer was closed — reopen it.
            self.open_buffer(Some(filename.to_string()));
        }

        // Restore cursor position.
        let (win, buf) = self.active_window_and_buf_mut();
        let max_row = buf.len_lines().saturating_sub(1);
        win.row = entry.row.min(max_row);
        win.col = entry.col.min(buf.line_char_len(win.row).saturating_sub(1));
        win.desired_col = win.col;
        self.scroll_active_window_to_cursor();

        self.set_status_msg(
            &format!("Tag back (stack depth: {})", depth),
            MessageKind::Info,
        );
    }

    /// `:retag` — regenerate the tags file for the current repo.
    pub fn retag(&mut self) {
        let repo_root = match self.resolve_repo_root() {
            Some(r) => r,
            None => {
                self.set_status_msg("Not in a git repository", MessageKind::Error);
                return;
            }
        };

        // Force a full reload (bypasses the freshness cache).
        self.tag_manager.invalidate_cache();

        if self.tag_manager.load_for_repo(&repo_root) {
            let count = self.tag_manager.tag_count();
            self.set_status_msg(
                &format!("Tags regenerated ({} tags loaded)", count),
                MessageKind::Success,
            );
        } else {
            self.set_status_msg(
                "Failed to generate tags. Install gentag or ctags.",
                MessageKind::Error,
            );
        }
    }

    /// `:tags` — show tag manager status.
    pub fn show_tag_info(&mut self) {
        let count = self.tag_manager.tag_count();
        let depth = self.tag_manager.stack_depth();
        let root = self
            .tag_manager
            .loaded_root()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|| "none".to_string());

        self.set_status_msg(
            &format!("Tags: {} loaded | Stack: {} | Root: {}", count, depth, root),
            MessageKind::Info,
        );
    }

    // ═══════════════════════════════════════════════════════════════════
    // Private helpers
    // ═══════════════════════════════════════════════════════════════════

    /// Perform the actual file-open and cursor-position for a tag jump.
    fn do_tag_jump(
        &mut self,
        entry: &TagEntry,
        repo_root: &std::path::Path,
        name: &str,
        total: usize,
    ) {
        // Resolve the tag's file path relative to the repo root.
        let full_path = repo_root.join(&entry.file);

        if !full_path.exists() {
            self.set_status_msg(
                &format!("Tag file not found: {}", entry.file.display()),
                MessageKind::Error,
            );
            // Roll back the stack push.
            self.tag_manager.pop();
            return;
        }

        let path_str = full_path.to_string_lossy().to_string();

        // Check if the buffer is already open.
        let existing_bid = self
            .buffers
            .iter()
            .find(|b| b.filename.as_deref() == Some(&path_str))
            .map(|b| b.id);

        if let Some(bid) = existing_bid {
            self.switch_window_to_buffer(bid);
        } else {
            self.open_buffer(Some(path_str));
        }

        // Position cursor at the tag's line.
        {
            let (win, buf) = self.active_window_and_buf_mut();
            // tag line numbers are 1-based; row is 0-based.
            let target_row = entry
                .line
                .saturating_sub(1)
                .min(buf.len_lines().saturating_sub(1));
            win.row = target_row;
            win.col = 0;
            win.desired_col = 0;
            win.save_jump_position();
        }
        self.center_viewport_on_cursor();

        // Status message.
        let kind_suffix = match &entry.kind {
            Some(k) => format!(" [{}]", k),
            None => String::new(),
        };
        let multi_suffix = if total > 1 {
            format!(" ({} matches, :tn for next)", total)
        } else {
            String::new()
        };

        self.set_status_msg(
            &format!(
                "Tag: {} → {}:{}{}{}",
                name,
                entry.file.display(),
                entry.line,
                kind_suffix,
                multi_suffix
            ),
            MessageKind::Info,
        );
    }
}
