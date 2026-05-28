// ed/handle_guide.rs
//! Guide popup initialization and key handling.

use crate::ed::mode::MessageKind;
use crate::event::KeyEvent;
use crate::Editor;
use crossterm::event::KeyCode;

impl Editor {
    /// Open the code guide popup.
    pub fn open_guide_popup(&mut self) {
        let guide = crate::ed::guide::Guide::load();
        self.popup.guide = Some(crate::popup::guide::GuidePopup::new(guide.entries));
    }

    /// Handle key events when the Guide popup is active.
    pub fn handle_guide_popup_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Esc => {
                self.popup.close();
            }
            KeyCode::Up => {
                if let Some(ref mut p) = self.popup.guide {
                    p.move_up();
                }
            }
            KeyCode::Down => {
                if let Some(ref mut p) = self.popup.guide {
                    p.move_down();
                }
            }
            KeyCode::Enter => {
                let entry_data = self
                    .popup
                    .guide
                    .as_ref()
                    .and_then(|p| p.selected_entry())
                    .cloned();
                self.popup.close();

                if let Some(entry) = entry_data {
                    let target_path = std::path::PathBuf::from(&entry.file);
                    let abs_path = crate::git::gutter::find_git_root(std::path::Path::new("."))
                        .unwrap_or_else(|| std::path::PathBuf::from("."))
                        .join(&target_path);

                    // Check if buffer is already open
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

                    // Jump to the anchor string in the target file
                    let source = self.buf().rope.to_string();
                    if let Some(line) =
                        crate::ed::guide::Guide::find_anchor_line(&source, &entry.anchor)
                    {
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
            KeyCode::Backspace => {
                if let Some(ref mut p) = self.popup.guide {
                    p.filter_pop();
                }
            }
            KeyCode::Char(c) => {
                if let Some(ref mut p) = self.popup.guide {
                    p.filter_push(c);
                }
            }
            _ => {}
        }
    }
}
