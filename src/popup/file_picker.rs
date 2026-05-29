//! File picker popup overlay for browsing and opening files.

use crate::popup::filtered_list::{EntryFilter, FilteredList};
use crate::popup::fuzzy;
use ignore::WalkBuilder;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
pub struct FileEntry {
    pub name: String,
    pub path: PathBuf,
    pub is_dir: bool,
    pub is_parent: bool,
}

impl EntryFilter for FileEntry {
    fn match_query(&self, query: &str) -> Option<Vec<usize>> {
        fuzzy::fuzzy_match(&self.name, query)
    }

    fn is_pinned(&self) -> bool {
        self.is_parent
    }
}

#[derive(Debug, Clone)]
pub struct FilePicker {
    pub list: FilteredList<FileEntry>,
    pub cwd: PathBuf,
    pub initial_cwd: PathBuf,
    pub flat: bool,
    pub last_error: Option<String>,

    pub new_file_mode: bool,
    pub new_file_input: String,
    pub new_file_base_dir: PathBuf,

    pub delete_confirm_mode: bool,
    pub delete_target_path: PathBuf,
}

impl FilePicker {
    fn canonicalize_dir(path: &Path) -> PathBuf {
        if path.is_dir() {
            std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
        } else {
            path.to_path_buf()
        }
    }

    fn resolve_initial_cwd(initial_path: &Path) -> PathBuf {
        let raw = if initial_path.is_file() {
            initial_path
                .parent()
                .map(|p| {
                    if p.as_os_str().is_empty() {
                        PathBuf::from(".")
                    } else {
                        p.to_path_buf()
                    }
                })
                .unwrap_or_else(|| PathBuf::from("."))
        } else if initial_path.as_os_str().is_empty() {
            PathBuf::from(".")
        } else if initial_path.is_dir() {
            initial_path.to_path_buf()
        } else {
            initial_path
                .parent()
                .and_then(|p| {
                    if p.as_os_str().is_empty() {
                        Some(PathBuf::from("."))
                    } else if p.is_dir() {
                        Some(p.to_path_buf())
                    } else {
                        None
                    }
                })
                .unwrap_or_else(|| PathBuf::from("."))
        };
        Self::canonicalize_dir(&raw)
    }

    pub fn new(initial_path: &Path, flat: bool) -> Self {
        let effective_cwd = Self::resolve_initial_cwd(initial_path);
        let mut picker = FilePicker {
            list: FilteredList::new(Vec::new()),
            cwd: effective_cwd.clone(),
            initial_cwd: effective_cwd,
            flat,
            last_error: None,
            new_file_mode: false,
            new_file_input: String::new(),
            new_file_base_dir: PathBuf::new(),
            delete_confirm_mode: false,
            delete_target_path: PathBuf::new(),
        };
        picker.refresh_entries();
        picker
    }

    pub fn refresh_entries(&mut self) {
        self.last_error = None;
        let entries = if self.flat {
            self.build_flat_entries()
        } else {
            self.build_tree_entries()
        };
        self.list.set_entries(entries);
    }

    fn build_tree_entries(&self) -> Vec<FileEntry> {
        let mut all_entries = Vec::new();

        // Parent entry ("../")
        if self.can_go_up() {
            if let Some(parent) = self.cwd.parent() {
                all_entries.push(FileEntry {
                    name: "../".to_string(),
                    path: parent.to_path_buf(),
                    is_dir: true,
                    is_parent: true,
                });
            }
        }

        match std::fs::read_dir(&self.cwd) {
            Ok(entries) => {
                let mut dirs: Vec<FileEntry> = Vec::new();
                let mut files: Vec<FileEntry> = Vec::new();

                for entry in entries.flatten() {
                    let path = entry.path();
                    let name = entry.file_name().to_string_lossy().to_string();
                    if name.starts_with('.') {
                        continue;
                    }
                    let is_dir = path.is_dir();
                    let fe = FileEntry {
                        name: if is_dir { format!("{}/", name) } else { name },
                        path,
                        is_dir,
                        is_parent: false,
                    };
                    if is_dir {
                        dirs.push(fe);
                    } else {
                        files.push(fe)
                    }
                }

                dirs.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
                files.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
                all_entries.extend(dirs);
                all_entries.extend(files);
            }
            Err(e) => {
                let _ = e;
            }
        }

        all_entries
    }

    fn build_flat_entries(&self) -> Vec<FileEntry> {
        let mut entries = Vec::new();

        if !self.cwd.is_dir() {
            return entries;
        }

        let walker = WalkBuilder::new(&self.cwd)
            .hidden(true)
            .git_ignore(true)
            .git_global(true)
            .git_exclude(true)
            .build();

        let mut walk_error = None;
        for result in walker {
            match result {
                Ok(entry) => {
                    if entry.path() == self.cwd {
                        continue;
                    }
                    let is_dir = entry.file_type().map_or(false, |ft| ft.is_dir());
                    if is_dir {
                        continue;
                    }
                    let path = entry.path().to_path_buf();
                    let relative = entry.path().strip_prefix(&self.cwd).unwrap_or(entry.path());
                    let name = relative.to_string_lossy().to_string();
                    entries.push(FileEntry {
                        name,
                        path,
                        is_dir: false,
                        is_parent: false,
                    });
                }
                Err(e) => {
                    if walk_error.is_none() {
                        walk_error = Some(e);
                    }
                }
            }
        }

        if entries.is_empty() {
            if let Some(e) = walk_error {
                let _ = e;
            }
        }

        entries.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
        entries
    }

    pub fn toggle_flat(&mut self) {
        self.flat = !self.flat;
        self.list.filter_clear();
        self.refresh_entries();
    }

    pub fn go_up(&mut self) {
        if self.flat {
            return;
        }
        if let Some(parent) = self.cwd.parent() {
            if parent.as_os_str().is_empty() {
                return;
            }
            let canonical = Self::canonicalize_dir(parent);
            if canonical == self.cwd {
                return;
            }
            self.cwd = canonical;
            self.list.filter_clear();
            self.refresh_entries();
        }
    }

    pub fn go_into(&mut self, path: &Path) {
        if path.is_dir() {
            self.cwd = Self::canonicalize_dir(path);
            self.list.filter_clear();
            self.refresh_entries();
        }
    }

    pub fn selected_entry(&self) -> Option<&FileEntry> {
        self.list.selected_entry()
    }

    pub fn can_go_up(&self) -> bool {
        self.cwd
            .parent()
            .map(|p| !p.as_os_str().is_empty() && Self::canonicalize_dir(p) != self.cwd)
            .unwrap_or(false)
    }

    pub fn cwd_display(&self) -> String {
        self.cwd.display().to_string()
    }

    pub fn set_initial_filter(&mut self, filter: &str) {
        self.list.filter = filter.to_string();
        self.list.selected = 0;
        self.list.scroll = 0;
        self.list.apply_filter();
    }

    pub fn go_home(&mut self) {
        let target = crate::git::gutter::find_git_root(&self.cwd)
            .unwrap_or_else(|| self.initial_cwd.clone());

        let canonical = Self::canonicalize_dir(&target);
        if canonical != self.cwd {
            self.cwd = canonical;
            self.list.filter_clear();
            self.refresh_entries();
        }
    }
}
