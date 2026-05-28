use crate::ed::MessageKind;
use crate::event::KeyCode;
use crate::popup::dispatch_list_nav;
use crate::Editor;

impl Editor {
    pub fn handle_function_list_key(&mut self, key: crate::event::KeyEvent) {
        if dispatch_list_nav(&mut self.popup.function_list, &key) {
            return;
        }

        match key.code {
            KeyCode::Esc => {
                self.popup.function_list = None;
            }

            KeyCode::Enter => {
                let target_line = self
                    .popup
                    .function_list
                    .as_ref()
                    .and_then(|p| p.selected_entry().map(|e| e.line));
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

            _ => {}
        }
    }
}
