//! Key handling for the fd (fuzzy-find) popup.

use crate::event::KeyCode;
use crate::popup::dispatch_list_nav;
use crate::Editor;

impl Editor {
    pub fn handle_fd_key(&mut self, key: crate::event::KeyEvent) {
        if key.kind != crate::event::KeyEventKind::Press {
            return;
        }

        // Ctrl combinations
        if key.modifiers.contains(crate::event::KeyModifiers::CONTROL) {
            match key.code {
                KeyCode::Char('n') | KeyCode::Char('N') => {
                    if let Some(popup) = &mut self.popup.fd {
                        popup.list.move_down();
                    }
                    return;
                }
                KeyCode::Char('p') | KeyCode::Char('P') => {
                    if let Some(popup) = &mut self.popup.fd {
                        popup.list.move_up();
                    }
                    return;
                }
                KeyCode::Char('u') | KeyCode::Char('U') => {
                    if let Some(popup) = &mut self.popup.fd {
                        popup.list.filter_clear();
                    }
                    return;
                }
                _ => return,
            }
        }

        // Delegate standard list navigation (arrows, page-up/down, char filter)
        if dispatch_list_nav(&mut self.popup.fd, &key) {
            return;
        }

        match key.code {
            KeyCode::Esc => {
                self.popup.close();
            }

            KeyCode::Enter => {
                let path_opt = self
                    .popup
                    .fd
                    .as_ref()
                    .and_then(|p| p.selected_entry())
                    .map(|e| e.path.clone());

                self.popup.close();

                if let Some(path) = path_opt {
                    let path_str = path.to_string_lossy().to_string();
                    self.open_buffer(Some(path_str));
                }
            }

            _ => {}
        }
    }
}
