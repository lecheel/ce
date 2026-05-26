//! git/log.rs — tig-like git log viewer as a special buffer.
//!
//! Displays a structured git log with per-commit file changes.
//! Pressing Enter on a file line opens that file; pressing Enter on
//! a commit header shows the full diff.  The buffer intercepts all
//! keys so it behaves like its own mini-mode.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Command;

// ---------------------------------------------------------------------------
// Data types
// ---------------------------------------------------------------------------

/// One commit entry in the git log.
#[derive(Debug, Clone)]
pub struct GitLogEntry {
    pub hash_full: String,
    pub hash_short: String,
    pub author: String,
    pub date: String,
    pub subject: String,
    /// `(status_letter, file_path)` — status is M / A / D / R / C / T …
    pub files: Vec<(String, String)>,
}

/// What happens when the user presses Enter on a given display row.
#[derive(Debug, Clone)]
pub enum GitLogLineAction {
    /// Open the file (at HEAD) in a normal buffer.
    OpenFile { path: String, commit: String },
    /// Open a GitDiff buffer showing the full commit diff.
    ShowDiff { commit: String },
}

/// Opaque state attached to a `BufferKind::GitLog` buffer.
#[derive(Debug, Clone)]
pub struct GitLogState {
    pub repo_root: PathBuf,
    pub entries: Vec<GitLogEntry>,
    /// Maps 0-based display row → action for that row.
    pub line_actions: HashMap<usize, GitLogLineAction>,
}

// ---------------------------------------------------------------------------
// Parsing
// ---------------------------------------------------------------------------

impl GitLogState {
    /// Run `git log` in `repo_root`, parse the output, and return
    /// `(state, display_text_for_rope)`.
    ///
    /// Returns `None` when git is unavailable or the repo has no commits.
    pub fn load(repo_root: &Path) -> Option<(Self, String)> {
        let output = Command::new("git")
            .args([
                "log",
                "-500", // cap at 500 commits for performance
                "--pretty=format:_COMMIT_%x00%H%x00%h%x00%an%x00%ad%x00%s",
                "--date=short",
                "--name-status",
            ])
            .current_dir(repo_root)
            .output()
            .ok()?;

        if !output.status.success() {
            return None;
        }

        let text = String::from_utf8_lossy(&output.stdout);
        Self::parse(&text, repo_root)
    }

    fn parse(log_text: &str, repo_root: &Path) -> Option<(Self, String)> {
        let mut entries: Vec<GitLogEntry> = Vec::new();
        let mut line_actions: HashMap<usize, GitLogLineAction> = HashMap::new();
        let mut display_lines: Vec<String> = Vec::new();

        let mut current_entry: Option<GitLogEntry> = None;

        for raw_line in log_text.lines() {
            // ── Commit delimiter ────────────────────────────────────
            if let Some(rest) = raw_line.strip_prefix("_COMMIT_\x00") {
                // Finish the previous entry
                if let Some(entry) = current_entry.take() {
                    entries.push(entry);
                    display_lines.push(String::new()); // blank line between entries
                }

                let parts: Vec<&str> = rest.split('\x00').collect();
                if parts.len() < 5 {
                    continue;
                }

                let entry = GitLogEntry {
                    hash_full: parts[0].to_string(),
                    hash_short: parts[1].to_string(),
                    author: parts[2].to_string(),
                    date: parts[3].to_string(),
                    subject: parts[4].to_string(),
                    files: Vec::new(),
                };

                // ── Render commit header ─────────────────────────────
                let header_idx = display_lines.len();
                display_lines.push(format!(
                    "{} {}  {}",
                    entry.date, entry.hash_short, entry.author
                ));
                line_actions.insert(
                    header_idx,
                    GitLogLineAction::ShowDiff {
                        commit: entry.hash_full.clone(),
                    },
                );

                let subject_idx = display_lines.len();
                display_lines.push(format!("  {}", entry.subject));
                line_actions.insert(
                    subject_idx,
                    GitLogLineAction::ShowDiff {
                        commit: entry.hash_full.clone(),
                    },
                );

                current_entry = Some(entry);
                continue;
            }

            // ── File-status line (M\tpath / A\tpath / R100\told\tnew) ─
            if let Some(ref mut entry) = current_entry {
                let trimmed = raw_line.trim();
                if trimmed.is_empty() {
                    continue;
                }

                let tab_parts: Vec<&str> = trimmed.splitn(3, '\t').collect();
                if tab_parts.len() >= 2 {
                    let status = tab_parts[0].trim().to_string();
                    // For renames, `tab_parts[2]` is the new path.
                    let path = if tab_parts.len() == 3 && status.starts_with('R') {
                        tab_parts[2].to_string()
                    } else {
                        tab_parts[1].to_string()
                    };

                    let status_char = match status.as_str() {
                        "A" => "+",
                        "D" => "−",
                        "M" => "~",
                        "R" => ">",
                        "C" => "=",
                        "T" => "T",
                        other => other,
                    };

                    let file_idx = display_lines.len();
                    display_lines.push(format!("    {} {}", status_char, path));

                    // `status` is moved here, but `status_char` borrow has already been consumed by format! above
                    entry.files.push((status, path.clone()));

                    line_actions.insert(
                        file_idx,
                        GitLogLineAction::OpenFile {
                            path,
                            commit: entry.hash_full.clone(),
                        },
                    );
                }
            }
        }

        // Flush the last entry
        if let Some(entry) = current_entry.take() {
            entries.push(entry);
        }

        if display_lines.is_empty() {
            return None;
        }

        // Ensure trailing newline (rope convention)
        let display_text = format!("{}\n", display_lines.join("\n"));

        Some((
            Self {
                repo_root: repo_root.to_path_buf(),
                entries,
                line_actions,
            },
            display_text,
        ))
    }

    /// Look up the action for a given 0-based display row.
    #[inline]
    pub fn action_for_line(&self, line: usize) -> Option<&GitLogLineAction> {
        self.line_actions.get(&line)
    }
}

// ---------------------------------------------------------------------------
// Git-diff buffer content
// ---------------------------------------------------------------------------

/// Run `git show` for a commit and return the display text for a
/// `BufferKind::GitDiff` buffer.
pub fn load_commit_diff(repo_root: &Path, commit: &str) -> Option<String> {
    let output = Command::new("git")
        .args(["show", "--stat", "--patch", "--no-color", commit])
        .current_dir(repo_root)
        .output()
        .ok()?;

    if !output.status.success() {
        return None;
    }

    let text = String::from_utf8_lossy(&output.stdout);
    if text.is_empty() {
        return None;
    }

    Some(format!("commit {}\n\n{}\n", commit, text))
}
