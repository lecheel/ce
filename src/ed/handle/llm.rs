use crate::ed::buffer::BufferKind;
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
            return false; // regular Enter → newline via normal insert path
        }

        match key.code {
            KeyCode::Char('q') if key.modifiers.is_empty() => {
                if self.windows.len() > 1 {
                    self.llm_close_split_session();
                } else {
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
}
