use crate::popup::fd::FdPopup;
use crate::popup::guide::GuidePopup;
use crate::popup::{FilePicker, FilterableList, FunctionListPopup, MruPopup, Scrollable};

impl FilterableList for FilePicker {
    fn move_up(&mut self) {
        self.list.move_up();
    }
    fn move_down(&mut self) {
        self.list.move_down();
    }
    fn filter_pop(&mut self) {
        self.list.filter_pop();
    }
    fn filter_push(&mut self, c: char) {
        self.list.filter_push(c);
    }
}

impl FilterableList for FunctionListPopup {
    fn move_up(&mut self) {
        self.list.move_up();
    }
    fn move_down(&mut self) {
        self.list.move_down();
    }
    fn filter_pop(&mut self) {
        self.list.filter_pop();
    }
    fn filter_push(&mut self, c: char) {
        self.list.filter_push(c);
    }
}

impl FilterableList for GuidePopup {
    fn move_up(&mut self) {
        self.list.move_up();
    }
    fn move_down(&mut self) {
        self.list.move_down();
    }
    fn filter_pop(&mut self) {
        self.list.filter_pop();
    }
    fn filter_push(&mut self, c: char) {
        self.list.filter_push(c);
    }
}

impl FilterableList for MruPopup {
    fn move_up(&mut self) {
        self.list.move_up()
    }
    fn move_down(&mut self) {
        self.list.move_down()
    }
    fn filter_pop(&mut self) {
        self.list.filter_pop()
    }
    fn filter_push(&mut self, c: char) {
        self.list.filter_push(c)
    }
}

// ── FdPopup trait implementations ──────────────────────────────────

impl FilterableList for FdPopup {
    fn move_up(&mut self) {
        self.list.move_up();
    }
    fn move_down(&mut self) {
        self.list.move_down();
    }
    fn filter_pop(&mut self) {
        self.list.filter_pop();
    }
    fn filter_push(&mut self, c: char) {
        self.list.filter_push(c);
    }
}

impl Scrollable for FdPopup {
    fn selected(&self) -> usize {
        self.list.selected()
    }
    fn selected_mut(&mut self) -> &mut usize {
        self.list.selected_mut()
    }
    fn scroll_mut(&mut self) -> &mut usize {
        self.list.scroll_mut()
    }
    fn len(&self) -> usize {
        self.list.len()
    }
    fn visible_rows(&self) -> usize {
        self.list.visible_rows()
    }
}
