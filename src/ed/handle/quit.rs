use crate::ed::editor::QuitPrompt;
use crate::ed::MessageKind;
use crate::event::KeyEvent;
use crate::Editor;
use crossterm::event::KeyCode;

impl Editor {
    pub fn handle_quit_prompt_key(&mut self, key: KeyEvent) {
        match self.quit_prompt {
            QuitPrompt::BufferQuit => match key.code {
                KeyCode::Char('y') | KeyCode::Char('Y') => {
                    if let Err(e) = self.save_active_buffer() {
                        self.set_status_msg(&format!("Save failed: {}", e), MessageKind::Error);
                    } else if self.buffers.len() > 1 {
                        self.close_buffer();
                    } else {
                        self.save_all_window_positions();
                        self.should_quit = true;
                    }
                    self.quit_prompt = QuitPrompt::None;
                }
                KeyCode::Char('n') | KeyCode::Char('N') => {
                    if self.buffers.len() > 1 {
                        self.close_buffer();
                    } else {
                        self.save_all_window_positions();
                        self.should_quit = true;
                    }
                    self.quit_prompt = QuitPrompt::None;
                }
                KeyCode::Char('c') | KeyCode::Char('C') | KeyCode::Esc => {
                    self.quit_prompt = QuitPrompt::None;
                    self.clear_status_msg();
                }
                _ => {}
            },
            QuitPrompt::QuitAllConfirm => match key.code {
                KeyCode::Char('y') | KeyCode::Char('Y') => {
                    if let Err(e) = self.save_active_buffer() {
                        self.set_status_msg(&format!("Save failed: {}", e), MessageKind::Error);
                        self.quit_prompt = QuitPrompt::None;
                    } else {
                        self.quit_all_check();
                    }
                }
                KeyCode::Char('n') | KeyCode::Char('N') => {
                    self.buf_mut().modified = false;
                    self.quit_all_check();
                }
                KeyCode::Char('c') | KeyCode::Char('C') | KeyCode::Esc => {
                    self.quit_prompt = QuitPrompt::None;
                    self.clear_status_msg();
                    self.set_status_msg("Quit aborted", MessageKind::Info);
                }
                _ => {}
            },
            QuitPrompt::None => {}
        }
    }
}
