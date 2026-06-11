use crate::ed::MessageKind;
use crate::event::KeyEvent;
use crate::Editor;
use crossterm::event::KeyCode;

impl Editor {
    pub fn handle_ripgrep_key(&mut self, key: KeyEvent) -> bool {
        // ── wgrep mode normal-mode keys ──
        if self.buf().wgrep_mode {
            return self.handle_wgrep_normal_key(key);
        }

        // ── Standard ripgrep buffer keys ──
        match key.code {
            KeyCode::Enter => {
                self.ripgrep_goto_result();
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

    fn handle_wgrep_normal_key(&mut self, key: KeyEvent) -> bool {
        match key.code {
            KeyCode::Char('q') => {
                if self.buf().modified {
                    self.set_status_msg(
                        "Unsaved changes — w to apply, q! to discard",
                        MessageKind::Error,
                    );
                } else {
                    self.ripgrep_close_buffer();
                    // self.wgrep_exit();
                }
                true
            }
            KeyCode::Char('!') => {
                if self.buf().wgrep_mode {
                    self.wgrep_force_exit();
                }
                true
            }
            KeyCode::Char('w') => {
                self.wgrep_apply();
                true
            }
            // Let movement and other normal-mode keys fall through
            _ => false,
        }
    }
}
