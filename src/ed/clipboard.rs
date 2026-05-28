//! System clipboard integration via `arboard`.
//!
//! Provides bidirectional sync between the editor's internal
//! `clipboard: Option<String>` and the OS system clipboard so that
//! yanked text can be pasted into other applications and text copied
//! externally can be pasted into the editor.

use crate::ed::editing;
use crate::ed::mode::Mode;
use crate::ed::Editor;
use crate::ed::MessageKind;

// ---------------------------------------------------------------------------
// Low-level arboard helpers (isolated for testability & error handling)
// ---------------------------------------------------------------------------

/// Read the current system clipboard contents as a String.
fn read_system_clipboard() -> Option<String> {
    match arboard::Clipboard::new() {
        Ok(mut cb) => match cb.get_text() {
            Ok(text) => Some(text),
            Err(_) => None,
        },
        Err(_) => None,
    }
}

/// Write a string to the system clipboard.
fn write_system_clipboard(text: &str) -> bool {
    match arboard::Clipboard::new() {
        Ok(mut cb) => cb.set_text(text).is_ok(),
        Err(_) => false,
    }
}

// ---------------------------------------------------------------------------
// Editor integration
// ---------------------------------------------------------------------------

impl Editor {
    // ── Yank (copy) operations ──────────────────────────────────────

    /// Yank the current line or selection to both the internal and system clipboards.
    ///
    /// In Visual/VisualLine mode, yanks the selection instead.
    pub fn yank_to_system_clipboard(&mut self) {
        let text = self.extract_yank_text();

        if text.is_empty() {
            self.set_status_msg("Nothing to yank", MessageKind::Info);
            // Still exit visual mode if we tried to yank
            if self.mode == Mode::Visual || self.mode == Mode::VisualLine {
                self.enter_normal();
            }
            return;
        }

        // Always store in internal clipboard
        self.clipboard = Some(text.clone());

        // Push to system clipboard
        if write_system_clipboard(&text) {
            let lines = text.lines().count();
            let chars = text.chars().count();
            self.set_status_msg(
                &format!(
                    "Yanked {} line{}, {} chars → system clipboard",
                    lines,
                    if lines == 1 { "" } else { "s" },
                    chars
                ),
                MessageKind::Success,
            );
        } else {
            // Fallback: still yanked internally, just couldn't reach OS clipboard
            self.set_status_msg(
                "Yanked internally (system clipboard unavailable)",
                MessageKind::Info,
            );
        }

        // Exit visual mode after yanking (standard Vim behavior)
        if self.mode == Mode::Visual || self.mode == Mode::VisualLine {
            self.enter_normal();
        }
    }

    /// Yank from the system clipboard into the editor's internal clipboard,
    /// then paste at the cursor position.
    pub fn paste_from_system_clipboard(&mut self) {
        let text = match read_system_clipboard() {
            Some(t) => t,
            None => {
                self.set_status_msg(
                    "System clipboard is empty or unavailable",
                    MessageKind::Error,
                );
                return;
            }
        };

        // Sync internal clipboard
        self.clipboard = Some(text.clone());

        let is_linewise = text.ends_with('\n') && text.lines().count() > 1;

        let (win, buf) = self.active_window_and_buf_mut();
        buf.push_undo(win.row, win.col);

        if is_linewise {
            // Drop the trailing newline that indicates linewise —
            // `paste_line_below` re-adds it via rope insertion.
            let paste_content = if text.ends_with('\n') {
                text.clone()
            } else {
                format!("{}\n", text)
            };
            editing::paste_line_below(win, buf, &paste_content);
        } else {
            editing::paste_text(win, buf, &text);
        }

        self.set_status_msg(
            &format!(
                "Pasted {} chars from system clipboard",
                text.chars().count()
            ),
            MessageKind::Success,
        );
    }

    /// Cut (delete + yank) the current line or selection to both clipboards.
    pub fn cut_to_system_clipboard(&mut self) {
        let text = self.extract_yank_text();

        if text.is_empty() {
            self.set_status_msg("Nothing to cut", MessageKind::Info);
            if self.mode == Mode::Visual || self.mode == Mode::VisualLine {
                self.enter_normal();
            }
            return;
        }

        // Store in both clipboards before deletion
        self.clipboard = Some(text.clone());
        let _ = write_system_clipboard(&text);

        // Perform the deletion
        if self.mode == Mode::Visual || self.mode == Mode::VisualLine {
            self.delete_visual_selection();
            // delete_visual_selection already exits visual mode
        } else {
            // Cut the current line (dd-style)
            let (win, buf) = self.active_window_and_buf_mut();
            buf.push_undo(win.row, win.col);
            editing::delete_current_line(win, buf);
        }

        self.set_status_msg(
            &format!("Cut {} chars → system clipboard", text.chars().count()),
            MessageKind::Success,
        );
    }

    /// Copy the word under the cursor to both clipboards.
    pub fn yank_word_to_system_clipboard(&mut self) {
        if let Some(word) = self.get_word_under_cursor() {
            self.clipboard = Some(word.clone());
            if write_system_clipboard(&word) {
                self.set_status_msg(
                    &format!("Yanked word '{}' → system clipboard", word),
                    MessageKind::Success,
                );
            } else {
                self.set_status_msg(
                    "Yanked word internally (system clipboard unavailable)",
                    MessageKind::Info,
                );
            }
        } else {
            self.set_status_msg("No word under cursor", MessageKind::Info);
        }
    }

    /// Yank from the system clipboard as a line-wise paste below the
    /// current line (vim `:put` style).
    pub fn put_from_system_clipboard_below(&mut self) {
        let text = match read_system_clipboard() {
            Some(t) => t,
            None => {
                self.set_status_msg(
                    "System clipboard is empty or unavailable",
                    MessageKind::Error,
                );
                return;
            }
        };

        self.clipboard = Some(text.clone());

        let paste_content = if text.ends_with('\n') {
            text
        } else {
            format!("{}\n", text)
        };

        let (win, buf) = self.active_window_and_buf_mut();
        buf.push_undo(win.row, win.col);
        editing::paste_line_below(win, buf, &paste_content);

        self.set_status_msg("Put from system clipboard below", MessageKind::Success);
    }

    // ── Internal helpers ────────────────────────────────────────────

    /// Extract the text that should be yanked based on the current mode.
    ///
    /// - **Visual / VisualLine**: returns the selection
    /// - **Normal**: returns the current line (including its newline)
    fn extract_yank_text(&self) -> String {
        let win = self.active_window();
        let buf = self.buf();

        if self.mode == Mode::Visual || self.mode == Mode::VisualLine {
            if let Some((start, end)) = win.get_selection_range(buf, self.mode) {
                if end > start && end <= buf.rope.len_chars() {
                    return buf.rope.slice(start..end).to_string();
                }
            }
        }

        // Default: yank the current line (linewise)
        if win.row < buf.len_lines() {
            let line = buf.line_text(win.row);
            format!("{}\n", line)
        } else {
            String::new()
        }
    }

    /// Delete the current visual selection (used by cut).
    fn delete_visual_selection(&mut self) {
        let (start, end) = {
            let win = self.active_window();
            let buf = self.buf();
            match win.get_selection_range(buf, self.mode) {
                Some(range) => range,
                None => return,
            }
        };

        if end <= start || end > self.buf().rope.len_chars() {
            return;
        }

        let (win, buf) = self.active_window_and_buf_mut();
        buf.push_undo(win.row, win.col);
        buf.rope.remove(start..end);
        buf.mark_modified();

        // Reposition cursor to start of deleted region
        let new_row = buf.rope.char_to_line(start);
        let line_start = buf.rope.line_to_char(new_row);
        win.row = new_row;
        win.col = (start - line_start).min(buf.line_char_len(new_row));
        win.desired_col = win.col;
        win.visual_anchor = None;

        buf.parse_syntax();

        // Exit visual mode
        self.enter_normal();
    }
}
