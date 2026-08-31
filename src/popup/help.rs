use crate::popup::filtered_list::{EntryFilter, FilteredList};
use crate::popup::fuzzy;
use crate::popup::Scrollable;

impl EntryFilter for HelpEntry {
    fn match_query(&self, query: &str) -> Option<Vec<usize>> {
        let haystack = format!("{} {}", self.name, self.description);
        let name_len = self.name.chars().count();
        fuzzy::fuzzy_match(&haystack, query)
            .map(|indices| indices.into_iter().filter(|&i| i < name_len).collect())
    }
}

#[derive(Debug, Clone)]
pub struct HelpEntry {
    pub name: String,
    pub description: String,
}

#[derive(Debug, Clone)]
pub struct HelpPopup {
    pub list: FilteredList<HelpEntry>,
}

impl HelpPopup {
    pub fn new(entries: Vec<HelpEntry>) -> Self {
        Self {
            list: FilteredList::new(entries),
        }
    }

    pub fn selected_entry(&self) -> Option<&HelpEntry> {
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

impl Scrollable for HelpPopup {
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

pub fn build_help_entries() -> Vec<HelpEntry> {
    vec![
        HelpEntry {
            name: "q".to_string(),
            description: "Quit current buffer or editor".to_string(),
        },
        HelpEntry {
            name: "w".to_string(),
            description: "Write (save) the current buffer".to_string(),
        },
        HelpEntry {
            name: "wq".to_string(),
            description: "Save and quit".to_string(),
        },
        HelpEntry {
            name: "ls".to_string(),
            description: "List open buffers".to_string(),
        },
        HelpEntry {
            name: "bd".to_string(),
            description: "Delete (close) the current buffer".to_string(),
        },
        HelpEntry {
            name: "e".to_string(),
            description: "Edit a file (or create new)".to_string(),
        },
        HelpEntry {
            name: "sp".to_string(),
            description: "Split window horizontally".to_string(),
        },
        HelpEntry {
            name: "vs".to_string(),
            description: "Split window vertically".to_string(),
        },
        HelpEntry {
            name: "close".to_string(),
            description: "Close current window".to_string(),
        },
        HelpEntry {
            name: "config".to_string(),
            description: "Open editor configuration popup".to_string(),
        },
        HelpEntry {
            name: "scankey".to_string(),
            description: "Open key scanner popup".to_string(),
        },
        HelpEntry {
            name: "checkhealth".to_string(),
            description: "Check editor and system health".to_string(),
        },
        HelpEntry {
            name: "guide".to_string(),
            description: "Open guide popup".to_string(),
        },
        HelpEntry {
            name: "command_palette".to_string(),
            description: "Open the command palette".to_string(),
        },
        HelpEntry {
            name: "vocab".to_string(),
            description: "Add a word to the local vocabulary".to_string(),
        },
        HelpEntry {
            name: "ff".to_string(),
            description: "Open file picker".to_string(),
        },
        HelpEntry {
            name: "functions".to_string(),
            description: "Open function list popup".to_string(),
        },
        HelpEntry {
            name: "mru".to_string(),
            description: "Open Most Recently Used files (repo only)".to_string(),
        },
        HelpEntry {
            name: "tig".to_string(),
            description: "Open git log".to_string(),
        },
        HelpEntry {
            name: "gs".to_string(),
            description: "Open git status".to_string(),
        },
        HelpEntry {
            name: "diffthis".to_string(),
            description: "Open diff for current file".to_string(),
        },
        HelpEntry {
            name: "stash".to_string(),
            description: "Stash changes".to_string(),
        },
        HelpEntry {
            name: "tag".to_string(),
            description: "Jump to tag".to_string(),
        },
        HelpEntry {
            name: "fd".to_string(),
            description: "Find files".to_string(),
        },
        HelpEntry {
            name: "sym".to_string(),
            description: "Search workspace symbols".to_string(),
        },
        HelpEntry {
            name: "copilot".to_string(),
            description: "Copilot auth".to_string(),
        },
        HelpEntry {
            name: "codellm".to_string(),
            description: "Open CodeLLM chat".to_string(),
        },
        HelpEntry {
            name: "llm".to_string(),
            description: "Send message to LLM".to_string(),
        },
        HelpEntry {
            name: "build".to_string(),
            description: "Run build".to_string(),
        },
        HelpEntry {
            name: "retab".to_string(),
            description: "Convert tabs to spaces".to_string(),
        },
        HelpEntry {
            name: "rg".to_string(),
            description: "Ripgrep search".to_string(),
        },
        HelpEntry {
            name: "sort".to_string(),
            description: "Sort lines (unique, reverse, insensitive)".to_string(),
        },
        HelpEntry {
            name: "blame".to_string(),
            description: "Toggle git blame".to_string(),
        },
        HelpEntry {
            name: "skill".to_string(),
            description: "Activate LLM skill".to_string(),
        },
        HelpEntry {
            name: "vim".to_string(),
            description: "Switch to Vim (Normal) mode".to_string(),
        },
        HelpEntry {
            name: "brief".to_string(),
            description: "Switch to Brief mode".to_string(),
        },
    ]
}
