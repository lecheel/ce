//--+ buffer_list.rs
use crate::ed::MessageKind;
use crate::event::KeyEvent;
use crate::popup::BufferList;
use crate::Editor;
use crossterm::event::KeyCode;

impl crate::popup::FilterableList for BufferList {
    fn move_up(&mut self) {
        BufferList::move_up(self)
    }
    fn move_down(&mut self) {
        BufferList::move_down(self)
    }
    fn filter_pop(&mut self) {
        BufferList::filter_pop(self)
    }
    fn filter_push(&mut self, c: char) {
        BufferList::filter_push(self, c)
    }
}

impl Editor {
    pub fn handle_buffer_list_key(&mut self, key: KeyEvent) -> bool {
        if crate::popup::dispatch_list_nav(&mut self.popup.buffer_list, &key) {
            return true;
        }
        match key.code {
            KeyCode::Enter => {
                self.buffer_list_enter();
                true
            }
            KeyCode::Char('q') | KeyCode::Esc => {
                self.popup.buffer_list = None;
                true
            }
            _ => false,
        }
    }

    fn buffer_list_enter(&mut self) {
        if let Some(ref p) = self.popup.buffer_list {
            if let Some(entry) = p.entries.get(p.selected) {
                let bid = entry.id;
                self.popup.buffer_list = None;
                self.switch_window_to_buffer(bid);
                self.set_status_msg("Switched buffer", MessageKind::Info);
            }
        }
    }
}
