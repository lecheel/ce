use crate::event::KeyCode;
use crate::popup::dispatch_list_nav;
use crate::Editor;

impl Editor {
    pub fn handle_file_picker_key(&mut self, key: crate::event::KeyEvent) {
        if dispatch_list_nav(&mut self.popup.file_picker, &key) {
            return;
        }

        let selected_entry = self
            .popup
            .file_picker
            .as_ref()
            .and_then(|p| p.selected_entry().cloned());

        match key.code {
            KeyCode::Esc => {
                let cwd = self.popup.file_picker.as_ref().map(|p| p.cwd.clone());
                self.popup.last_file_picker_dir = cwd;
                self.popup.close();
            }

            KeyCode::Enter | KeyCode::Right => {
                if let Some(entry) = selected_entry {
                    if entry.is_dir {
                        if let Some(picker) = &mut self.popup.file_picker {
                            picker.go_into(&entry.path);
                        }
                    } else {
                        let cwd = self.popup.file_picker.as_ref().map(|p| p.cwd.clone());
                        self.popup.last_file_picker_dir = cwd;
                        self.popup.close();
                        self.open_buffer(Some(entry.path.to_string_lossy().to_string()));
                    }
                }
            }

            KeyCode::Backspace | KeyCode::Left => {
                if let Some(picker) = &mut self.popup.file_picker {
                    if picker.list.filter.is_empty() {
                        picker.go_up();
                    }
                    // else handled by dispatch_list_nav's filter_pop
                }
            }

            KeyCode::Tab => {
                if let Some(picker) = &mut self.popup.file_picker {
                    picker.toggle_flat();
                }
            }

            _ => {}
        }
    }
}
