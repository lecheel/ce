use crate::ed::Mode;
use crate::event::{KeyCode, KeyModifiers};
use crate::Editor;

impl Editor {
    pub fn handle_llm_key(&mut self, key: crate::event::KeyEvent) -> bool {
        if matches!(self.mode, Mode::Insert) {
            return false;
        }

        match key.code {
            KeyCode::Char('>') if key.modifiers.is_empty() => {
                self.enter_command();
                for ch in "> ".chars() {
                    self.push_command(ch);
                }
                true
            }
            KeyCode::Char('q') if key.modifiers.is_empty() => {
                if self.windows.len() > 1 {
                    self.llm_close_split_session();
                } else {
                    self.llm_close_buffer();
                }
                true
            }
            _ => false,
        }
    }

    pub fn handle_llm_input_key(&mut self, key: crate::event::KeyEvent) -> bool {
        if self.handle_llm_key(key) {
            return true;
        }

        if key.code == KeyCode::Enter
            && !key.modifiers.contains(KeyModifiers::SHIFT)
            && !key.modifiers.contains(KeyModifiers::CONTROL)
        {
            self.llm_send_input_buffer();
            return true;
        }

        false
    }
}
