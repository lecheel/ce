use crate::Editor;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use std::path::PathBuf;

impl Editor {
    pub fn handle_file_picker_key(&mut self, key: KeyEvent) {
        // ── New File Input Mode ────────────────────────────────────────
        if let Some(picker) = &mut self.popup.file_picker {
            if picker.new_file_mode {
                match key.code {
                    KeyCode::Esc => {
                        picker.new_file_mode = false;
                        picker.new_file_input.clear();
                    }
                    KeyCode::Enter => {
                        let input = picker.new_file_input.trim().to_string();
                        if input.is_empty() {
                            picker.new_file_mode = false;
                            return;
                        }

                        let base_dir = picker.new_file_base_dir.clone();
                        self.popup.close(); // Close popup first

                        let mut target_path = base_dir;
                        target_path.push(&input);

                        // Ensure parent directories exist
                        if let Some(parent) = target_path.parent() {
                            let _ = std::fs::create_dir_all(parent);
                        }

                        // Create the file on disk if it doesn't exist
                        if !target_path.exists() {
                            if let Err(e) = std::fs::File::create(&target_path) {
                                self.set_status_msg(
                                    &format!("Failed to create file: {}", e),
                                    crate::ed::mode::MessageKind::Error,
                                );
                                return;
                            }
                        }

                        // Open the newly created file in a buffer
                        let path_str = target_path.to_string_lossy().to_string();
                        crate::repl::command::execute(self, &format!("e {}", path_str));
                        self.set_status_msg(
                            &format!("Created and opened {}", path_str),
                            crate::ed::mode::MessageKind::Success,
                        );
                    }
                    KeyCode::Backspace => {
                        picker.new_file_input.pop();
                    }
                    KeyCode::Char(c) => {
                        picker.new_file_input.push(c);
                    }
                    _ => {}
                }
                return; // Key fully handled
            }
        }

        // ── Delete Confirmation Mode ────────────────────────────────────
        if let Some(picker) = &mut self.popup.file_picker {
            if picker.delete_confirm_mode {
                match key.code {
                    KeyCode::Char('y') | KeyCode::Char('Y') => {
                        let path = picker.delete_target_path.clone();
                        if let Err(e) = std::fs::remove_file(&path) {
                            picker.last_error = Some(format!("Delete failed: {}", e));
                        }
                        picker.delete_confirm_mode = false;
                        picker.refresh_entries();
                    }
                    KeyCode::Esc | KeyCode::Char('n') | KeyCode::Char('N') => {
                        picker.delete_confirm_mode = false;
                    }
                    _ => {} // Ignore other keys while confirming
                }
                return; // Key fully handled
            }
        }

        // ── Normal Picker Mode ─────────────────────────────────────────
        if key.modifiers.contains(KeyModifiers::CONTROL) {
            if let Some(picker) = &mut self.popup.file_picker {
                match key.code {
                    KeyCode::Char('n') | KeyCode::Char('N') => picker.list.move_down(),
                    KeyCode::Char('p') | KeyCode::Char('P') => picker.list.move_up(),
                    _ => {}
                }
            }
            return;
        }

        // Determine the action to take based on the current state and key.
        // We extract all necessary data here so that we don't hold a mutable
        // borrow on `self.popup.file_picker` while trying to mutate `self`.
        enum PickerAction {
            Close(PathBuf),
            OpenFile(PathBuf),
            EnterDir(PathBuf),
            GoUp,
            ToggleFlat,
            GoHome,
            EnterNewFileMode(PathBuf, String),
            RequestDelete(PathBuf),
            FilterKey, // <-- Added to handle filter keys safely
            None,
        }

        let action = {
            let picker = match &mut self.popup.file_picker {
                Some(p) => p,
                None => return,
            };

            // Use the built-in dispatch_nav for Char, Backspace, Up, Down
            if picker.list.dispatch_nav(&key) {
                PickerAction::FilterKey
            } else {
                let selected_idx = picker.list.selected;
                let selected_data = if selected_idx < picker.list.filtered.len() {
                    picker
                        .list
                        .filtered
                        .get(selected_idx)
                        .and_then(|&real_idx| picker.list.entries.get(real_idx))
                        .map(|e| (e.is_dir, e.is_parent, e.path.clone()))
                } else {
                    None
                };

                match key.code {
                    KeyCode::Esc => PickerAction::Close(picker.cwd.clone()),
                    KeyCode::Enter | KeyCode::Right => {
                        if let Some((is_dir, _, path)) = selected_data {
                            if is_dir {
                                PickerAction::EnterDir(path)
                            } else {
                                PickerAction::OpenFile(path)
                            }
                        } else {
                            PickerAction::None
                        }
                    }
                    KeyCode::Left => {
                        if picker.list.filter.is_empty() {
                            PickerAction::GoUp
                        } else {
                            PickerAction::None
                        }
                    }
                    KeyCode::Insert => {
                        let mut base_dir = picker.cwd.clone();
                        let selected_idx = picker.list.selected;
                        if selected_idx < picker.list.filtered.len() {
                            if let Some(&real_idx) = picker.list.filtered.get(selected_idx) {
                                if let Some(entry) = picker.list.entries.get(real_idx) {
                                    if entry.is_dir && !entry.is_parent {
                                        base_dir = entry.path.clone();
                                    }
                                }
                            }
                        }
                        PickerAction::EnterNewFileMode(base_dir, picker.list.filter.clone())
                    }
                    KeyCode::Delete => {
                        if let Some((_, is_parent, path)) = selected_data {
                            if !is_parent {
                                PickerAction::RequestDelete(path)
                            } else {
                                PickerAction::None
                            }
                        } else {
                            PickerAction::None
                        }
                    }
                    KeyCode::Tab => PickerAction::ToggleFlat,
                    KeyCode::Home => PickerAction::GoHome,
                    _ => PickerAction::None,
                }
            }
        };

        match action {
            PickerAction::Close(cwd) => {
                self.popup.last_file_picker_dir = Some(cwd);
                self.popup.close();
            }
            PickerAction::OpenFile(path) => {
                let cwd = self.popup.file_picker.as_ref().map(|p| p.cwd.clone());
                self.popup.last_file_picker_dir = cwd;
                self.popup.close();
                self.open_buffer(Some(path.to_string_lossy().to_string()));
            }
            PickerAction::EnterDir(path) => {
                if let Some(picker) = &mut self.popup.file_picker {
                    picker.go_into(&path);
                }
            }
            PickerAction::GoUp => {
                if let Some(picker) = &mut self.popup.file_picker {
                    picker.go_up();
                }
            }
            PickerAction::ToggleFlat => {
                if let Some(picker) = &mut self.popup.file_picker {
                    picker.toggle_flat();
                }
            }
            PickerAction::GoHome => {
                if let Some(picker) = &mut self.popup.file_picker {
                    picker.go_home();
                }
            }
            PickerAction::EnterNewFileMode(base_dir, filter) => {
                if let Some(picker) = &mut self.popup.file_picker {
                    picker.new_file_mode = true;
                    picker.new_file_input = filter;
                    picker.new_file_base_dir = base_dir;
                }
            }
            PickerAction::RequestDelete(path) => {
                if let Some(picker) = &mut self.popup.file_picker {
                    picker.delete_confirm_mode = true;
                    picker.delete_target_path = path;
                }
            }
            PickerAction::FilterKey | PickerAction::None => {}
        }
    }
}
