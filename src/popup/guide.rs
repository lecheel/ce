// popup/guide.rs
//! Guide popup overlay for searchable checkpoints navigation.

use crate::ed::guide::GuideEntry;
use crate::popup::filtered_list::{EntryFilter, FilteredList};
use crate::popup::Scrollable;

impl EntryFilter for GuideEntry {
    fn match_query(&self, query: &str) -> Option<Vec<usize>> {
        let q = query.to_lowercase();
        if self.label.to_lowercase().contains(&q)
            || self.desc.to_lowercase().contains(&q)
            || self.file.to_lowercase().contains(&q)
            || self.kind.to_lowercase().contains(&q)
            || self.tags.iter().any(|t| t.contains(&q))
            || self.anchor.to_lowercase().contains(&q)
        {
            Some(Vec::new())
        } else {
            None
        }
    }
}

#[derive(Debug, Clone)]
pub struct GuidePopup {
    pub list: FilteredList<crate::ed::guide::GuideEntry>,
}

impl GuidePopup {
    pub fn new(entries: Vec<crate::ed::guide::GuideEntry>) -> Self {
        Self {
            list: FilteredList::new(entries),
        }
    }

    pub fn selected_entry(&self) -> Option<&crate::ed::guide::GuideEntry> {
        self.list.selected_entry()
    }

    pub fn filter_push(&mut self, c: char) {
        self.list.filter_push(c);
    }
    pub fn filter_pop(&mut self) {
        self.list.filter_pop();
    }
    pub fn filter_clear(&mut self) {
        self.list.filter_clear();
    }
    pub fn move_up(&mut self) {
        self.list.move_up();
    }
    pub fn move_down(&mut self) {
        self.list.move_down();
    }
}

impl Scrollable for GuidePopup {
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
