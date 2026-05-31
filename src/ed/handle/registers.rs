//! Key handler for the registers popup (informational overlay).

use crate::ed::misc_helper::is_valid_register_char;
use crate::event::KeyCode;
use crate::Editor;

impl Editor {
    pub fn handle_registers_key(&mut self, key: crate::event::KeyEvent) {
        // ── Register prefix selection flow ──────────────────────────
        // If we just pressed `"` to open the popup, pressing a register char
        // selects it for the next command and closes the popup.
        if self.normal_register_prefix == Some('"') {
            match key.code {
                KeyCode::Char(c) if is_valid_register_char(c) => {
                    self.normal_register_prefix = Some(c);
                    self.popup.close(); // Use close() to clear both data and kind
                    self.clear_status_msg();
                }
                KeyCode::Esc => {
                    self.normal_register_prefix = None;
                    self.popup.close();
                    self.clear_status_msg();
                }
                _ => {
                    self.normal_register_prefix = None;
                    self.popup.close();
                    self.clear_status_msg();
                }
            }
            return;
        }

        // ── Default: popup open without prefix state (defensive) ───
        match key.code {
            KeyCode::Esc => {
                self.normal_register_prefix = None;
                self.popup.close();
                self.clear_status_msg();
            }
            KeyCode::Char(c) if is_valid_register_char(c) => {
                self.normal_register_prefix = Some(c);
                self.popup.close();
                self.clear_status_msg();
            }
            _ => {
                self.normal_register_prefix = None;
                self.popup.close();
                self.clear_status_msg();
            }
        }
    }
}
