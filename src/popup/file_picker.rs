//! File picker popup overlay for browsing and opening files.

use ignore::WalkBuilder;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
pub struct FileEntry {
    pub name: String,
    pub path: PathBuf,
    pub is_dir: bool,
    pub is_parent: bool,
}

#[derive(Debug, Clone)]
pub struct FilePicker {
    pub all_entries: Vec<FileEntry>,
    pub filtered: Vec<usize>,
    pub selected: usize,
    pub scroll: usize,
    pub filter: String,
    pub cwd: PathBuf,
    pub visible_height: usize,
    pub flat: bool,
    /// For each filtered entry, the char indices in `name` that matched (for highlighting).
    pub match_indices: Vec<Vec<usize>>,
    /// Last I/O error, shown in the picker when set.
    pub last_error: Option<String>,
}

impl FilePicker {
    // ── Canonicalisation helper ────────────────────────────────────
    /// Resolve a directory path to an absolute, normalised form.
    /// Falls back to the raw path if `canonicalize` fails (e.g. the
    /// dir doesn't exist yet).
    fn canonicalize_dir(path: &Path) -> PathBuf {
        if path.is_dir() {
            std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
        } else {
            path.to_path_buf()
        }
    }

    // ── Initial CWD resolution (with canonicalisation) ─────────────
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
            // Path doesn't exist – try parent, then fall back to "."
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

        // Always store an absolute, normalised path so that
        // navigation, prefix-stripping, and duplicate detection work
        // reliably regardless of how the initial path was specified.
        Self::canonicalize_dir(&raw)
    }

    pub fn new(initial_path: &Path, flat: bool) -> Self {
        let effective_cwd = Self::resolve_initial_cwd(initial_path);

        let mut picker = FilePicker {
            all_entries: Vec::new(),
            filtered: Vec::new(),
            match_indices: Vec::new(),
            selected: 0,
            scroll: 0,
            filter: String::new(),
            cwd: effective_cwd,
            visible_height: 20,
            flat,
            last_error: None,
        };
        picker.refresh_entries();
        picker
    }

    pub fn refresh_entries(&mut self) {
        self.all_entries.clear();
        self.last_error = None;

        if self.flat {
            self.refresh_entries_flat();
        } else {
            self.refresh_entries_tree();
        }

        self.apply_filter();
    }

    fn refresh_entries_tree(&mut self) {
        // ── Parent entry ("../") ───────────────────────────────────
        if self.can_go_up() {
            if let Some(parent) = self.cwd.parent() {
                self.all_entries.push(FileEntry {
                    name: "../".to_string(),
                    path: parent.to_path_buf(),
                    is_dir: true,
                    is_parent: true,
                });
            }
        }

        // ── Directory listing ──────────────────────────────────────
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
                        files.push(fe);
                    }
                }

                dirs.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
                files.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
                self.all_entries.extend(dirs);
                self.all_entries.extend(files);
            }
            Err(e) => {
                self.last_error = Some(format!("Cannot read directory: {}", e));
            }
        }
    }

    fn refresh_entries_flat(&mut self) {
        if !self.cwd.is_dir() {
            self.last_error = Some(format!("Not a directory: {}", self.cwd.display()));
            return;
        }

        let mut entries = Vec::new();

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
                    // Keep the first error for display but continue walking
                    if walk_error.is_none() {
                        walk_error = Some(e);
                    }
                }
            }
        }

        if entries.is_empty() && walk_error.is_some() {
            self.last_error = Some(format!("Walk failed: {}", walk_error.unwrap()));
        }

        entries.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
        self.all_entries = entries;
    }

    pub fn toggle_flat(&mut self) {
        self.flat = !self.flat;
        self.filter.clear();
        self.selected = 0;
        self.scroll = 0;
        self.refresh_entries();
    }

    /// Fuzzy match: returns `Some(indices)` if every char in `query` appears
    /// in `text` in order (case-insensitive), `None` otherwise.
    fn fuzzy_match(text: &str, query: &str) -> Option<Vec<usize>> {
        if query.is_empty() {
            return Some(Vec::new());
        }

        let text_lower: Vec<char> = text.to_lowercase().chars().collect();
        let query_chars: Vec<char> = query.to_lowercase().chars().collect();

        let mut indices = Vec::with_capacity(query_chars.len());
        let mut qi = 0;

        for (ti, tc) in text_lower.iter().enumerate() {
            if qi < query_chars.len() && *tc == query_chars[qi] {
                indices.push(ti);
                qi += 1;
            }
        }

        if qi == query_chars.len() {
            Some(indices)
        } else {
            None
        }
    }

    pub(crate) fn apply_filter(&mut self) {
        self.filtered.clear();
        self.match_indices.clear();

        let query = self.filter.trim();

        for (i, entry) in self.all_entries.iter().enumerate() {
            if entry.is_parent {
                self.filtered.push(i);
                self.match_indices.push(Vec::new());
                continue;
            }

            if query.is_empty() {
                self.filtered.push(i);
                self.match_indices.push(Vec::new());
                continue;
            }

            if let Some(indices) = Self::fuzzy_match(&entry.name, query) {
                self.filtered.push(i);
                self.match_indices.push(indices);
            }
        }

        if self.filtered.is_empty() {
            self.selected = 0;
        } else if self.selected >= self.filtered.len() {
            let parent_pos = self.filtered.iter().position(|&idx| {
                self.all_entries
                    .get(idx)
                    .map(|e| e.is_parent)
                    .unwrap_or(false)
            });
            self.selected = parent_pos.unwrap_or(self.filtered.len() - 1);
        }

        self.clamp_scroll();
    }

    fn clamp_scroll(&mut self) {
        if self.scroll > self.selected {
            self.scroll = self.selected;
        } else if self.selected >= self.scroll + self.visible_height {
            self.scroll = self.selected - self.visible_height + 1;
        }
    }

    pub fn filter_push(&mut self, c: char) {
        self.filter.push(c);
        self.selected = 0;
        self.scroll = 0;
        self.apply_filter();
    }

    pub fn filter_pop(&mut self) {
        self.filter.pop();
        self.selected = 0;
        self.scroll = 0;
        self.apply_filter();
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
            // Prevent infinite loop at filesystem root where
            // parent resolution might circle back.
            if canonical == self.cwd {
                return;
            }
            self.cwd = canonical;
            self.filter.clear();
            self.selected = 0;
            self.scroll = 0;
            self.refresh_entries();
        }
    }

    pub fn go_into(&mut self, path: &Path) {
        if path.is_dir() {
            self.cwd = Self::canonicalize_dir(path);
            self.filter.clear();
            self.selected = 0;
            self.scroll = 0;
            self.refresh_entries();
        }
    }

    pub fn selected_entry(&self) -> Option<&FileEntry> {
        self.filtered
            .get(self.selected)
            .and_then(|&i| self.all_entries.get(i))
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

    pub fn move_up(&mut self) {
        if self.selected > 0 {
            self.selected -= 1;
            self.clamp_scroll();
        }
    }

    pub fn move_down(&mut self) {
        if !self.filtered.is_empty() && self.selected < self.filtered.len() - 1 {
            self.selected += 1;
            self.clamp_scroll();
        }
    }

    pub fn set_initial_filter(&mut self, filter: &str) {
        self.filter = filter.to_string();
        self.selected = 0;
        self.scroll = 0;
        self.apply_filter();
    }
}
