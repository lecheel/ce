use crate::event::KeyCode;
use crate::popup::dispatch_list_nav;
use crate::Editor;

impl Editor {
    pub fn open_mru_popup(&mut self, repo_only: bool) {
        let repo_root = if repo_only {
            self.buf().filename.as_ref().and_then(|f| {
                let canonical = std::path::Path::new(f).canonicalize().ok()?;
                let mut current = canonical.as_path();
                while let Some(parent) = current.parent() {
                    if parent.join(".git").exists() {
                        return Some(parent.to_path_buf());
                    }
                    current = parent;
                }
                None
            })
        } else {
            None
        };

        let entries = self.mru_manager.get_entries();
        let mut popup = crate::popup::mru::MruPopup::new(entries, repo_root, repo_only);
        popup.list.apply_filter();
        self.popup.mru = Some(popup);
    }

    pub fn handle_mru_key(&mut self, key: crate::event::KeyEvent) {
        if dispatch_list_nav(&mut self.popup.mru, &key) {
            return;
        }

        let selected_entry = self
            .popup
            .mru
            .as_ref()
            .and_then(|p| p.selected_entry().cloned());

        match key.code {
            KeyCode::Esc => {
                self.popup.close();
            }
            KeyCode::Home => {
                if let Some(ref mut mru) = self.popup.mru {
                    mru.toggle_sort(&self.mru_manager);
                }
            }
            KeyCode::Tab => {
                if let Some(ref mut mru) = self.popup.mru {
                    mru.toggle_repo_filter();
                }
            }
            KeyCode::Delete => {
                if let Some(ref mut mru) = self.popup.mru {
                    mru.remove_selected(&mut self.mru_manager);
                }
            }
            KeyCode::Enter => {
                self.popup.close();
                if let Some(entry) = selected_entry {
                    let path_str = entry.path.to_string_lossy().to_string();
                    self.open_buffer(Some(path_str));
                    let (win, buf) = self.active_window_and_buf_mut();
                    win.row = entry.line.min(buf.len_lines().saturating_sub(1));
                    win.col = entry.col.min(buf.line_char_len(win.row));
                    self.scroll_active_window_to_cursor();
                }
            }
            _ => {}
        }
    }
}
