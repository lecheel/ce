//--+ filter.rs
use crate::popup::guide::GuidePopup;
use crate::popup::{FilePicker, FilterableList, FunctionListPopup, MruPopup};

impl FilterableList for FilePicker {
    fn move_up(&mut self) {
        FilePicker::move_up(self)
    }
    fn move_down(&mut self) {
        FilePicker::move_down(self)
    }
    fn filter_pop(&mut self) {
        FilePicker::filter_pop(self)
    }
    fn filter_push(&mut self, c: char) {
        FilePicker::filter_push(self, c)
    }
}

impl FilterableList for FunctionListPopup {
    fn move_up(&mut self) {
        FunctionListPopup::move_up(self)
    }
    fn move_down(&mut self) {
        FunctionListPopup::move_down(self)
    }
    fn filter_pop(&mut self) {
        FunctionListPopup::filter_pop(self)
    }
    fn filter_push(&mut self, c: char) {
        FunctionListPopup::filter_push(self, c)
    }
}

impl FilterableList for GuidePopup {
    fn move_up(&mut self) {
        GuidePopup::move_up(self)
    }
    fn move_down(&mut self) {
        GuidePopup::move_down(self)
    }
    fn filter_pop(&mut self) {
        GuidePopup::filter_pop(self)
    }
    fn filter_push(&mut self, c: char) {
        GuidePopup::filter_push(self, c)
    }
}

impl FilterableList for MruPopup {
    fn move_up(&mut self) {
        MruPopup::move_up(self)
    }
    fn move_down(&mut self) {
        MruPopup::move_down(self)
    }
    fn filter_pop(&mut self) {
        MruPopup::filter_pop(self)
    }
    fn filter_push(&mut self, c: char) {
        MruPopup::filter_push(self, c)
    }
}
