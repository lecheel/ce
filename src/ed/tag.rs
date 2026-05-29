//! Tag management — ctags / gentag integration for jump-to-definition.
//!
//! Supports:
//! - `C-]`  — jump to tag under cursor
//! - `:tag <name>` — jump to a named tag
//! - `C-t`  — return from tag jump (tag stack)

use std::collections::HashMap;
use std::path::{Path, PathBuf};

// ═══════════════════════════════════════════════════════════════════════
// Data structures
// ═══════════════════════════════════════════════════════════════════════

/// A single tag entry parsed from a ctags / gentag file.
#[derive(Debug, Clone)]
pub struct TagEntry {
    /// Symbol name (e.g. `"main"`, `"Vec::new"`).
    pub name: String,
    /// File path, relative to the repo root where tags were generated.
    pub file: PathBuf,
    /// 1-based line number of the definition.
    pub line: usize,
    /// Kind of symbol (e.g. `"function"`, `"struct"`).
    pub kind: Option<String>,
}

/// Saved position on the tag stack (for `C-t` back-navigation).
#[derive(Debug, Clone)]
pub struct TagStackEntry {
    pub buffer_id: usize,
    pub row: usize,
    pub col: usize,
    pub filename: Option<String>,
}

// ═══════════════════════════════════════════════════════════════════════
// TagManager
// ═══════════════════════════════════════════════════════════════════════

/// Loads, caches, and queries ctags / gentag files.
pub struct TagManager {
    /// Tags indexed by **lowercase** name for case-insensitive lookup.
    tags: HashMap<String, Vec<TagEntry>>,
    /// Tag stack for `C-t` back-navigation.
    stack: Vec<TagStackEntry>,
    /// Repo root that tags were loaded for.
    loaded_root: Option<PathBuf>,
    /// Modification time of the tags file when last loaded.
    tags_mtime: Option<std::time::SystemTime>,
}

impl TagManager {
    pub fn new() -> Self {
        Self {
            tags: HashMap::new(),
            stack: Vec::new(),
            loaded_root: None,
            tags_mtime: None,
        }
    }

    // ── Loading ────────────────────────────────────────────────────────

    /// Ensure tags are loaded for `repo_root`.  Loads on first call,
    /// skips reload if the file hasn't changed since the last load.
    pub fn ensure_loaded(&mut self, repo_root: &Path) -> bool {
        if self.is_fresh(repo_root) {
            return !self.tags.is_empty();
        }
        self.load_for_repo(repo_root)
    }

    /// Force a full reload for `repo_root`.
    pub fn load_for_repo(&mut self, repo_root: &Path) -> bool {
        let tags_path = self.find_or_generate(repo_root);
        let Some(ref path) = tags_path else {
            return false;
        };

        // Stale-check: skip re-parse if the file hasn't changed.
        if let Ok(meta) = std::fs::metadata(path) {
            let mtime = meta.modified().ok();
            if self.loaded_root.as_deref() == Some(repo_root) && self.tags_mtime == mtime {
                return !self.tags.is_empty();
            }
            self.tags_mtime = mtime;
        }

        let content = match std::fs::read_to_string(path) {
            Ok(c) => c,
            Err(_) => return false,
        };

        self.tags.clear();
        self.loaded_root = Some(repo_root.to_path_buf());
        self.parse(&content)
    }

    /// Reload tags if the file on disk has been modified.
    pub fn reload_if_stale(&mut self, repo_root: &Path) -> bool {
        if self.is_fresh(repo_root) {
            return !self.tags.is_empty();
        }
        self.load_for_repo(repo_root)
    }

    // ── Lookup ─────────────────────────────────────────────────────────

    /// Look up tags by name (case-insensitive).  Returns an empty slice
    /// when no tags match.
    pub fn lookup(&self, name: &str) -> &[TagEntry] {
        self.tags.get(&name.to_lowercase()).map_or(&[], |v| v)
    }

    /// Total number of unique tag names loaded.
    pub fn tag_count(&self) -> usize {
        self.tags.len()
    }

    /// The repo root tags were loaded for.
    pub fn loaded_root(&self) -> Option<&Path> {
        self.loaded_root.as_deref()
    }

    // ── Tag stack ──────────────────────────────────────────────────────

    /// Push a position onto the tag stack before jumping.
    pub fn push(&mut self, entry: TagStackEntry) {
        if self.stack.len() >= 100 {
            self.stack.remove(0);
        }
        self.stack.push(entry);
    }

    /// Pop a position from the tag stack (`C-t`).
    pub fn pop(&mut self) -> Option<TagStackEntry> {
        self.stack.pop()
    }

    /// Current stack depth (for display in status messages).
    pub fn stack_depth(&self) -> usize {
        self.stack.len()
    }

    /// Clear the cache so the next load/ensure_loaded will regenerate tags.
    pub fn invalidate_cache(&mut self) {
        self.loaded_root = None;
        self.tags_mtime = None;
    }

    // ── Private ────────────────────────────────────────────────────────

    fn is_fresh(&self, repo_root: &Path) -> bool {
        self.loaded_root.as_deref() == Some(repo_root) && !self.tags.is_empty()
    }

    /// Locate an existing tags file, or generate one with `gentag` /
    /// `ctags` as fallback.
    fn find_or_generate(&self, repo_root: &Path) -> Option<PathBuf> {
        // 1. Check standard locations for an existing tags file in the repo root.
        let candidates = [
            repo_root.join("tags"),
            repo_root.join(".tags"),
            repo_root.join("GTAGS"),
        ];
        for path in &candidates {
            if path.exists() {
                return Some(path.clone());
            }
        }

        // 2. Generate with `gentag` (user's preferred generator).
        let tags_path = repo_root.join("tags");

        let out = std::process::Command::new("gentag")
            .current_dir(repo_root)
            .output();
        if let Ok(o) = out {
            if o.status.success() {
                // gentag may write to `tags` or `.tags` — check both.
                for path in &candidates {
                    if path.exists() {
                        return Some(path.clone());
                    }
                }
            }
        }

        // 3. Fallback: try `ctags -R`.
        let out = std::process::Command::new("ctags")
            .args(["-R", "--fields=+n", "-o"])
            .arg(&tags_path)
            .arg(".")
            .current_dir(repo_root)
            .output();
        if let Ok(o) = out {
            if o.status.success() && tags_path.exists() {
                return Some(tags_path);
            }
        }

        // 4. Try universal-ctags (`uctags`) as a last resort.
        let out = std::process::Command::new("uctags")
            .args(["-R", "--fields=+n", "-o"])
            .arg(&tags_path)
            .arg(".")
            .current_dir(repo_root)
            .output();
        if let Ok(o) = out {
            if o.status.success() && tags_path.exists() {
                return Some(tags_path);
            }
        }

        None
    }

    /// Parse a standard ctags-format file into `self.tags`.
    ///
    /// Handles both the simple format (`name<TAB>file<TAB>line`)
    /// and the extended format (`name<TAB>file<TAB>/^pat$/;"<TAB>fields`).
    fn parse(&mut self, content: &str) -> bool {
        for line in content.lines() {
            if line.is_empty() || line.starts_with('!') {
                continue; // skip !_TAG_ headers and blank lines
            }

            let fields: Vec<&str> = line.split('\t').collect();
            if fields.len() < 3 {
                continue;
            }

            let name = fields[0];
            let file = PathBuf::from(fields[1]);
            let third = fields[2];

            // ── Extract line number ────────────────────────────────────
            let mut line_num: Option<usize> = None;
            let mut kind: Option<String> = None;

            if let Ok(n) = third.parse::<usize>() {
                // Simple format: third field is just a line number.
                line_num = Some(n);
            } else {
                // Extended format — scan all fields past the second for
                // `line:N` and `kind:K` (or single-letter kind).
                let scan_start = if third.starts_with("/^")
                    || third.starts_with("/?")
                    || third.contains(";\"")
                {
                    3 // skip the pattern field
                } else {
                    2 // include third field in scan
                };

                for field in &fields[scan_start..] {
                    if let Some(rest) = field.strip_prefix("line:") {
                        line_num = line_num.or(rest.parse::<usize>().ok());
                    } else if let Some(rest) = field.strip_prefix("kind:") {
                        kind = Some(rest.to_string());
                    }
                }

                // Single-letter kind (ctags often emits just "f", "s", etc.)
                if kind.is_none() {
                    for field in &fields[scan_start..] {
                        if field.starts_with('"') || field.contains(':') {
                            continue;
                        }
                        if field.len() == 1
                            && field
                                .chars()
                                .next()
                                .map_or(false, |c| c.is_ascii_lowercase())
                        {
                            kind = Some(kind_name(field.chars().next().unwrap()));
                            break;
                        }
                    }
                }
            }

            let Some(line_num) = line_num else {
                continue; // cannot jump without a line number
            };

            self.tags
                .entry(name.to_lowercase())
                .or_default()
                .push(TagEntry {
                    name: name.to_string(),
                    file,
                    line: line_num,
                    kind,
                });
        }

        !self.tags.is_empty()
    }
}

// ═══════════════════════════════════════════════════════════════════════
// Helpers
// ═══════════════════════════════════════════════════════════════════════

/// Map a single-letter ctags kind code to a human-readable name.
fn kind_name(code: char) -> String {
    let name = match code {
        'f' => "function",
        'v' => "variable",
        's' => "struct",
        'c' => "class",
        'm' => "method",
        'p' => "property",
        't' => "typedef",
        'e' => "enum",
        'E' => "enumerator",
        'M' => "macro",
        'd' => "define",
        'g' => "enum",
        'n' => "namespace",
        'i' => "interface",
        'l' => "label",
        'r' => "import",
        'z' => "parameter",
        'F' => "field",
        _ => return code.to_string(),
    };
    name.to_string()
}
