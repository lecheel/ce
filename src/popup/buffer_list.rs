//! Buffer list popup for browsing, switching, and closing active buffers.

use crate::popup::filtered_list::{EntryFilter, FilteredList};
use crate::popup::fuzzy;
use std::path::PathBuf;

#[derive(Debug, Clone)]
pub struct BufferEntry {
    pub id: usize,
    pub name: String,
    pub path: Option<PathBuf>,
    pub is_modified: bool,
    pub line_count: usize,
}

impl EntryFilter for BufferEntry {
    fn match_query(&self, query: &str) -> Option<Vec<usize>> {
        fuzzy::fuzzy_match(&self.name, query)
    }
}

#[derive(Debug, Clone)]
pub struct BufferList {
    pub list: FilteredList<BufferEntry>,
}

impl BufferList {
    pub fn new(entries: Vec<BufferEntry>) -> Self {
        Self {
            list: FilteredList::new(entries),
        }
    }

    pub fn selected_entry(&self) -> Option<&BufferEntry> {
        self.list.selected_entry()
    }
}
