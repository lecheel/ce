//--+ ed/handle/llm.rs
use crate::ed::buffer::BufferKind;
use crate::ed::MessageKind;
use crate::Editor;
use crossterm::event::KeyModifiers;

impl Editor {
    /// Unified key handler for both Llm (history) and LlmInput buffers.
    /// Called from handle_key when buf_kind is Llm or LlmInput.
    /// Returns true if the key was consumed.
    pub fn handle_llm_buffer_key(&mut self, key: crate::event::KeyEvent) -> bool {
        use crate::ed::Mode;
        use crate::event::KeyCode;

        let is_input_buf = self.buf().kind == BufferKind::LlmInput;

        // Insert/Brief in the input buffer: intercept send shortcuts only
        if matches!(self.mode, Mode::Insert | Mode::Brief) && is_input_buf {
            if key.code == KeyCode::Enter
                && (key.modifiers.contains(KeyModifiers::CONTROL)
                    || key.modifiers.contains(KeyModifiers::SHIFT))
            {
                self.llm_send_input_buffer();
                return true;
            }
            return false; // regular typing falls through to normal insert
        }

        match key.code {
            KeyCode::Char('l') if key.modifiers.is_empty() && !is_input_buf => {
                // Jump to next "User:" input in LLM history
                self.llm_jump_to_marker("User:", true);
                true
            }
            KeyCode::Char('L') => {
                // Jump to previous "User:" input in LLM history (Shift+L)
                if !is_input_buf {
                    self.llm_jump_to_marker("User:", false);
                    true
                } else {
                    false
                }
            }
            KeyCode::Char('q') if key.modifiers.is_empty() => {
                // 1. Close the current split pane if we are in a split layout
                if self.windows.len() > 1 {
                    self.llm_close_split_session();
                }

                // 2. If closing the split left the partner LLM/LlmInput buffer
                // as the active full-screen buffer, close it too.
                // (If `llm_close_split_session` already closed both, this safely does nothing)
                if self.buf().kind == BufferKind::Llm || self.buf().kind == BufferKind::LlmInput {
                    self.llm_close_buffer();
                }

                true
            }
            KeyCode::Enter if key.modifiers.is_empty() && is_input_buf => {
                self.llm_send_input_buffer();
                true
            }
            // Consume Enter in the history pane — don't let it fall through
            // to normal movement which panics on the short rope
            KeyCode::Enter if key.modifiers.is_empty() && !is_input_buf => true,
            KeyCode::Char('>') if key.modifiers.is_empty() => {
                self.enter_command();
                for ch in "> ".chars() {
                    self.push_command(ch);
                }
                true
            }
            _ => false,
        }
    }

    /// Jump to the next (or previous) line starting with `pattern` in the
    /// active LLM / CodeLlm buffer. Used by `l` / `L` navigation.
    fn llm_jump_to_marker(&mut self, pattern: &str, forward: bool) {
        let current_row = self.active_window().row;
        let target_row = {
            let buf = self.buf();
            let total = buf.len_lines();
            if forward {
                (current_row + 1..total)
                    .find(|&row| buf.line_text(row).trim_start().starts_with(pattern))
            } else {
                (0..current_row)
                    .rev()
                    .find(|&row| buf.line_text(row).trim_start().starts_with(pattern))
            }
        };
        match target_row {
            Some(row) => {
                self.active_window_mut().row = row;
                self.active_window_mut().col = 0;
                self.active_window_mut().desired_col = 0;
                self.center_viewport_on_cursor();
            }
            None => {
                let msg = if forward {
                    "No next user input"
                } else {
                    "No previous user input"
                };
                self.set_status_msg(msg, MessageKind::Info);
            }
        }
    }

    // Keep the old names as thin shims so no other callsite breaks during migration.
    // Remove once all callers are updated.
    pub fn handle_llm_key(&mut self, key: crate::event::KeyEvent) -> bool {
        self.handle_llm_buffer_key(key)
    }
    pub fn handle_llm_input_key(&mut self, key: crate::event::KeyEvent) -> bool {
        self.handle_llm_buffer_key(key)
    }

    pub fn clamp_window_row_to_buf(&mut self, win_idx: usize) {
        let bid = self.windows[win_idx].buffer_id();
        if let Some(buf) = self.buffers.iter().find(|b| b.id == bid) {
            let max_row = buf.len_lines().saturating_sub(1);
            let win = &mut self.windows[win_idx];
            if win.row > max_row {
                win.row = max_row;
                win.col = 0;
                win.desired_col = 0;
            }
        }
    }
    // ── CodeLlm Insert/Brief mode intercept ────────────────
    /// Called from handle_key BEFORE the normal Insert-mode
    /// action dispatch.  Returns true if the key was consumed.
    pub fn handle_codellm_insert_key(&mut self, key: crate::event::KeyEvent) -> bool {
        use crate::event::KeyCode;

        let lock_line = self.buf().llm_lock_line;
        let cursor_row = self.active_window().row;

        // Ctrl+Enter or Shift+Enter → send prompt
        if key.code == KeyCode::Enter
            && (key.modifiers.contains(KeyModifiers::CONTROL)
                || key.modifiers.contains(KeyModifiers::SHIFT))
        {
            self.codellm_send();
            return true;
        }

        // If cursor somehow drifted above the lock line,
        // snap it back and consume the key
        if cursor_row < lock_line {
            let total = self.buf().len_lines();
            self.active_window_mut().row = lock_line.min(total.saturating_sub(1));
            self.active_window_mut().col = 0;
            self.active_window_mut().desired_col = 0;
            return true;
        }

        // Backspace at the lock line boundary: don't let the user
        // delete into the locked history area
        if key.code == KeyCode::Backspace
            && cursor_row == lock_line
            && self.active_window().col == 0
        {
            return true; // consume — block deletion of the header
        }

        // Everything else: allow normal insert processing
        false
    }

    // ── CodeLlm Normal-mode key handler ────────────────────
    /// Called from the special-buffer dispatch in handle_key.
    pub fn handle_codellm_key(&mut self, key: crate::event::KeyEvent) -> bool {
        use crate::ed::Mode;
        use crate::event::KeyCode;

        // Insert/Brief mode is handled by handle_codellm_insert_key
        // (called earlier in handle_key).  We should never get here
        // in Insert mode, but guard just in case.
        if matches!(self.mode, Mode::Insert | Mode::Brief) {
            return false;
        }

        match key.code {
            // q or Esc → close the CodeLlm buffer
            KeyCode::Char('q') if key.modifiers.is_empty() => {
                self.close_buffer();
                true
            }

            // Enter in Normal mode → send prompt
            KeyCode::Enter if key.modifiers.is_empty() => {
                self.codellm_send();
                true
            }

            KeyCode::Char('l') if key.modifiers.is_empty() => {
                self.llm_jump_to_marker("## You", true);
                true
            }
            KeyCode::Char('L') => {
                self.llm_jump_to_marker("## You", false);
                true
            }

            // i / a → enter insert at the prompt area
            KeyCode::Char('i') | KeyCode::Char('a') if key.modifiers.is_empty() => {
                let lock_line = self.buf().llm_lock_line;
                let total = self.buf().len_lines();
                let current_row = self.active_window().row;

                // Determine the target row first (immutable borrows)
                let target_row = if current_row < lock_line {
                    lock_line.min(total.saturating_sub(1))
                } else {
                    current_row
                };

                let col = self.buf().line_char_len(target_row);

                // Apply using a single mutable borrow
                let win = self.active_window_mut();
                win.row = target_row;
                win.col = col;
                win.desired_col = col;

                self.enter_insert();
                true
            }

            // Allow hjkl / movement keys to scroll history,
            // but clamp cursor so it can't sit on locked lines
            _ => {
                // Let normal-mode movement fall through, but after
                // the action executes, clamp the cursor
                false
            }
        }
    }
    /// After any Normal-mode action in a CodeLlm buffer, ensure
    /// the cursor isn't sitting on a locked line (unless just
    /// browsing).  Called from the end of handle_key.
    pub fn clamp_codellm_cursor(&mut self) {
        if self.buf().kind != BufferKind::CodeLlm {
            return;
        }
        // In Normal mode on CodeLlm, allow the cursor anywhere
        // for reading, but when entering Insert it will be
        // snapped to the editable zone.
    }

    /// Translate the current line (Chinese ↔ English) via LLM.
    /// Result is shown in the infobar and stored in register `"z"`.
    /// Reuses the existing async LLM infrastructure — no separate HTTP client needed.
    pub fn trans_zh_line(&mut self) {
        let row = self.active_window().row;
        if row >= self.buf().len_lines() {
            self.set_status_msg("No line to translate", MessageKind::Error);
            return;
        }

        let line_text = self.buf().line_text(row);
        let trimmed = line_text
            .trim_end_matches('\n')
            .trim_end_matches('\r')
            .trim();

        if trimmed.is_empty() {
            self.set_status_msg("Empty line, nothing to translate", MessageKind::Error);
            return;
        }

        // Detect if the line actually contains CJK characters
        let has_cjk = trimmed.chars().any(|c| {
            ('\u{4E00}'..='\u{9FFF}').contains(&c)
                || ('\u{3400}'..='\u{4DBF}').contains(&c)
                || ('\u{F900}'..='\u{FAFF}').contains(&c)
                || ('\u{FF00}'..='\u{FFEF}').contains(&c)
        });

        let prompt = if has_cjk {
            format!(
                "Translate the following Chinese text to English. \
                 Output ONLY the English translation, no explanations, no quotes:\n\n{}",
                trimmed
            )
        } else {
            format!(
                "Translate the following English text to Chinese (Simplified). \
                 Output ONLY the Chinese translation, no explanations, no quotes:\n\n{}",
                trimmed
            )
        };

        // Set the infobar flag so poll_llm_responses routes to infobar + register "z"
        self.llm.infobar_response = true;
        self.llm.infobar_accumulator.clear();

        let messages = vec![
            (
                "system".to_string(),
                "You are a translator. Output ONLY the translation, nothing else.".to_string(),
            ),
            ("user".to_string(), prompt),
        ];

        let backend = self.config.llm_backend;
        self.spawn_llm_request_with_backend(messages, backend);

        self.set_status_msg("Translating…", MessageKind::Info);
    }
}
