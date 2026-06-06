use crate::event::KeyEvent;
use crate::Editor;
use crossterm::event::KeyCode;

impl Editor {
    pub fn handle_ripgrep_key(&mut self, key: KeyEvent) -> bool {
        match key.code {
            KeyCode::Enter => {
                self.ripgrep_goto_result();
                // ── Request git gutter for the target file ──
                let bid = self.buf().id;
                let rope = self.buf().rope.clone();
                if let Some(ref name) = self.buf().filename {
                    self.async_gutter.request_diff(bid, &rope, Some(name));
                }
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
