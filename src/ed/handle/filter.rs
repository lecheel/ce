//--+ filter.rs
use crate::popup::guide::GuidePopup;
use crate::popup::{FilePicker, FilterableList, FunctionListPopup, MruPopup};

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
