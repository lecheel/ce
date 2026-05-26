use crate::event::KeyEvent;
use crate::Editor;

impl Editor {
    /// Opens the MRU popup overlay, auto-detecting the Git repository root of the active file.
    pub fn open_mru_popup(&mut self) {
        let repo_root = self.buf().filename.as_ref().and_then(|f| {
            let canonical = std::path::Path::new(f).canonicalize().ok()?;
            let mut current = canonical.as_path();
            while let Some(parent) = current.parent() {
                if parent.join(".git").exists() {
                    return Some(parent.to_path_buf());
                }
                current = parent;
            }
            None
        });

        let entries = self.mru_manager.get_entries();
        let mut mru_popup = crate::popup::mru::MruPopup::new(entries, repo_root);
        mru_popup.apply_filter();
        self.popup.mru = Some(mru_popup);
    }

    /// Handles keys specifically when the MRU popup is focused.
    pub fn handle_mru_key(&mut self, key: KeyEvent) {
        use crossterm::event::KeyCode;

        // Retrieve selected entry upfront to bypass borrow-checker limitations
        let selected_entry = self
            .popup
            .mru
            .as_ref()
            .and_then(|p| p.selected_entry().cloned());

        match key.code {
            KeyCode::Esc => {
                self.popup.mru = None;
            }

            KeyCode::Up => {
                if let Some(ref mut mru) = self.popup.mru {
                    mru.move_up();
                }
            }

            KeyCode::Down => {
                if let Some(ref mut mru) = self.popup.mru {
                    mru.move_down();
                }
            }

            // Toggles sorting between recency and frequency frequency
            KeyCode::Home => {
                if let Some(ref mut mru) = self.popup.mru {
                    mru.toggle_sort(&self.mru_manager);
                }
            }

            // Toggles restricting results to the active repository root
            KeyCode::Tab => {
                if let Some(ref mut mru) = self.popup.mru {
                    mru.toggle_repo_filter();
                }
            }

            // Deletes the highlighted file from MRU lists
            KeyCode::Delete => {
                if let Some(ref mut mru) = self.popup.mru {
                    mru.remove_selected(&mut self.mru_manager);
                }
            }

            // Selects and opens the file
            KeyCode::Enter => {
                self.popup.mru = None;
                if let Some(entry) = selected_entry {
                    let path_str = entry.path.to_string_lossy().to_string();
                    self.open_buffer(Some(path_str));

                    let (win, buf) = self.active_window_and_buf_mut();
                    win.row = entry.line.min(buf.len_lines().saturating_sub(1));
                    win.col = entry.col.min(buf.line_char_len(win.row));
                    self.scroll_active_window_to_cursor();
                }
            }

            KeyCode::Backspace => {
                if let Some(ref mut mru) = self.popup.mru {
                    mru.filter_pop();
                }
            }

            KeyCode::Char(c) => {
                if let Some(ref mut mru) = self.popup.mru {
                    mru.filter_push(c);
                }
            }

            _ => {}
        }
    }
}
