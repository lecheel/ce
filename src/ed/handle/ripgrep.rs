use crate::event::KeyEvent;
use crate::Editor;
use crossterm::event::KeyCode;

impl Editor {
    pub fn handle_ripgrep_key(&mut self, key: KeyEvent) -> bool {
        match key.code {
            KeyCode::Enter => {
                self.ripgrep_goto_result();
                true
            }
            KeyCode::Char('q') => {
                self.ripgrep_close_buffer();
                true
            }
            _ => false,
        }
    }
}
