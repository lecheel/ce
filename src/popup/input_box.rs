//! Lightweight top-anchored input box popup.

#[derive(Debug, Clone)]
pub struct InputBox {
    /// Title shown in the top border.
    pub title: String,
    /// Static prompt label (e.g. "Find:").
    pub prompt: String,
    /// Current user-typed text.
    pub input: String,
    /// Byte-oriented cursor position inside `input`.
    pub cursor: usize,
    /// One-line hint shown in the bottom border.
    pub hint: String,
    /// Maximum input length (0 = unlimited).
    pub max_len: usize,
}

impl InputBox {
    pub fn new(title: impl Into<String>, prompt: impl Into<String>) -> Self {
        Self {
            title: title.into(),
            prompt: prompt.into(),
            input: String::new(),
            cursor: 0,
            hint: "[Esc] cancel".into(),
            max_len: 0,
        }
    }

    pub fn with_hint(mut self, hint: impl Into<String>) -> Self {
        self.hint = hint.into();
        self
    }

    pub fn with_max_len(mut self, max: usize) -> Self {
        self.max_len = max;
        self
    }

    pub fn with_default(mut self, text: impl Into<String>) -> Self {
        self.input = text.into();
        self.cursor = self.input.len();
        self
    }

    // ── Editing helpers ──────────────────────────────────────────

    pub fn insert_char(&mut self, ch: char) {
        if self.max_len > 0 && self.input.chars().count() >= self.max_len {
            return;
        }
        self.input.insert(self.cursor, ch);
        self.cursor += ch.len_utf8();
    }

    pub fn insert_newline(&mut self) {
        if self.max_len > 0 && self.input.chars().count() >= self.max_len {
            return;
        }
        self.input.insert(self.cursor, '\n');
        self.cursor += 1;
    }

    pub fn backspace(&mut self) {
        if self.cursor == 0 {
            return;
        }
        let prev = self.input[..self.cursor]
            .char_indices()
            .last()
            .map(|(i, _)| i)
            .unwrap_or(0);
        self.input.drain(prev..self.cursor);
        self.cursor = prev;
    }

    pub fn delete_char(&mut self) {
        if self.cursor >= self.input.len() {
            return;
        }
        let next = self.input[self.cursor..]
            .char_indices()
            .nth(1)
            .map(|(i, _)| self.cursor + i)
            .unwrap_or(self.input.len());
        self.input.drain(self.cursor..next);
    }

    pub fn move_left(&mut self) {
        if self.cursor > 0 {
            self.cursor = self.input[..self.cursor]
                .char_indices()
                .last()
                .map(|(i, _)| i)
                .unwrap_or(0);
        }
    }

    pub fn move_right(&mut self) {
        if self.cursor < self.input.len() {
            self.cursor = self.input[self.cursor..]
                .char_indices()
                .nth(1)
                .map(|(i, _)| self.cursor + i)
                .unwrap_or(self.input.len());
        }
    }

    pub fn move_up(&mut self) {
        let before_cursor = &self.input[..self.cursor];
        let current_line_start = before_cursor.rfind('\n').map(|i| i + 1).unwrap_or(0);
        let char_col = before_cursor[current_line_start..].chars().count();

        if current_line_start == 0 {
            // Already on the first line, move to start
            self.cursor = 0;
            return;
        }

        let prev_line_end = current_line_start - 1; // The '\n' character
        let prev_lines = &self.input[..prev_line_end];
        let prev_line_start = prev_lines.rfind('\n').map(|i| i + 1).unwrap_or(0);

        let prev_line_text = &self.input[prev_line_start..prev_line_end];
        let target_col = char_col.min(prev_line_text.chars().count());

        let byte_offset = prev_line_text
            .chars()
            .take(target_col)
            .collect::<String>()
            .len();
        self.cursor = prev_line_start + byte_offset;
    }

    pub fn move_down(&mut self) {
        let before_cursor = &self.input[..self.cursor];
        let current_line_start = before_cursor.rfind('\n').map(|i| i + 1).unwrap_or(0);
        let char_col = before_cursor[current_line_start..].chars().count();

        let rest = &self.input[self.cursor..];
        let next_line_start = rest.find('\n').map(|i| self.cursor + i + 1);

        let Some(next_start) = next_line_start else {
            // Already on the last line, move to end
            self.cursor = self.input.len();
            return;
        };

        let next_rest = &self.input[next_start..];
        let next_line_end = next_rest
            .find('\n')
            .map(|i| next_start + i)
            .unwrap_or(self.input.len());

        let next_line_text = &self.input[next_start..next_line_end];
        let target_col = char_col.min(next_line_text.chars().count());

        let byte_offset = next_line_text
            .chars()
            .take(target_col)
            .collect::<String>()
            .len();
        self.cursor = next_start + byte_offset;
    }

    pub fn move_home(&mut self) {
        // Move to start of current line
        let line_start = self.input[..self.cursor]
            .rfind('\n')
            .map(|i| i + 1)
            .unwrap_or(0);
        self.cursor = line_start;
    }

    pub fn move_end(&mut self) {
        // Move to end of current line
        let line_end = self.input[self.cursor..]
            .find('\n')
            .map(|i| self.cursor + i)
            .unwrap_or(self.input.len());
        self.cursor = line_end;
    }

    pub fn kill_to_end(&mut self) {
        let line_end = self.input[self.cursor..]
            .find('\n')
            .map(|i| self.cursor + i)
            .unwrap_or(self.input.len());
        self.input.drain(self.cursor..line_end);
    }
}
