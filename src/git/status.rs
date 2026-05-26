//! git/status.rs — Git status viewer as a special buffer.
//!
//! Displays staged and unstaged files with their status,
//! along with recent branch history and stashes.
//! Press 'c' to generate commit message using LLM.

use std::collections::HashMap;
use std::path::Path;
use std::path::PathBuf;
use std::process::Command;

// ---------------------------------------------------------------------------
// Data types
// ---------------------------------------------------------------------------

/// Status of a single file.
#[derive(Debug, Clone)]
pub struct FileStatus {
    pub path: String,
    pub status: FileStatusType,
    pub staged: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileStatusType {
    Added,
    Modified,
    Deleted,
    Renamed,
    Untracked,
    Conflicted,
}

impl FileStatusType {
    pub fn to_char(self) -> char {
        match self {
            FileStatusType::Added => 'A',
            FileStatusType::Modified => 'M',
            FileStatusType::Deleted => 'D',
            FileStatusType::Renamed => 'R',
            FileStatusType::Untracked => '?',
            FileStatusType::Conflicted => 'C',
        }
    }
}

/// What happens when the user presses Enter on a given display row.
#[derive(Debug, Clone)]
pub enum GitStatusLineAction {
    /// Open the file in a normal buffer.
    OpenFile { path: String },
    /// Switch to a branch.
    SwitchBranch { branch: String },
    /// Pop a stash entry.
    PopStash { stash_ref: String },
    /// No action (header/separator line).
    None,
}

/// Opaque state attached to a `BufferKind::GitStatus` buffer.
#[derive(Debug, Clone)]
pub struct GitStatusState {
    pub repo_root: PathBuf,
    pub files: Vec<FileStatus>,
    /// Maps 0-based display row → action for that row.
    pub line_actions: HashMap<usize, GitStatusLineAction>,
    pub has_staged_changes: bool,
}

// ---------------------------------------------------------------------------
// Parsing & Loading
// ---------------------------------------------------------------------------

impl GitStatusState {
    /// Run `git status --porcelain=v1` as well as branch and stash commands in
    /// `repo_root`, parse the results, and return `(state, display_text_for_rope)`.
    ///
    /// Returns `None` when git is unavailable or not a repo.
    pub fn load(repo_root: &Path) -> Option<(Self, String)> {
        // 1. Fetch status porcelain
        let status_output = Command::new("git")
            .args(["status", "--porcelain=v1"])
            .current_dir(repo_root)
            .output()
            .ok()?;

        if !status_output.status.success() {
            return None;
        }
        let status_text = String::from_utf8_lossy(&status_output.stdout);

        // 2. Fetch recent branch history sorted by committer date (relative)
        let branch_text = Command::new("git")
            .args([
                "for-each-ref",
                "--sort=-committerdate",
                "--count=10",
                "--format=%(refname:short) %(committerdate:relative)",
                "refs/heads/",
            ])
            .current_dir(repo_root)
            .output()
            .ok()
            .map(|o| String::from_utf8_lossy(&o.stdout).to_string())
            .unwrap_or_default();

        // 3. Fetch list of stashes
        let stash_text = Command::new("git")
            .args(["stash", "list"])
            .current_dir(repo_root)
            .output()
            .ok()
            .map(|o| String::from_utf8_lossy(&o.stdout).to_string())
            .unwrap_or_default();

        Self::parse(&status_text, &branch_text, &stash_text, repo_root)
    }

    fn parse(
        status_text: &str,
        branch_text: &str,
        stash_text: &str,
        repo_root: &Path,
    ) -> Option<(Self, String)> {
        let mut files: Vec<FileStatus> = Vec::new();
        let mut line_actions: HashMap<usize, GitStatusLineAction> = HashMap::new();
        let mut display_lines: Vec<String> = Vec::new();
        let mut has_staged_changes = false;

        let mut staged_files = Vec::new();
        let mut unstaged_files = Vec::new();
        let mut untracked_files = Vec::new();

        for raw_line in status_text.lines() {
            if raw_line.is_empty() {
                continue;
            }

            // Parse porcelain format: XY path
            // X = staged status, Y = unstaged status
            // For renames: XY old_path -> new_path
            // We must NOT trim raw_line here to preserve position columns at index 0 and 1
            let (staged_char, unstaged_char, path) = if raw_line.len() >= 3 {
                let staged = raw_line.chars().next().unwrap();
                let unstaged = raw_line.chars().nth(1).unwrap();
                let path_part = &raw_line[3..];

                // Handle renames (R  old -> new)
                let final_path = if path_part.contains(" -> ") {
                    path_part.split(" -> ").last().unwrap_or(path_part)
                } else {
                    path_part
                };

                // Strip quotes if git escaped paths containing spaces
                (staged, unstaged, final_path.trim_matches('"').to_string())
            } else {
                continue;
            };

            let is_staged = staged_char != ' ' && staged_char != '?';
            let is_unstaged = unstaged_char != ' ' && unstaged_char != '?';
            let is_untracked = staged_char == '?' && unstaged_char == '?';

            let status_type = if is_untracked {
                FileStatusType::Untracked
            } else if is_staged {
                has_staged_changes = true;
                match staged_char {
                    'A' => FileStatusType::Added,
                    'M' => FileStatusType::Modified,
                    'D' => FileStatusType::Deleted,
                    'R' => FileStatusType::Renamed,
                    'C' => FileStatusType::Conflicted,
                    _ => FileStatusType::Modified,
                }
            } else {
                match unstaged_char {
                    'M' => FileStatusType::Modified,
                    'D' => FileStatusType::Deleted,
                    _ => FileStatusType::Modified,
                }
            };

            let file_status = FileStatus {
                path: path.clone(),
                status: status_type,
                staged: is_staged && !is_untracked,
            };

            files.push(file_status.clone());

            if is_untracked {
                untracked_files.push(file_status);
            } else if is_staged {
                staged_files.push(file_status);
            } else {
                unstaged_files.push(file_status);
            }
        }

        // ── 1. Render Stage Changes section ──────────────────────────────
        display_lines.push(String::new());
        display_lines.push(format!("  Stage Changes ({})", staged_files.len()));
        display_lines.push(format!("  {}", "─".repeat(40)));

        if staged_files.is_empty() {
            display_lines.push("    (none)".to_string());
        } else {
            for file in &staged_files {
                let idx = display_lines.len();
                display_lines.push(format!("   {}", file.path)); // Indented with space, no indicator symbol
                line_actions.insert(
                    idx,
                    GitStatusLineAction::OpenFile {
                        path: file.path.clone(),
                    },
                );
            }
        }

        // ── 2. Render Unstage Changes section ────────────────────────────
        display_lines.push(String::new());
        display_lines.push(format!("  Unstage Changes ({})", unstaged_files.len()));
        display_lines.push(format!("  {}", "─".repeat(40)));

        if unstaged_files.is_empty() {
            display_lines.push("    (none)".to_string());
        } else {
            for file in &unstaged_files {
                let idx = display_lines.len();
                display_lines.push(format!("  {}", file.path)); // Enclosed in brackets [path]
                line_actions.insert(
                    idx,
                    GitStatusLineAction::OpenFile {
                        path: file.path.clone(),
                    },
                );
            }
        }

        // ── 3. Render Untracked Files section ───────────────────────────
        display_lines.push(String::new());
        display_lines.push(format!("  Untracked Files ({})", untracked_files.len()));
        display_lines.push(format!("  {}", "─".repeat(40)));

        if untracked_files.is_empty() {
            display_lines.push("    (none)".to_string());
        } else {
            for file in &untracked_files {
                let idx = display_lines.len();
                display_lines.push(format!("    {}", file.path));
                line_actions.insert(
                    idx,
                    GitStatusLineAction::OpenFile {
                        path: file.path.clone(),
                    },
                );
            }
        }

        // ── 4. Render Branches section ────────────────────────────────
        display_lines.push(String::new());
        display_lines.push("  ------ Branch ------".to_string());
        display_lines.push(format!("  {}", "─".repeat(40)));

        let active_branch = Command::new("git")
            .args(["symbolic-ref", "--short", "HEAD"])
            .current_dir(repo_root)
            .output()
            .ok()
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
            .unwrap_or_default();

        let mut branch_count = 0;
        for line in branch_text.lines() {
            let trimmed = line.trim();
            if trimmed.is_empty() {
                continue;
            }
            branch_count += 1;
            let is_current = !active_branch.is_empty() && trimmed.starts_with(&active_branch);
            let indicator = if is_current { "* " } else { "  " };

            // Extract the actual branch name (the first word of the line)
            let branch_name = trimmed
                .split_whitespace()
                .next()
                .unwrap_or(trimmed)
                .to_string();

            let idx = display_lines.len();
            display_lines.push(format!("    {}{}", indicator, trimmed));

            // Map this specific line index to branch switching
            line_actions.insert(
                idx,
                GitStatusLineAction::SwitchBranch {
                    branch: branch_name,
                },
            );
        }
        if branch_count == 0 {
            display_lines.push("    (none)".to_string());
        }

        // ── 5. Render Stashes section ─────────────────────────────────
        display_lines.push(String::new());
        display_lines.push("  ------ Stash ------".to_string());
        display_lines.push(format!("  {}", "─".repeat(40)));

        let mut stash_count = 0;
        for line in stash_text.lines() {
            let trimmed = line.trim();
            if trimmed.is_empty() {
                continue;
            }
            if stash_count >= 10 {
                break;
            }
            stash_count += 1;

            // Extract the stash reference (e.g., "stash@{0}" from "stash@{0}: WIP on master...")
            let stash_ref = trimmed.split(':').next().unwrap_or(trimmed).to_string();

            let idx = display_lines.len();
            display_lines.push(format!("    {}", trimmed));

            // Map this specific line index to the pop action
            line_actions.insert(idx, GitStatusLineAction::PopStash { stash_ref });
        }
        if stash_count == 0 {
            display_lines.push("    (none)".to_string());
        }

        // ── Empty repo fallback message ──────────────────────────────
        if files.is_empty() {
            display_lines.push(String::new());
            display_lines.push("  No changes detected".to_string());
            display_lines.push(String::new());
        }

        // ── Footer actions ───────────────────────────────────────────
        display_lines.push(String::new());
        display_lines.push(format!("  {}", "─".repeat(40)));
        if has_staged_changes {
            display_lines.push("  [c] Commit staged changes with LLM".to_string());
        } else if !files.is_empty() {
            display_lines.push("  [c] Stage all and commit with LLM".to_string());
        }
        display_lines
            .push("  [s] Toggle staged  [Enter] Open file  [z] stash [q] Close".to_string());

        if display_lines.is_empty() {
            return None;
        }

        // Ensure trailing newline
        let display_text = format!("{}\n", display_lines.join("\n"));

        Some((
            Self {
                repo_root: repo_root.to_path_buf(),
                files,
                line_actions,
                has_staged_changes,
            },
            display_text,
        ))
    }

    /// Look up the action for a given 0-based display row.
    #[inline]
    pub fn action_for_line(&self, line: usize) -> Option<&GitStatusLineAction> {
        self.line_actions.get(&line)
    }
}
