// src/ed/git_commit.rs
//! Git commit message generation using LLM.
//!
//! Provides a special buffer that gathers git diffs and recent commits,
//! sends them to the LLM, and allows the user to edit/confirm the message.

use crate::ed::buffer::{Buffer, BufferKind};
use crate::ed::editor::Editor;
use crate::ed::mode::MessageKind;
use ropey::Rope;
use std::path::PathBuf;

const SPINNER_CHARS: &[char] = &['⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧', '⠇', '⠏'];

const COMMIT_PROMPT_TEMPLATE: &str = r#"Generate a git commit message following this structure no explanation just the core:
1. First line: conventional commit format (type: concise description) (use semantic types like feat, fix, docs, style, refactor, perf, test, chore, etc.).
2. Optional bullet points if necessary:
   - Keep the second line blank
   - Keep them short and direct
   - Focus on what changed
   - Avoid overly formal or fluffy language

Examples:

feat: add user auth system

- Add JWT tokens for API auth
- Handle token refresh for long sessions

fix: resolve memory leak in worker pool

- Clean up idle connections
- Add timeout for stale workers

Simple change example:
fix: typo in README.md

Your message must be based on the provided git diff, with a bit of styling from recent commits.
Recent commits for reference:
{recent_commits}
Git diff:
{diff}"#;

impl Editor {
    /// Open a GitCommit buffer and ask the LLM to generate a message.
    pub fn git_commit_generate(&mut self) {
        // ── 1. Locate git root ──────────────────────────────
        let start_dir = self
            .buf()
            .filename
            .as_ref()
            .and_then(|p| std::path::Path::new(p).parent())
            .map(|p| p.to_path_buf())
            .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));

        let git_root = match crate::git::gutter::find_git_root(&start_dir) {
            Some(root) => root,
            None => {
                self.set_status_msg("Not in a git repository", MessageKind::Error);
                return;
            }
        };

        // ── 2. Gather staged diff first, fall back to unstaged ──
        let staged_output = match std::process::Command::new("git")
            .args(["diff", "--cached", "-U3", "--diff-algorithm=minimal"])
            .current_dir(&git_root)
            .output()
        {
            Ok(o) => String::from_utf8_lossy(&o.stdout).to_string(),
            Err(e) => {
                self.set_status_msg(
                    &format!("Failed to run git diff --cached: {}", e),
                    MessageKind::Error,
                );
                return;
            }
        };

        let unstaged_output = match std::process::Command::new("git")
            .args(["diff", "-U3", "--diff-algorithm=minimal"])
            .current_dir(&git_root)
            .output()
        {
            Ok(o) => String::from_utf8_lossy(&o.stdout).to_string(),
            Err(e) => {
                self.set_status_msg(
                    &format!("Failed to run git diff: {}", e),
                    MessageKind::Error,
                );
                return;
            }
        };

        if staged_output.trim().is_empty() && unstaged_output.trim().is_empty() {
            self.set_status_msg("No staged or unstaged changes found", MessageKind::Error);
            return;
        }

        let diff_output = if !staged_output.trim().is_empty() && !unstaged_output.trim().is_empty()
        {
            format!(
                "Staged changes:\n{}\nUnstaged changes:\n{}",
                staged_output, unstaged_output
            )
        } else if !staged_output.trim().is_empty() {
            staged_output
        } else {
            format!("Unstaged changes:\n{}", unstaged_output)
        };

        // ── 3. Get last 2 commit messages (deduplicated) ─────
        let recent_commits = match std::process::Command::new("git")
            .args(["log", "-2", "--format=%B"])
            .current_dir(&git_root)
            .output()
        {
            Ok(o) if o.status.success() => String::from_utf8_lossy(&o.stdout).trim().to_string(),
            _ => "(no recent commits)".to_string(),
        };

        // ── 4. Build the user prompt containing the diff ──
        let user_prompt = COMMIT_PROMPT_TEMPLATE
            .replace("{recent_commits}", &recent_commits)
            .replace("{diff}", &diff_output);

        // Clear any leftover system_prompt override from a previous call
        self.llm.system_prompt = None;

        // ── 5. Create / reuse GitCommit buffer ───────────────
        self.git_commit_start_time = Some(std::time::Instant::now());

        let target_filename = "*git-commit*";
        let existing_id = self
            .buffers
            .iter()
            .find(|b| b.filename.as_deref() == Some(target_filename))
            .map(|b| b.id);

        let buffer_id = if let Some(id) = existing_id {
            if let Some(buf) = self.buf_mut_by_id(id) {
                buf.rope = Rope::from_str(&format!(
                    "  [COMMIT] Generating commit message… 0.0s ⠋\n  {}\n\n  (querying LLM, please wait...)\n",
                    "─".repeat(40)
                ));
                buf.modified = false;
            }
            id
        } else {
            let id = self.next_buf_id;
            self.next_buf_id += 1;

            let buf = Buffer {
                id,
                rope: Rope::from_str(&format!(
                    "  [COMMIT] Generating commit message… 0.0s ⠋\n  {}\n\n  (querying LLM, please wait...)\n",
                    "─".repeat(40)
                )),
                filename: Some(target_filename.to_string()),
                modified: false,
                undo_stack: Vec::new(),
                syntax: crate::ed::syntax::SyntaxState::new(),
                bookmarks: std::collections::HashSet::new(),
                git_diffs: std::collections::HashMap::new(),
                named_bookmarks: std::collections::HashMap::new(),
                kind: BufferKind::GitCommit,
                git_log_state: None,
                git_status_state: None,
                ripgrep_results: Vec::new(),
                ripgrep_line_map: Vec::new(),
                search_pattern: None,
                diff_alignment: None,
            };

            self.buffers.push(buf);
            id
        };

        // ── Switch to the commit buffer ──
        self.active_window_mut().save_jump_position();
        self.switch_window_to_buffer(buffer_id);
        self.git_commit_buffer_id = Some(buffer_id);

        // ── 6. Fire the LLM request using structured messages ──
        log::debug!(
            "[GitCommit] Spawning LLM request. Prompt length: {}",
            user_prompt.len()
        );

        let messages = vec![
            (
                "system".to_string(),
                "You are a git commit message generator. Generate concise, conventional-commit-style \
                 messages based on diffs. Output ONLY the commit message text — no explanations, \
                 no markdown code fences, no preamble."
                    .to_string(),
            ),
            ("user".to_string(), user_prompt),
        ];

        self.spawn_llm_request(messages);

        self.set_status_msg("Generating commit message…", MessageKind::Info);
    }

    /// Route a successful LLM response into the commit buffer.
    /// Call this from your LLM poller.
    pub fn git_commit_on_llm_response(&mut self, response: &str) -> bool {
        log::debug!(
            "[GitCommit] Received LLM response. Length: {}",
            response.len()
        );

        self.git_commit_start_time = None;

        let commit_buf_id = match self.git_commit_buffer_id {
            Some(id) => id,
            None => return false,
        };

        let cleaned = clean_llm_response(response);

        // Fetch git status to display at the bottom
        let git_status_output = self.get_git_status_for_commit();

        let mut status_lines = String::new();
        for line in git_status_output.lines() {
            status_lines.push_str(&format!("# {}\n", line));
        }

        let content = format!(
            "{}\n\n# ── w to commit  |  q to cancel ──\n# Lines starting with # are ignored\n#\n{}",
            cleaned.trim_end(),
            status_lines.trim_end()
        );

        if let Some(buf) = self.buf_mut_by_id(commit_buf_id) {
            buf.rope = Rope::from_str(&content);
            buf.mark_modified();
        }

        // Move cursor to the top
        let win = self.active_window_mut();
        win.row = 0;
        win.col = 0;
        win.desired_col = 0;

        self.set_status_msg(
            "Commit message generated — edit and 'w' to commit",
            MessageKind::Info,
        );
        true
    }

    /// Route an LLM error into the commit buffer.
    pub fn git_commit_on_llm_error(&mut self, error: &str) -> bool {
        self.git_commit_start_time = None;

        if self.git_commit_buffer_id.is_none() {
            return false;
        }

        if let Some(commit_buf_id) = self.git_commit_buffer_id {
            if let Some(buf) = self.buf_mut_by_id(commit_buf_id) {
                buf.rope = Rope::from_str(&format!(
                    "# Error generating commit message:\n# {}\n\n# Press q/Esc to cancel",
                    error
                ));
                buf.mark_modified();
            }
        }

        self.set_status_msg(&format!("LLM error: {}", error), MessageKind::Error);
        true
    }

    /// Close the commit buffer without committing.
    pub fn git_commit_close(&mut self) {
        self.git_commit_start_time = None;
        self.git_commit_buffer_id = None;

        let target_filename = "*git-commit*";
        if let Some(id) = self
            .buffers
            .iter()
            .find(|b| b.filename.as_deref() == Some(target_filename))
            .map(|b| b.id)
        {
            // Cancel any in-flight LLM request
            if let Some(handle) = self.llm.task_handle.take() {
                handle.abort();
            }

            // Switch back to a normal buffer
            let normal_id = self
                .buffers
                .iter()
                .find(|b| b.kind == BufferKind::Normal && b.filename.is_some())
                .or_else(|| self.buffers.iter().find(|b| b.kind == BufferKind::Normal))
                .map(|b| b.id);

            if let Some(nid) = normal_id {
                self.switch_window_to_buffer(nid);
            }

            self.close_buffer_by_id(id);
            self.set_status_msg("Commit cancelled", MessageKind::Info);
        }
    }

    /// Stage only tracked modified files and then generate a commit message.
    /// Called from git status buffer when user presses 'c'.
    ///
    /// Uses `git add -u` (update tracked files only) rather than `git add -A`
    /// to avoid accidentally staging untracked files unrelated to the current work.
    pub fn git_commit_stage_all_and_generate(&mut self) {
        let start_dir = self
            .buf()
            .filename
            .as_ref()
            .and_then(|p| std::path::Path::new(p).parent())
            .map(|p| p.to_path_buf())
            .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));

        let git_root = match crate::git::gutter::find_git_root(&start_dir) {
            Some(root) => root,
            None => {
                self.set_status_msg("Not in a git repository", MessageKind::Error);
                return;
            }
        };

        // Stage only already-tracked modified files; leave untracked files alone.
        let output = match std::process::Command::new("git")
            .args(["add", "-u"])
            .current_dir(&git_root)
            .output()
        {
            Ok(o) => o,
            Err(e) => {
                self.set_status_msg(&format!("Failed to run git add: {}", e), MessageKind::Error);
                return;
            }
        };

        if !output.status.success() {
            let msg = String::from_utf8_lossy(&output.stderr);
            self.set_status_msg(
                &format!("git add failed: {}", msg.trim()),
                MessageKind::Error,
            );
            return;
        }

        self.set_status_msg(
            "Staged tracked changes, generating commit message…",
            MessageKind::Info,
        );
        self.git_commit_generate();
    }

    /// Called when user presses 'w' in the commit buffer.
    pub fn handle_commit_write(&mut self) {
        // ── Don't commit while the LLM is still streaming ──
        if self.git_commit_start_time.is_some() {
            self.set_status_msg("Wait for LLM response to finish…", MessageKind::Info);
            return;
        }

        let commit_buf_id = match self.git_commit_buffer_id {
            Some(id) => id,
            None => {
                self.set_status_msg("No commit buffer active", MessageKind::Error);
                return;
            }
        };

        let (text, start_dir) = match self.buf_by_id(commit_buf_id) {
            Some(buf) => {
                let text = buf.rope.to_string();
                let dir = buf
                    .filename
                    .as_ref()
                    .and_then(|p| std::path::Path::new(p).parent())
                    .map(|p| p.to_path_buf())
                    .unwrap_or_else(|| {
                        std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."))
                    });
                (text, dir)
            }
            None => {
                self.set_status_msg("Commit buffer not found", MessageKind::Error);
                return;
            }
        };

        // Pre-flight: ensure there is at least one non-comment, non-empty line
        let has_real_content = text.lines().any(|line| {
            let trimmed = line.trim();
            !trimmed.is_empty() && !trimmed.starts_with('#')
        });

        if !has_real_content {
            self.set_status_msg(
                "Aborting commit due to empty commit message.",
                MessageKind::Error,
            );
            return;
        }

        let git_root = match crate::git::gutter::find_git_root(&start_dir) {
            Some(root) => root,
            None => {
                self.set_status_msg("Not a git repository", MessageKind::Error);
                return;
            }
        };

        // ── Execute git commit -F - ──
        use std::io::Write;
        let mut child = match std::process::Command::new("git")
            .args(["commit", "--cleanup=strip", "-F", "-"])
            .current_dir(&git_root)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
        {
            Ok(c) => c,
            Err(e) => {
                self.set_status_msg(
                    &format!("Failed to run git commit: {}", e),
                    MessageKind::Error,
                );
                return;
            }
        };

        if let Some(mut stdin) = child.stdin.take() {
            let _ = stdin.write_all(text.as_bytes());
        }

        match child.wait_with_output() {
            Ok(output) if output.status.success() => {
                let stdout = String::from_utf8_lossy(&output.stdout);
                let summary = stdout
                    .lines()
                    .next()
                    .unwrap_or("Committed successfully")
                    .to_string();

                self.git_commit_buffer_id = None;
                self.git_commit_close();
                self.set_status_msg(&summary, MessageKind::Success);
            }
            Ok(output) => {
                let stderr = String::from_utf8_lossy(&output.stderr);
                let stdout = String::from_utf8_lossy(&output.stdout);
                let msg = if stderr.trim().is_empty() {
                    stdout.trim().to_string()
                } else {
                    stderr.trim().to_string()
                };
                self.set_status_msg(&format!("git commit failed: {}", msg), MessageKind::Error);
            }
            Err(e) => {
                self.set_status_msg(
                    &format!("Failed to wait for git commit: {}", e),
                    MessageKind::Error,
                );
            }
        }
    }

    /// Animate the git commit buffer while LLM is generating.
    /// Call from your main loop tick.
    pub fn tick_git_commit(&mut self) {
        if self.git_commit_buffer_id.is_some() && self.git_commit_start_time.is_some() {
            if let Some(start) = self.git_commit_start_time {
                self.tick_spinner();
                let elapsed = start.elapsed().as_secs_f32();
                let spinner = SPINNER_CHARS[self.spinner_frame() % SPINNER_CHARS.len()];

                let status_msg = format!("{} Generating commit ({:.1}s)", spinner, elapsed);
                self.set_status_msg(&status_msg, MessageKind::Info);

                if let Some(id) = self.git_commit_buffer_id {
                    if let Some(buf) = self.buf_mut_by_id(id) {
                        let header = format!(
                            "  [COMMIT] Generating commit message… {:.1}s {}\n  {}\n\n  querying LLM, please wait ...\n",
                            elapsed,
                            spinner,
                            "─".repeat(40)
                        );
                        buf.rope = Rope::from_str(&header);
                    }
                }
            }
        }
    }

    /// Fetch `git status --short` for displaying in the commit buffer.
    fn get_git_status_for_commit(&self) -> String {
        let start_dir = self
            .buf()
            .filename
            .as_ref()
            .and_then(|p| std::path::Path::new(p).parent())
            .map(|p| p.to_path_buf())
            .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));

        if let Some(git_root) = crate::git::gutter::find_git_root(&start_dir) {
            match std::process::Command::new("git")
                .args(["status", "--short", "--untracked-files=no"])
                .current_dir(&git_root)
                .output()
            {
                Ok(output) if output.status.success() => {
                    let stdout = String::from_utf8_lossy(&output.stdout);
                    let mut lines = vec!["".to_string()];

                    for line in stdout.lines() {
                        let trimmed = line.trim_start();
                        if !trimmed.is_empty() {
                            lines.push(format!("     {}", trimmed));
                        }
                    }

                    lines.join("\n")
                }
                _ => "(failed to get status)".to_string(),
            }
        } else {
            "(not a git repo)".to_string()
        }
    }
}

// ── Helper functions ────────────────────────────────────────────────

/// Strip markdown code fences and trim the LLM response.
fn clean_llm_response(text: &str) -> String {
    let mut result = text.trim().to_string();

    // Opening fence with optional language tag: ```text\n …
    if result.starts_with("```") {
        if let Some(nl) = result.find('\n') {
            result = result[nl + 1..].to_string();
        }
    }
    // Closing fence
    if result.ends_with("```") {
        result.truncate(result.len() - 3);
    }

    result.trim().to_string()
}
