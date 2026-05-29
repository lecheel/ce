//! Substitution command (`:s`, `:%s`) with optional confirm (`c` flag).

use crate::ed::MessageKind;
use crate::event::KeyCode;
use crate::Editor;

/// State tracked during an interactive `gc` substitution.
pub struct SubstitutionState {
    pub pattern: String,
    pub replacement: String,
    pub global: bool,
    pub current_row: usize,
    pub current_col: usize, // Character index (NOT byte index)
    pub end_row: usize,
    pub replace_all: bool,
    pub replacements_made: usize,
}

/// Safely find a substring using character indices instead of byte indices.
fn find_char_pos_from(text: &str, pattern: &str, start_char: usize) -> Option<usize> {
    let byte_start = text
        .char_indices()
        .nth(start_char)
        .map(|(b, _)| b)
        .unwrap_or(text.len());

    if let Some(rel_byte_pos) = text[byte_start..].find(pattern) {
        let abs_byte_pos = byte_start + rel_byte_pos;
        let char_pos = text[..abs_byte_pos].chars().count();
        Some(char_pos)
    } else {
        None
    }
}

impl Editor {
    /// Begin a substitution command.
    /// `range`: Optional tuple of (start_row, end_row) inclusive for visual selections.
    /// `whole_file`: If true and no range provided, operates on the entire file.
    pub fn start_substitution(
        &mut self,
        pattern: String,
        replacement: String,
        flags: String,
        range: Option<(usize, usize)>,
        whole_file: bool,
    ) {
        if pattern.is_empty() {
            self.set_status_msg("Empty pattern", MessageKind::Error);
            return;
        }

        let global = flags.contains('g');
        let confirm = flags.contains('c');

        let (start_row, end_row) = if let Some((r1, r2)) = range {
            (r1, r2 + 1) // end_row is exclusive in our loops, so +1
        } else if whole_file {
            (0, self.buf().len_lines())
        } else {
            let current_row = self.active_window().row;
            (current_row, current_row + 1)
        };

        if confirm {
            // Push a single undo snapshot for the entire substitution operation
            let (win, buf) = self.active_window_and_buf_mut();
            buf.push_undo(win.row, win.col);

            // Save previous search so we can restore it after
            self.prev_search_query = self.last_search_query.clone();

            self.substitution_state = Some(SubstitutionState {
                pattern: pattern.clone(),
                replacement,
                global,
                current_row: start_row,
                current_col: 0,
                end_row,
                replace_all: false,
                replacements_made: 0,
            });

            self.last_search_query = Some(pattern.clone());
            self.buf_mut().search_pattern = Some(pattern);

            self.set_status_msg("replace (y/n/a/q)?", MessageKind::Info);

            if !self.advance_substitution() {
                self.finish_substitution();
            }
        } else {
            self.execute_substitution_all(&pattern, &replacement, global, start_row, end_row);
        }
    }

    /// Non-interactive: replace all matches immediately.
    fn execute_substitution_all(
        &mut self,
        pattern: &str,
        replacement: &str,
        global: bool,
        start_row: usize,
        end_row: usize,
    ) {
        let mut count = 0;
        let (win, buf) = self.active_window_and_buf_mut();
        buf.push_undo(win.row, win.col);

        for r in start_row..end_row {
            if r >= buf.len_lines() {
                break;
            }
            let line_text = buf.line_text(r);

            let mut matches_to_replace = Vec::new();
            let mut first_on_line = true;

            for (byte_pos, pat_str) in line_text.match_indices(pattern) {
                if first_on_line || global {
                    let char_pos = line_text[..byte_pos].chars().count();
                    let pat_len = pat_str.chars().count();
                    matches_to_replace.push((char_pos, pat_len));
                    first_on_line = false;
                }
            }

            for (char_pos, pat_len) in matches_to_replace.into_iter().rev() {
                let rope_start = buf.rope.line_to_char(r) + char_pos;
                let rope_end = rope_start + pat_len;
                buf.rope.remove(rope_start..rope_end);
                buf.rope.insert(rope_start, replacement);
                count += 1;
            }
        }

        buf.mark_modified();
        buf.parse_syntax();

        if count > 0 {
            self.set_status_msg(&format!("{} substitutions", count), MessageKind::Success);
        } else {
            self.set_status_msg("Pattern not found", MessageKind::Info);
        }
    }

    /// Find the next match and highlight it. Returns `false` if no more matches.
    fn advance_substitution(&mut self) -> bool {
        let (mut r, mut c, end_row, pattern, global) = {
            match &self.substitution_state {
                Some(s) => (
                    s.current_row,
                    s.current_col,
                    s.end_row,
                    s.pattern.clone(),
                    s.global,
                ),
                None => return false,
            }
        };

        while r < end_row {
            let line_text = self.buf().line_text(r);

            if !global && c > 0 {
                r += 1;
                c = 0;
                continue;
            }

            if let Some(abs_char_pos) = find_char_pos_from(&line_text, &pattern, c) {
                {
                    let win = self.active_window_mut();
                    win.row = r;
                    win.col = abs_char_pos;
                    win.desired_col = abs_char_pos;
                }
                self.scroll_active_window_to_cursor();

                self.last_search_query = Some(pattern.clone());
                self.buf_mut().search_pattern = Some(pattern.clone());

                self.set_status_msg("replace (y/n/a/q)?", MessageKind::Info);

                if let Some(ref mut state) = self.substitution_state {
                    state.current_row = r;
                    state.current_col = abs_char_pos;
                }

                return true;
            }

            r += 1;
            c = 0;
        }

        false
    }

    /// Interactive key handler for the substitution prompt.
    pub fn handle_substitution_key(&mut self, key: crate::event::KeyEvent) {
        match key.code {
            KeyCode::Char('y') => self.substitution_replace(),
            KeyCode::Char('n') => self.substitution_skip(),
            KeyCode::Char('a') => self.substitution_replace_all(),
            KeyCode::Char('q') | KeyCode::Esc => self.finish_substitution(),
            _ => {}
        }
    }

    /// Replace the current match and advance.
    fn substitution_replace(&mut self) {
        let (r, c, pattern, replacement, is_replace_all) = {
            match &self.substitution_state {
                Some(s) => (
                    s.current_row,
                    s.current_col,
                    s.pattern.clone(),
                    s.replacement.clone(),
                    s.replace_all,
                ),
                None => return,
            }
        };

        let pat_len_chars = pattern.chars().count();
        let start_char = self.buf().rope.line_to_char(r) + c;
        let end_char = start_char + pat_len_chars;

        let buf = self.buf_mut();
        buf.rope.remove(start_char..end_char);
        buf.rope.insert(start_char, &replacement);
        buf.mark_modified();
        buf.parse_syntax();

        if let Some(ref mut state) = self.substitution_state {
            state.replacements_made += 1;
            state.current_col = c + replacement.chars().count();
        }

        if is_replace_all {
            self.advance_substitution_or_finish();
        } else {
            if !self.advance_substitution() {
                self.finish_substitution();
            }
        }
    }

    /// Skip the current match and advance.
    fn substitution_skip(&mut self) {
        if let Some(ref mut state) = self.substitution_state {
            state.current_col += 1;
        }

        if !self.advance_substitution() {
            self.finish_substitution();
        }
    }

    /// Replace this and all remaining matches.
    fn substitution_replace_all(&mut self) {
        if let Some(ref mut state) = self.substitution_state {
            state.replace_all = true;
        }
        self.substitution_replace();
    }

    fn advance_substitution_or_finish(&mut self) {
        if !self.advance_substitution() {
            self.finish_substitution();
        }
    }

    /// End the substitution session and restore previous search state.
    pub fn finish_substitution(&mut self) {
        let count = self
            .substitution_state
            .as_ref()
            .map(|s| s.replacements_made)
            .unwrap_or(0);

        self.substitution_state = None;

        self.last_search_query = self.prev_search_query.take();
        self.buf_mut().search_pattern = self.last_search_query.clone();
        self.buf_mut().parse_syntax();

        self.set_status_msg(
            &format!("{} substitutions", count),
            if count > 0 {
                MessageKind::Success
            } else {
                MessageKind::Info
            },
        );
    }
}
