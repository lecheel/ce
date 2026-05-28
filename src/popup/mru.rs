//! MRU (Most Recently Used) popup overlay for opening recent files.

use crate::popup::filtered_list::{EntryFilter, FilteredList};
use crate::popup::fuzzy;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

// ── MRU Entry ──

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MruEntry {
    pub path: PathBuf,
    pub line: usize,
    pub col: usize,
    pub open_count: usize,
    pub last_opened: std::time::SystemTime,
}

impl MruEntry {
    pub fn relative_time(&self) -> String {
        let elapsed = self
            .last_opened
            .elapsed()
            .unwrap_or(std::time::Duration::ZERO);
        let secs = elapsed.as_secs();
        if secs < 60 {
            "just now".to_string()
        } else if secs < 3600 {
            format!("{}m ago", secs / 60)
        } else if secs < 86400 {
            format!("{}h ago", secs / 3600)
        } else {
            format!("{}d ago", secs / 86400)
        }
    }
}

impl EntryFilter for MruEntry {
    fn match_query(&self, query: &str) -> Option<Vec<usize>> {
        let q = query.to_lowercase();
        let file_name = self.path.file_name().and_then(|n| n.to_str()).unwrap_or("");
        let dir = self.path.parent().and_then(|p| p.to_str()).unwrap_or("");

        if file_name.to_lowercase().contains(&q) || dir.to_lowercase().contains(&q) {
            if let Some((s, e)) = fuzzy::substring_find(file_name, query) {
                Some((s..e).collect())
            } else {
                Some(Vec::new())
            }
        } else {
            None
        }
    }
}

// ── MRU Manager ──

#[derive(Debug, Clone, Default)]
pub struct MruManager {
    pub entries: Vec<MruEntry>,
}

impl MruManager {
    pub fn new() -> Self {
        Self {
            entries: Vec::new(),
        }
    }

    pub fn load() -> Self {
        if let Ok(dir) = crate::config::app_config::Config::config_dir() {
            let path = dir.join("mru.json");
            if path.exists() {
                if let Ok(content) = std::fs::read_to_string(path) {
                    if let Ok(entries) = serde_json::from_str::<Vec<MruEntry>>(&content) {
                        return Self { entries };
                    }
                }
            }
        }
        Self::new()
    }

    pub fn save(&self) {
        if let Ok(dir) = crate::config::app_config::Config::config_dir() {
            let path = dir.join("mru.json");
            if let Ok(content) = serde_json::to_string_pretty(&self.entries) {
                let _ = std::fs::write(path, content);
            }
        }
    }

    pub fn get_entries(&self) -> Vec<MruEntry> {
        let mut sorted = self.entries.clone();
        sorted.sort_by(|a, b| b.last_opened.cmp(&a.last_opened));
        sorted
    }

    pub fn entries_by_frequency(&self) -> Vec<MruEntry> {
        let mut sorted = self.entries.clone();
        sorted.sort_by(|a, b| {
            b.open_count
                .cmp(&a.open_count)
                .then_with(|| b.last_opened.cmp(&a.last_opened))
        });
        sorted
    }

    pub fn insert(&mut self, path: PathBuf, line: usize, col: usize) {
        let canon_path = std::fs::canonicalize(&path).unwrap_or(path);

        if let Some(pos) = self.entries.iter().position(|e| e.path == canon_path) {
            let entry = &mut self.entries[pos];
            entry.line = line;
            entry.col = col;
            entry.open_count += 1;
            entry.last_opened = std::time::SystemTime::now();
        } else {
            self.entries.push(MruEntry {
                path: canon_path,
                line,
                col,
                open_count: 1,
                last_opened: std::time::SystemTime::now(),
            });
        }

        if self.entries.len() > 100 {
            let mut sorted = self.entries.clone();
            sorted.sort_by(|a, b| b.last_opened.cmp(&a.last_opened));
            sorted.truncate(100);
            self.entries = sorted;
        }

        self.save();
    }
}

// ── MRU Popup ──

#[derive(Debug, Clone)]
pub struct MruPopup {
    pub list: FilteredList<MruEntry>,
    pub sort_by_frequency: bool,
    pub repo_root: Option<PathBuf>,
    pub repo_only: bool,
    pub all_mru_entries: Vec<MruEntry>,
}

impl MruPopup {
    pub fn new(entries: Vec<MruEntry>, repo_root: Option<PathBuf>, repo_only: bool) -> Self {
        let all = entries.clone();
        let mut list = FilteredList::new(entries);
        list.wraps = true;
        list.visible_height = 23;
        MruPopup {
            list,
            sort_by_frequency: false,
            repo_root,
            repo_only,
            all_mru_entries: all,
        }
    }

    pub fn selected_entry(&self) -> Option<&MruEntry> {
        self.list.selected_entry()
    }

    fn rebuild_entries(&mut self) {
        let entries: Vec<MruEntry> = if self.repo_only {
            self.all_mru_entries
                .iter()
                .filter(|e| match &self.repo_root {
                    Some(root) => e.path.starts_with(root),
                    None => true,
                })
                .cloned()
                .collect()
        } else {
            self.all_mru_entries.clone()
        };
        self.list.set_entries(entries);
    }

    pub fn remove_selected(&mut self, mru: &mut MruManager) {
        if let Some(entry) = self.list.selected_entry() {
            let path = entry.path.clone();
            mru.entries.retain(|e| e.path != path);
            mru.save();
            self.all_mru_entries.retain(|e| e.path != path);
            self.rebuild_entries();
        }
    }

    pub fn toggle_sort(&mut self, mru: &MruManager) -> bool {
        self.sort_by_frequency = !self.sort_by_frequency;
        self.all_mru_entries = if self.sort_by_frequency {
            mru.entries_by_frequency()
        } else {
            mru.get_entries()
        };
        self.rebuild_entries();
        self.sort_by_frequency
    }

    pub fn toggle_repo_filter(&mut self) {
        if self.repo_root.is_none() {
            return;
        }
        self.repo_only = !self.repo_only;
        self.rebuild_entries();
    }

    pub fn move_up(&mut self) {
        self.list.move_up();
    }

    pub fn move_down(&mut self) {
        self.list.move_down();
    }

    pub fn filter_push(&mut self, c: char) {
        self.list.filter_push(c);
    }

    pub fn filter_pop(&mut self) {
        self.list.filter_pop();
    }
}
