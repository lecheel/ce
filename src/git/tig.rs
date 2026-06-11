use crate::ed::buffer::BufferKind;
use crate::ed::syntax::SyntaxState;
use crate::ed::Buffer;
use crate::git::log::{GitLogLineAction, GitLogState};
use crate::Editor;

// ═══════════════════════════════════════════════════════════════════════════
// Tig-style Git Log — VSplit Diff (business logic only)
// Key handlers live in ed/handle/git.rs to avoid duplicate definitions.
// ═══════════════════════════════════════════════════════════════════════════

impl Editor {
    /// Open a vertical split showing the git diff for a commit or file-in-commit.
    /// Left pane keeps GitLog; right pane shows the diff.
    /// Uses a 1:3 ratio (25% log, 75% diff) — tig-style layout.
    pub fn open_git_log_diff_vsplit(&mut self, commit: &str, file_path: Option<&str>) {
        let log_win_id = self.active_window().id;
        let existing_sibling = self.active_window().diff_sibling;

        // If a diff pane is already open, just refresh it
        if let Some(sib_win_id) = existing_sibling {
            self.refresh_git_log_diff(sib_win_id, commit, file_path);
            // Focus the diff pane
            if let Some(idx) = self.windows.iter().position(|w| w.id == sib_win_id) {
                self.active_window_idx = idx;
            }
            return;
        }

        // 1. Create the diff buffer
        let diff_buf_id = self.create_git_log_diff_buffer(commit, file_path);

        // 2. Create a new window for the diff
        let diff_win_id = self.next_win_id;
        self.next_win_id += 1;
        let mut diff_win = crate::ed::window::Window::new(diff_win_id, diff_buf_id);
        diff_win.diff_sibling = Some(log_win_id);
        self.windows.push(diff_win);

        // 3. Split the layout tree with 1:3 ratio (left=25%, right=75%)
        self.layout.split_leaf_with_ratio(
            log_win_id,
            crate::ed::window::SplitDir::Vertical,
            diff_win_id,
            Some(0.30), // ← tig ratio: 1 part left, 3 parts right
        );

        // 4. Link the log window back to the diff window
        self.windows
            .iter_mut()
            .find(|w| w.id == log_win_id)
            .map(|w| w.diff_sibling = Some(diff_win_id));

        // 5. Focus the diff pane
        if let Some(idx) = self.windows.iter().position(|w| w.id == diff_win_id) {
            self.active_window_idx = idx;
        }
    }

    /// Create a `BufferKind::GitDiff` buffer for a commit (or file-in-commit).
    fn create_git_log_diff_buffer(&mut self, commit: &str, file_path: Option<&str>) -> usize {
        let buf_id = self.next_buf_id;
        self.next_buf_id += 1;

        let repo_root = self.get_git_log_repo_root();
        let diff_text = match file_path {
            Some(path) => crate::git::log::load_file_diff_in_commit(&repo_root, commit, path)
                .unwrap_or_else(|| format!("No diff for {} @ {}\n", path, commit)),
            None => crate::git::log::load_commit_diff(&repo_root, commit)
                .unwrap_or_else(|| format!("No diff for commit {}\n", commit)),
        };

        let mut buf = Buffer::new(buf_id, None).unwrap_or_else(|_| Buffer {
            id: buf_id,
            rope: ropey::Rope::from_str("\n"),
            filename: None,
            modified: false,
            undo_stack: Vec::new(),
            redo_stack: Vec::new(),
            syntax: SyntaxState::new(),
            bookmarks: std::collections::HashSet::new(),
            git_diffs: std::collections::HashMap::new(),
            kind: BufferKind::GitDiff,
            diagnostics: Vec::new(),
            git_log_state: None,
            git_status_state: None,
            diff_alignment: None,
            ripgrep_results: Vec::new(),
            ripgrep_line_map: Vec::new(),
            search_pattern: None,
            named_bookmarks: std::collections::HashMap::new(),
            llm_lock_line: 0,
            tab_size: self.config.tab_size,
            wgrep_mode: false,
            wgrep_prefix_lens: Vec::new(),
            wgrep_original_texts: Vec::new(),
        });

        buf.kind = BufferKind::GitDiff;
        buf.rope = ropey::Rope::from_str(&diff_text);
        buf.filename = file_path.map(|p| p.to_string());
        buf.parse_syntax();

        // Store metadata on the diff buffer so we can detect
        // what it's currently showing (used by auto-refresh dedup).
        let mut diff_meta = GitLogState {
            repo_root: repo_root,
            entries: Vec::new(),
            line_actions: std::collections::HashMap::new(),
        };
        diff_meta.entries.push(crate::git::log::GitLogEntry {
            hash_full: commit.to_string(),
            hash_short: commit[..7.min(commit.len())].to_string(),
            author: String::new(),
            date: String::new(),
            subject: file_path.unwrap_or("").to_string(),
            files: Vec::new(),
        });
        buf.git_log_state = Some(diff_meta);

        self.buffers.push(buf);
        buf_id
    }

    /// Replace the content of the diff buffer shown in `win_id`.
    pub fn refresh_git_log_diff(&mut self, win_id: usize, commit: &str, file_path: Option<&str>) {
        let buf_id = self
            .windows
            .iter()
            .find(|w| w.id == win_id)
            .map(|w| w.buffer_id());

        let Some(bid) = buf_id else { return };

        let repo_root = self.get_git_log_repo_root();
        let new_text = match file_path {
            Some(path) => crate::git::log::load_file_diff_in_commit(&repo_root, commit, path)
                .unwrap_or_else(|| format!("No diff for {} @ {}\n", path, commit)),
            None => crate::git::log::load_commit_diff(&repo_root, commit)
                .unwrap_or_else(|| format!("No diff for commit {}\n", commit)),
        };

        if let Some(buf) = self.buf_mut_by_id(bid) {
            buf.rope = ropey::Rope::from_str(&new_text);
            buf.filename = file_path.map(|p| p.to_string());
            buf.parse_syntax();

            // Update metadata
            let mut diff_meta = GitLogState {
                repo_root: repo_root,
                entries: Vec::new(),
                line_actions: std::collections::HashMap::new(),
            };
            diff_meta.entries.push(crate::git::log::GitLogEntry {
                hash_full: commit.to_string(),
                hash_short: commit[..7.min(commit.len())].to_string(),
                author: String::new(),
                date: String::new(),
                subject: file_path.unwrap_or("").to_string(),
                files: Vec::new(),
            });
            buf.git_log_state = Some(diff_meta);
        }

        // Reset scroll for the new content
        if let Some(win) = self.windows.iter_mut().find(|w| w.id == win_id) {
            win.scroll_line = 0;
            win.scroll_col = 0;
            win.row = 0;
            win.col = 0;
        }
    }

    /// Close both the GitLog window and its paired GitDiff window,
    /// collapsing the vsplit back to a single pane with a normal buffer.
    pub fn close_git_log_session(&mut self) {
        let current_kind = self.buf().kind;
        let sibling_id = self.active_window().diff_sibling;

        // Which window is the diff pane (the one added via split)?
        let diff_win_id: Option<usize> =
            if matches!(current_kind, BufferKind::GitDiff | BufferKind::GitDiffHead) {
                Some(self.active_window().id)
            } else {
                sibling_id
            };

        // Clear sibling references
        for win in &mut self.windows {
            win.diff_sibling = None;
        }

        // Remove the diff window from the layout tree — its parent Split
        // collapses into the sibling (the log pane).
        if let Some(did) = diff_win_id {
            self.layout.remove_leaf(did);
            self.windows.retain(|w| w.id != did);
        }

        // Remove all GitLog / GitDiff buffers
        self.buffers.retain(|b| {
            !matches!(
                b.kind,
                BufferKind::GitLog | BufferKind::GitDiff | BufferKind::GitDiffHead
            )
        });

        // Clamp the remaining window(s) to valid buffers
        let valid_ids: Vec<usize> = self.buffers.iter().map(|b| b.id).collect();
        self.active_window_idx = 0;
        if let Some(win) = self.windows.first_mut() {
            win.clamp_buffer_id(&valid_ids);
        }
    }

    /// After cursor movement in the GitLog pane, auto-refresh the diff
    /// pane to show the commit (or file) under the cursor.
    pub fn update_diff_for_log_cursor(&mut self) {
        let sibling_id = self.active_window().diff_sibling;
        let row = self.active_window().row;

        let action = self
            .buf()
            .git_log_state
            .as_ref()
            .and_then(|s| s.action_for_line(row).cloned());

        let (commit, file_path) = match action {
            Some(GitLogLineAction::ShowDiff { commit }) => (commit, None),
            Some(GitLogLineAction::OpenFile { path, commit }) => (commit, Some(path)),
            None => return,
        };

        let Some(win_id) = sibling_id else { return };

        // Only refresh if the commit (and file) changed — avoids
        // spawning git on every j/k when the line hasn't changed.
        let needs_refresh = {
            let buf_id = self
                .windows
                .iter()
                .find(|w| w.id == win_id)
                .map(|w| w.buffer_id());

            let Some(bid) = buf_id else {
                return;
            };

            self.buf_by_id(bid)
                .and_then(|buf| buf.git_log_state.as_ref())
                .and_then(|state| state.entries.first())
                .map(|entry| {
                    let same_commit = entry.hash_full == commit;
                    let same_file = entry.subject == file_path.as_deref().unwrap_or("");
                    !(same_commit && same_file)
                })
                .unwrap_or(true)
        };

        if needs_refresh {
            self.refresh_git_log_diff(win_id, &commit, file_path.as_deref());
        }
    }

    /// Helper: get the repo root from whichever buffer holds the GitLogState.
    pub fn get_git_log_repo_root(&self) -> std::path::PathBuf {
        self.buffers
            .iter()
            .find(|b| b.kind == BufferKind::GitLog)
            .and_then(|b| b.git_log_state.as_ref())
            .map(|s| s.repo_root.clone())
            .unwrap_or_else(|| std::path::PathBuf::from("."))
    }

    /// Open git log with a specific repo root and limit.
    pub fn open_git_log_with_root(&mut self, repo_root: &std::path::Path, limit: Option<usize>) {
        let (saved_row, saved_col) = {
            let win = self.active_window();
            (win.row, win.col)
        };
        let diff_sibling = self.active_window().diff_sibling;

        if let Some((state, text)) = crate::git::log::GitLogState::load(repo_root, limit) {
            let line_count = {
                let buf = self.buf_mut();
                buf.rope = ropey::Rope::from_str(&text);
                buf.git_log_state = Some(state);
                buf.parse_syntax();
                buf.len_lines()
            };

            let win = self.active_window_mut();
            win.row = saved_row.min(line_count.saturating_sub(1));
            win.col = saved_col;
            win.diff_sibling = diff_sibling;
        }
    }
}
