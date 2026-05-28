use crate::ed::guide::Guide;
use crate::ed::MessageKind;
use crate::event::KeyCode;
use crate::popup::dispatch_list_nav;
use crate::Editor;

impl Editor {
    pub fn open_guide_popup(&mut self) {
        let guide = Guide::load();
        self.popup.guide = Some(crate::popup::guide::GuidePopup::new(guide.entries));
    }

    pub fn handle_guide_popup_key(&mut self, key: crate::event::KeyEvent) {
        if dispatch_list_nav(&mut self.popup.guide, &key) {
            return;
        }

        match key.code {
            KeyCode::Esc => {
                self.popup.guide = None;
            }

            KeyCode::Enter => {
                let entry_data = self
                    .popup
                    .guide
                    .as_ref()
                    .and_then(|p| p.selected_entry().cloned());
                self.popup.guide = None;

                if let Some(entry) = entry_data {
                    let target_path = std::path::PathBuf::from(&entry.file);
                    let abs_path = crate::git::gutter::find_git_root(std::path::Path::new("."))
                        .unwrap_or_else(|| std::path::PathBuf::from("."))
                        .join(&target_path);

                    let bid = self
                        .buffers
                        .iter()
                        .find(|b| b.filename.as_deref() == Some(abs_path.to_str().unwrap_or("")))
                        .map(|b| b.id);

                    if let Some(id) = bid {
                        self.switch_window_to_buffer(id);
                    } else {
                        self.open_buffer(Some(abs_path.to_string_lossy().to_string()));
                    }

                    let source = self.buf().rope.to_string();
                    if let Some(line) = Guide::find_anchor_line(&source, &entry.anchor) {
                        let win = self.active_window_mut();
                        win.row = line;
                        win.col = 0;
                        win.desired_col = 0;
                        self.center_viewport_on_cursor();
                    } else {
                        self.set_status_msg(
                            &format!("Anchor not found: {}", entry.anchor),
                            MessageKind::Error,
                        );
                    }
                }
            }

            _ => {}
        }
    }
}
