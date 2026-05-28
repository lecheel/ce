use crate::event::KeyCode;
use crate::Editor;

impl Editor {
    pub fn handle_marks_key(&mut self, key: crate::event::KeyEvent) {
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
                        self.jump_to_mark(entry);
                    }
                }
            }
            KeyCode::Char(c) if c.is_ascii_lowercase() || c == '`' => {
                if let Some(ref p) = self.popup.marks {
                    if let Some(entry) = p.entries.iter().find(|e| e.ch == c).cloned() {
                        self.popup.close();
                        self.jump_to_mark(entry);
                    }
                }
            }
            _ => {}
        }
    }

    fn jump_to_mark(&mut self, entry: crate::popup::marks::MarkEntry) {
        if entry.ch == '`' {
            self.jump_last_position();
            return;
        }

        self.active_window_mut().save_jump_position();

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
