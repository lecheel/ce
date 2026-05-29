//! Fuzzy-find filename popup powered by `fd` / `find`.

use crate::popup::filtered_list::{EntryFilter, FilteredList};
use crate::popup::fuzzy;
use std::path::{Path, PathBuf};
use std::process::Command;

/// Maximum entries returned by the backend to avoid UI lag.
const MAX_RESULTS: usize = 2000;

// ── Entry ──────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct FdEntry {
    /// Relative path from root_dir (displayed and fuzzy-matched).
    pub name: String,
    /// Absolute (or cwd-relative) path used to open the file.
    pub path: PathBuf,
}

impl EntryFilter for FdEntry {
    fn match_query(&self, query: &str) -> Option<Vec<usize>> {
        fuzzy::fuzzy_match(&self.name, query)
    }
}

// ── Popup ──────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct FdPopup {
    pub list: FilteredList<FdEntry>,
    pub root_dir: PathBuf,
    /// The initial glob/regex passed to `fd` (may be empty).
    pub seed_pattern: String,
}

impl FdPopup {
    /// Build a new popup.  `root_dir` is the search root; `pattern` is an
    /// optional pre-filter passed directly to `fd` (or `find`).
    pub fn new(root_dir: &Path, pattern: &str) -> Self {
        let entries = collect_entries(root_dir, pattern);
        let mut popup = FdPopup {
            list: FilteredList::new(entries),
            root_dir: root_dir.to_path_buf(),
            seed_pattern: pattern.to_string(),
        };
        // If the user typed a seed pattern, also prime the in-popup filter
        // so they can further narrow results.
        if !pattern.is_empty() {
            for ch in pattern.chars() {
                popup.list.filter_push(ch);
            }
        }
        popup
    }

    pub fn selected_entry(&self) -> Option<&FdEntry> {
        self.list.selected_entry()
    }
}

// ── Backend collectors ─────────────────────────────────────────────

/// Pick the best available backend and collect file paths.
fn collect_entries(root_dir: &Path, pattern: &str) -> Vec<FdEntry> {
    if which_fd() {
        run_fd(root_dir, pattern)
    } else if which_fdfind() {
        run_fdfind(root_dir, pattern)
    } else {
        run_find(root_dir, pattern)
    }
}

fn which_fd() -> bool {
    Command::new("fd")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

fn which_fdfind() -> bool {
    Command::new("fdfind")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

fn run_fd_with_bin(bin: &str, root_dir: &Path, pattern: &str) -> Vec<FdEntry> {
    let mut cmd = Command::new(bin);
    cmd.arg("--type")
        .arg("f") // files only
        .arg("--follow")
        .arg("--color")
        .arg("never")
        .arg("--max-results")
        .arg(MAX_RESULTS.to_string())
        .current_dir(root_dir);

    if !pattern.is_empty() {
        cmd.arg(pattern);
    }

    match cmd.output() {
        Ok(output) if output.status.success() => {
            parse_stdout(&String::from_utf8_lossy(&output.stdout), root_dir)
        }
        _ => Vec::new(),
    }
}

fn run_fd(root_dir: &Path, pattern: &str) -> Vec<FdEntry> {
    run_fd_with_bin("fd", root_dir, pattern)
}

fn run_fdfind(root_dir: &Path, pattern: &str) -> Vec<FdEntry> {
    run_fd_with_bin("fdfind", root_dir, pattern)
}

fn run_find(root_dir: &Path, pattern: &str) -> Vec<FdEntry> {
    let mut cmd = Command::new("find");
    cmd.arg(".")
        .arg("-type")
        .arg("f")
        .arg("-not")
        .arg("-path")
        .arg("*/\\.*")
        .current_dir(root_dir);

    match cmd.output() {
        Ok(output) if output.status.success() => {
            let mut entries = parse_stdout(&String::from_utf8_lossy(&output.stdout), root_dir);
            // `find` has no built-in pattern filter; do it in-process.
            if !pattern.is_empty() {
                let pat = pattern.to_lowercase();
                entries.retain(|e| e.name.to_lowercase().contains(&pat));
            }
            entries.truncate(MAX_RESULTS);
            entries
        }
        _ => Vec::new(),
    }
}

fn parse_stdout(stdout: &str, root_dir: &Path) -> Vec<FdEntry> {
    let mut entries: Vec<FdEntry> = stdout
        .lines()
        .filter(|l| !l.is_empty())
        .map(|line| {
            let name = line.trim_start_matches("./").to_string();
            let path = root_dir.join(line);
            FdEntry { name, path }
        })
        .collect();
    entries.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
    entries
}
