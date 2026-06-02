// ed/handle/quickfix.rs
//! Key handler for the Quickfix popup.

use crate::ed::mode::MessageKind;
use crate::event::KeyCode;
use crate::Editor;

impl Editor {
    /// Key handler for the Quickfix popup.
    pub fn handle_quickfix_key(&mut self, key: crate::event::KeyEvent) {
        match key.code {
            KeyCode::Esc => {
                self.popup.close();
            }
            KeyCode::Up | KeyCode::Char('k') | KeyCode::Char('p') => {
                if let Some(ref mut p) = self.popup.quickfix {
                    p.move_up();
                }
            }
            KeyCode::Down | KeyCode::Char('j') | KeyCode::Char('n') => {
                if let Some(ref mut p) = self.popup.quickfix {
                    p.move_down();
                }
            }
            KeyCode::Enter => {
                let selected = self
                    .popup
                    .quickfix
                    .as_ref()
                    .map(|p| p.selected)
                    .unwrap_or(0);

                self.popup.close();
                self.quickfix_jump_to(selected);
            }
            _ => {}
        }
    }

    /// Open the quickfix popup from the current ripgrep results.
    pub fn open_quickfix_popup(&mut self) {
        if self.quickfix_results.is_empty() {
            self.set_status_msg("No quickfix results. Run :rg first.", MessageKind::Error);
            return;
        }

        let root_dir = self.last_rg_root_dir.clone().unwrap_or_else(|| {
            std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."))
        });

        self.popup.quickfix = Some(crate::popup::quickfix::QuickfixPopup::new(
            self.quickfix_results.clone(),
            root_dir,
        ));
        self.popup.kind = Some(crate::popup::PopupKind::Quickfix);

        // Set selected to current quickfix_index if valid
        if let Some(ref mut p) = self.popup.quickfix {
            p.selected = self.quickfix_index.min(p.entries.len().saturating_sub(1));
        }
    }

    /// Jump to a specific quickfix result by index.
    fn quickfix_jump_to(&mut self, index: usize) {
        let result = match self.quickfix_results.get(index).cloned() {
            Some(r) => r,
            None => return,
        };

        self.quickfix_index = index;
        self.open_file_at_line(&result.file_path, result.line_number);

        let display_path = result
            .file_path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("?");

        self.set_status_msg(
            &format!(
                "Quickfix {}/{}: {}:{}",
                index + 1,
                self.quickfix_results.len(),
                display_path,
                result.line_number,
            ),
            MessageKind::Info,
        );
    }
}
