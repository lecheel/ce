//! Buffer data model backed by a rope.
//!
//! A `Buffer` owns the text content (as a `Rope`), filename, undo stack,
//! and modified flag.  **Cursor position and scroll state have been moved
//! to [`crate::ed::window::Window`]** so that multiple windows can view
//! the same buffer with independent viewports.

use crate::ed::syntax::SyntaxState;
use anyhow::{Context, Result};
use ropey::Rope;
use std::path::Path;

// ---------------------------------------------------------------------------
// BufferKind — distinguishes normal files from special viewers
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub enum VirtualLine {
    /// A real line from the rope (0-based index into the rope).
    Real(usize),
    /// A virtual filler line with no content ("---").
    Padding,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BufferKind {
    /// A normal editable file on disk (or a scratch buffer).
    Normal,
    /// A read-only git-log viewer (tig-like).
    GitLog,
    /// A read-only git-diff viewer.
    GitDiff,
    GitDiffHead,
    Ripgrep,
    GitStatus,
    GitCommit,
    LlmInput,
    Llm,
    CheckHealth,
}

// ── GitSign (unchanged) ──────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GitSign {
    Added,
    Modified,
    Removed,
}

// ---------------------------------------------------------------------------
// Undo snapshot
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct UndoSnapshot {
    pub rope: Rope,
    /// Cursor row at the time the snapshot was taken (stored so that
    /// undo can restore the cursor position on the Window).
    pub cursor_row: usize,
    /// Cursor column at the time the snapshot was taken.
    pub cursor_col: usize,
    pub modified: bool,
}

// ---------------------------------------------------------------------------
// Buffer
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct Buffer {
    pub id: usize,
    pub rope: Rope,
    pub filename: Option<String>,
    pub modified: bool,
    pub undo_stack: Vec<UndoSnapshot>,
    pub syntax: SyntaxState,
    // Gutter States
    pub bookmarks: std::collections::HashSet<usize>, // 0-based document row numbers
    pub git_diffs: std::collections::HashMap<usize, GitSign>, // 0-based row to git status
    // ── Special buffer support ─────────────────────────────────────
    /// Kind of buffer — controls read-only semantics and key overrides.
    pub kind: BufferKind,
    /// Opaque state when `kind == BufferKind::GitLog`.
    pub git_log_state: Option<crate::git::log::GitLogState>,
    pub git_status_state: Option<crate::git::status::GitStatusState>,
    pub diff_alignment: Option<crate::ed::diff_align::DiffAlignment>,

    pub ripgrep_results: Vec<crate::ed::ripgrep::RipgrepResult>,
    pub ripgrep_line_map: Vec<Option<usize>>,
    pub search_pattern: Option<String>,
    pub named_bookmarks: std::collections::HashMap<char, (usize, usize)>,
}

impl Buffer {
    /// Mark the buffer as modified, but only if it's a normal editable buffer.
    /// Special buffers (GitLog, Ripgrep, LLM, etc.) never get marked dirty.
    #[inline]
    pub fn mark_modified(&mut self) {
        if self.kind == BufferKind::Normal
            || self.kind == BufferKind::LlmInput
            || self.kind == BufferKind::GitCommit
        {
            self.modified = true;
        }
    }
    // ---- Constructor ----
    /// Way 1: Fallback/Full parse trigger
    pub fn parse_syntax(&mut self) {
        let lang = match self.kind {
            BufferKind::GitStatus => "gitstatus".to_string(),
            BufferKind::GitDiff => "diff".to_string(),
            BufferKind::GitLog => "gitlog".to_string(),
            BufferKind::Ripgrep => "rg".to_string(),
            BufferKind::CheckHealth => "checkhealth".to_string(),
            BufferKind::GitDiffHead => detect_language(self.filename.as_deref()),
            _ => detect_language(self.filename.as_deref()),
        };
        self.syntax.parse_full(&self.rope, Some(&lang));
    }

    /// Way 2: Incremental parse trigger
    pub fn parse_syntax_incremental(&mut self, edit: tree_sitter::InputEdit) {
        let lang = match self.kind {
            BufferKind::GitStatus => "gitstatus".to_string(),
            BufferKind::GitDiff => "diff".to_string(),
            BufferKind::GitLog => "gitlog".to_string(),
            BufferKind::Ripgrep => "rg".to_string(),
            BufferKind::CheckHealth => "checkhealth".to_string(),
            _ => detect_language(self.filename.as_deref()),
        };
        self.syntax.parse_incremental(&self.rope, Some(&lang), edit);
    }

    pub fn new(id: usize, filename: Option<String>) -> Result<Self> {
        let mut buf = Self {
            id,
            rope: Rope::from_str(""),
            filename: None,
            modified: false,
            undo_stack: Vec::new(),
            syntax: SyntaxState::new(),

            bookmarks: std::collections::HashSet::new(),
            git_diffs: std::collections::HashMap::new(),

            kind: BufferKind::Normal,
            git_log_state: None,
            git_status_state: None,

            ripgrep_results: Vec::new(),
            ripgrep_line_map: Vec::new(),
            search_pattern: None,
            named_bookmarks: std::collections::HashMap::new(),
            diff_alignment: None,
        };

        if let Some(ref path) = filename {
            if Path::new(path).exists() {
                buf.open_file(path)?;
            }
            buf.filename = Some(path.clone());
        } else {
            buf.rope = Rope::from_str("\n");
        }

        buf.parse_syntax();
        // buf.update_mock_git_diffs();

        Ok(buf)
    }

    /// Rebuild `bookmarks` (row-only set used by the gutter) from
    /// `named_bookmarks`.
    pub fn sync_bookmark_rows(&mut self) {
        self.bookmarks.clear();
        for &(_row, _col) in self.named_bookmarks.values() {
            // Just row for the gutter indicator
            let _ = _col; // suppress unused warning
            self.bookmarks.insert(_row);
        }
    }

    /// Returns `true` for read-only special buffers (git log / diff).
    #[inline]
    pub fn is_readonly(&self) -> bool {
        matches!(
            self.kind,
            BufferKind::GitLog
                | BufferKind::GitDiff
                | BufferKind::GitDiffHead
                | BufferKind::Ripgrep
                | BufferKind::Llm
                | BufferKind::CheckHealth
        )
    }

    // ── Mock Git (unchanged) ───────────────────────────────────────

    pub fn update_mock_git_diffs(&mut self) {
        self.git_diffs.clear();
        let total = self.len_lines();
        if total > 5 {
            self.git_diffs.insert(1, GitSign::Added);
            self.git_diffs.insert(3, GitSign::Modified);
            self.git_diffs.insert(4, GitSign::Removed);
        }
    }

    // ---- File I/O ----
    pub fn open_file(&mut self, path: &str) -> Result<()> {
        let content =
            std::fs::read_to_string(path).with_context(|| format!("Cannot open {}", path))?;

        if content.is_empty() {
            self.rope = Rope::from_str("\n");
        } else if content.ends_with('\n') {
            self.rope = Rope::from_str(&content);
        } else {
            self.rope = Rope::from_str(&format!("{}\n", content));
        }

        self.modified = false;
        self.parse_syntax();
        Ok(())
    }

    pub fn save_file(&mut self, format_on_save: bool) -> Result<Option<String>> {
        // ── Reject saves for special buffers ────────────────────────
        if self.kind != BufferKind::Normal {
            anyhow::bail!("Cannot save a special buffer");
        }

        let filename = self.filename.clone();
        match filename {
            Some(path) => {
                let text = self.rope.to_string();
                std::fs::write(&path, &text).with_context(|| format!("Cannot write {}", path))?;

                let mut warning = None;

                if format_on_save {
                    if path.ends_with(".rs") {
                        match std::process::Command::new("rustfmt")
                            .arg("--edition")
                            .arg("2021")
                            .arg(&path)
                            .output()
                        {
                            Ok(output) if output.status.success() => {
                                if let Err(e) = self.open_file(&path) {
                                    warning = Some(format!(
                                        "Saved, but failed to reload after rustfmt: {}",
                                        e
                                    ));
                                }
                            }
                            Ok(output) => {
                                let stderr = String::from_utf8_lossy(&output.stderr);
                                warning = Some(format!("Saved, rustfmt failed: {}", stderr.trim()));
                            }
                            Err(e) => {
                                warning = Some(format!("Saved, but rustfmt not found: {}", e));
                            }
                        }
                    } else if path.ends_with(".py") || path.ends_with(".pyi") {
                        let formatted = match std::process::Command::new("ruff")
                            .args(["format", &path])
                            .output()
                        {
                            Ok(o) if o.status.success() => Some(o),
                            Ok(o) => {
                                let ruff_err =
                                    String::from_utf8_lossy(&o.stderr).trim().to_string();
                                // ruff failed — try black as fallback
                                match std::process::Command::new("black").arg(&path).output() {
                                    Ok(bo) if bo.status.success() => Some(bo),
                                    Ok(bo) => {
                                        let black_err =
                                            String::from_utf8_lossy(&bo.stderr).trim().to_string();
                                        warning = Some(format!(
                                            "Saved, but ruff ({}) and black ({}) both failed",
                                            ruff_err, black_err
                                        ));
                                        None
                                    }
                                    Err(_) => {
                                        warning = Some(format!(
                                            "Saved, but ruff ({}) failed and black not found",
                                            ruff_err
                                        ));
                                        None
                                    }
                                }
                            }
                            Err(_) => {
                                // ruff not found — try black
                                match std::process::Command::new("black").arg(&path).output() {
                                    Ok(bo) if bo.status.success() => Some(bo),
                                    Ok(bo) => {
                                        let black_err =
                                            String::from_utf8_lossy(&bo.stderr).trim().to_string();
                                        warning =
                                            Some(format!("Saved, but black failed: {}", black_err));
                                        None
                                    }
                                    Err(_) => {
                                        warning = Some(
                                            "Saved, but neither ruff nor black found for formatting"
                                                .to_string(),
                                        );
                                        None
                                    }
                                }
                            }
                        };

                        if let Some(_output) = formatted {
                            if let Err(e) = self.open_file(&path) {
                                warning = Some(format!(
                                    "Saved, but failed to reload after formatting: {}",
                                    e
                                ));
                            }
                        }
                    }
                }

                self.modified = false;
                Ok(warning)
            }
            None => anyhow::bail!("No filename — use :w <path>"),
        }
    }

    // ---- Rope helpers ----

    #[inline]
    pub fn len_lines(&self) -> usize {
        self.rope.len_lines()
    }

    pub fn line_text(&self, idx: usize) -> String {
        if idx >= self.rope.len_lines() {
            return String::new();
        }
        let start = self.rope.line_to_char(idx);
        let end = self.rope.line_to_char(idx + 1);
        if end > start {
            self.rope.slice(start..end - 1).to_string()
        } else {
            String::new()
        }
    }

    #[inline]
    pub fn line_char_len(&self, idx: usize) -> usize {
        if idx >= self.rope.len_lines() {
            return 0;
        }
        let start = self.rope.line_to_char(idx);
        let end = self.rope.line_to_char(idx + 1);
        end.saturating_sub(start).saturating_sub(1)
    }

    // ---- Undo ----

    /// Push an undo snapshot.  The caller passes the *window's* current
    /// cursor position so the snapshot can restore it later.
    pub fn push_undo(&mut self, cursor_row: usize, cursor_col: usize) {
        // Don't push undo for special (read-only) buffers
        if self.is_readonly() {
            return;
        }
        let snap = UndoSnapshot {
            rope: self.rope.clone(),
            cursor_row,
            cursor_col,
            modified: self.modified,
        };
        self.undo_stack.push(snap);
        if self.undo_stack.len() > 500 {
            self.undo_stack.remove(0);
        }
    }

    /// Pop the most recent undo snapshot.
    ///
    /// Returns `Some(UndoSnapshot)` if there was one, `None` otherwise.
    /// The caller is responsible for applying the rope/modified to the
    /// buffer and the cursor_row/cursor_col to the window.
    pub fn pop_undo(&mut self) -> Option<UndoSnapshot> {
        self.undo_stack.pop()
    }

    // ---- Display name ----

    pub fn display_name(&self) -> String {
        match self.kind {
            BufferKind::GitLog => "[Git Log]".to_string(),
            BufferKind::GitDiff => "[Git Diff]".to_string(),
            BufferKind::Ripgrep => "[Ripgrep]".to_string(),
            BufferKind::GitCommit => "[Git Commit]".to_string(),
            BufferKind::GitStatus => "[Git Status]".to_string(),
            BufferKind::CheckHealth => "[Check Health]".to_string(),
            BufferKind::GitDiffHead => self
                .filename
                .as_deref()
                .and_then(|p| {
                    // strip "git://head/" prefix, show just the filename
                    p.strip_prefix("git://head/")
                        .and_then(|rest| std::path::Path::new(rest).file_name())
                        .and_then(|n| n.to_str())
                })
                .unwrap_or("[HEAD]")
                .to_string(),

            // Add these two match arms here:
            BufferKind::LlmInput => "[LLM Prompt]".to_string(),
            BufferKind::Llm => "[LLM Conversation]".to_string(),

            BufferKind::Normal => self
                .filename
                .as_deref()
                .and_then(|p| Path::new(p).file_name())
                .and_then(|n| n.to_str())
                .unwrap_or("[No Name]")
                .to_string(),
        }
    }
    /// Get the current scope string (impl::function) at the given position.
    pub fn current_scope(&self, row: usize, col: usize) -> Option<String> {
        self.syntax.current_scope(&self.rope, row, col)
    }

    /// Calculates the summary of git diff signs.
    pub fn git_diff_stats(&self) -> (usize, usize, usize) {
        let mut added = 0;
        let mut modified = 0;
        let mut removed = 0;
        for sign in self.git_diffs.values() {
            match sign {
                GitSign::Added => added += 1,
                GitSign::Modified => modified += 1,
                GitSign::Removed => removed += 1,
            }
        }
        (added, modified, removed)
    }

    pub fn language_display_name(&self) -> &'static str {
        match self.syntax.language_id.as_deref().unwrap_or("") {
            "gitlog" => "Git Log",
            "gitstatus" => "Git Status",
            "rg" => "Ripgrep",
            "diff" => "Diff",
            "rust" => "Rust",
            "python" => "Python",
            "javascript" => "JavaScript",
            "typescript" => "TypeScript",
            "typescriptreact" => "TSX",
            "javascriptreact" => "JSX",
            "go" => "Go",
            "java" => "Java",
            "c" => "C",
            "cpp" => "C++",
            "ruby" => "Ruby",
            "php" => "PHP",
            "swift" => "Swift",
            "kotlin" => "Kotlin",
            "bash" => "Shell",
            "sql" => "SQL",
            "html" => "HTML",
            "css" => "CSS",
            "scss" => "SCSS",
            "json" => "JSON",
            "yaml" => "YAML",
            "markdown" => "Markdown",
            "lua" => "Lua",
            "dart" => "Dart",
            _ => "Plain Text",
        }
    }
}

// ---------------------------------------------------------------------------
// Language detection (unchanged)
// ---------------------------------------------------------------------------

pub fn detect_language(filename: Option<&str>) -> String {
    match filename.and_then(|p| p.rsplit('.').next()) {
        Some("rs") => "rust",
        Some("py") => "python",
        Some("js") => "javascript",
        Some("ts") => "typescript",
        Some("tsx") => "typescriptreact",
        Some("jsx") => "javascriptreact",
        Some("go") => "go",
        Some("java") => "java",
        Some("c") | Some("h") => "c",
        Some("cpp") | Some("hpp") | Some("cc") | Some("cxx") => "cpp",
        Some("rb") => "ruby",
        Some("php") => "php",
        Some("swift") => "swift",
        Some("kt") | Some("kts") => "kotlin",
        Some("sh") | Some("bash") => "bash",
        Some("sql") => "sql",
        Some("html") | Some("htm") => "html",
        Some("css") => "css",
        Some("scss") => "scss",
        Some("json") => "json",
        Some("yaml") | Some("yml") => "yaml",
        Some("md") | Some("markdown") => "markdown",
        Some("lua") => "lua",
        Some("vim") => "vim",
        Some("toml") | Some("zig") => "plaintext",
        Some("dart") => "dart",
        Some("diff") | Some("patch") => "diff",
        _ => "plaintext",
    }
    .to_string()
}

/// Find the ID of an existing `Buffer` whose filename resolves to the
/// same canonical path as `path`.
///
/// Uses `canonicalize` so that `"./src/main.rs"` and
/// `"/home/user/proj/src/main.rs"` are treated as the same file.
pub fn find_buffer_by_filename(buffers: &[Buffer], path: &Path) -> Option<usize> {
    let canonical = std::fs::canonicalize(path).ok()?;
    buffers
        .iter()
        .find(|buf| {
            buf.filename
                .as_ref()
                .and_then(|f| std::fs::canonicalize(f).ok())
                .map(|c| c == canonical)
                .unwrap_or(false)
        })
        .map(|buf| buf.id)
}
