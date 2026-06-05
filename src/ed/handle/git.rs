//! Git key dispatch — thin match arms that delegate to business logic
//! in git/tig.rs, git/commit.rs, and other git modules.

use crate::ed::editor::PendingGitAction;
use crate::ed::mode::MessageKind;
use crate::event::KeyEvent;
use crate::git::log::GitLogLineAction;
use crate::Editor;
use crossterm::event::KeyCode;

impl Editor {
    // ── GitLog (tig-style vsplit) ─────────────────────────────────────

    pub fn handle_git_log_key(&mut self, key: KeyEvent) -> bool {
        if key.kind != crossterm::event::KeyEventKind::Press {
            return false;
        }

        match key.code {
            // ── Quit: close both panes (tig behavior) ───────────────
            KeyCode::Char('q') => {
                if self.active_window().diff_sibling.is_some() {
                    self.close_git_log_session();
                } else {
                    self.close_buffer();
                }
                true
            }

            // ── Enter: tig-style vsplit diff ─────────────────────────
            //  On commit header → full commit diff
            //  On file line     → file-only diff in that commit
            KeyCode::Enter => {
                let row = self.active_window().row;
                let action = self
                    .buf()
                    .git_log_state
                    .as_ref()
                    .and_then(|s| s.action_for_line(row).cloned());

                match action {
                    Some(GitLogLineAction::ShowDiff { ref commit }) => {
                        self.open_git_log_diff_vsplit(commit, None);
                    }
                    Some(GitLogLineAction::OpenFile {
                        ref path,
                        ref commit,
                    }) => {
                        self.open_git_log_diff_vsplit(commit, Some(path));
                    }
                    None => {}
                }
                true
            }

            // ── 'e': open file in editor (original Enter behavior) ───
            //  Replaces the log buffer with the file at HEAD.
            KeyCode::Char('e') => {
                let row = self.active_window().row;
                let action = self
                    .buf()
                    .git_log_state
                    .as_ref()
                    .and_then(|s| s.action_for_line(row).cloned());

                if let Some(GitLogLineAction::OpenFile { ref path, .. }) = action {
                    let file_path = path.clone();
                    // Close the log session first (collapses vsplit if open)
                    if self.active_window().diff_sibling.is_some() {
                        self.close_git_log_session();
                    } else {
                        self.close_buffer();
                    }
                    // Open the file in a normal buffer at line 0
                    let p = std::path::PathBuf::from(&file_path);
                    self.open_file_at_line(&p, 0);
                }
                true
            }

            // ── Tab: switch focus between panes ──────────────────────
            KeyCode::Tab => {
                if let Some(sib_id) = self.active_window().diff_sibling {
                    if let Some(idx) = self.windows.iter().position(|w| w.id == sib_id) {
                        self.active_window_idx = idx;
                    }
                }
                true
            }

            // ── Movement: j/k auto-update the diff pane ──────────────
            KeyCode::Up | KeyCode::Char('k') => {
                {
                    let (win, buf) = self.active_window_and_buf_mut();
                    crate::ed::movement::move_up(win, buf);
                }
                if self.active_window().diff_sibling.is_some() {
                    self.update_diff_for_log_cursor();
                }
                true
            }
            KeyCode::Down | KeyCode::Char('j') => {
                {
                    let (win, buf) = self.active_window_and_buf_mut();
                    crate::ed::movement::move_down(win, buf);
                }
                if self.active_window().diff_sibling.is_some() {
                    self.update_diff_for_log_cursor();
                }
                true
            }
            KeyCode::PageUp => {
                {
                    let (win, buf) = self.active_window_and_buf_mut();
                    let jump = win.position.height.saturating_sub(2).max(1);
                    crate::ed::movement::page_up(win, buf, jump);
                }
                if self.active_window().diff_sibling.is_some() {
                    self.update_diff_for_log_cursor();
                }
                true
            }
            KeyCode::PageDown => {
                {
                    let (win, buf) = self.active_window_and_buf_mut();
                    let jump = win.position.height.saturating_sub(2).max(1);
                    crate::ed::movement::page_down(win, buf, jump);
                }
                if self.active_window().diff_sibling.is_some() {
                    self.update_diff_for_log_cursor();
                }
                true
            }

            // ── Space: load more commits ─────────────────────────────
            KeyCode::Char(' ') => {
                let repo_root = self.get_git_log_repo_root();
                let current_count = self
                    .buf()
                    .git_log_state
                    .as_ref()
                    .map(|s| s.entries.len())
                    .unwrap_or(10);
                let new_limit = (current_count * 2).max(20);
                self.open_git_log_with_root(&repo_root, Some(new_limit));
                true
            }

            // ── Refresh ──────────────────────────────────────────────
            KeyCode::Char('r') => {
                let repo_root = self.get_git_log_repo_root();
                let current_count = self
                    .buf()
                    .git_log_state
                    .as_ref()
                    .map(|s| s.entries.len())
                    .unwrap_or(10);
                self.open_git_log_with_root(&repo_root, Some(current_count));
                true
            }

            _ => false,
        }
    }

    // ── GitDiff (right pane of vsplit, or standalone) ─────────────────

    pub fn handle_git_diff_key(&mut self, key: KeyEvent) -> bool {
        if key.kind != crossterm::event::KeyEventKind::Press {
            return false;
        }

        match key.code {
            // ── Quit: close both panes if sibling exists ─────────────
            KeyCode::Char('q') => {
                if self.active_window().diff_sibling.is_some() {
                    self.close_git_log_session();
                } else {
                    self.close_buffer();
                }
                true
            }

            // ── Tab: switch focus back to the log pane ───────────────
            KeyCode::Tab => {
                if let Some(sib_id) = self.active_window().diff_sibling {
                    if let Some(idx) = self.windows.iter().position(|w| w.id == sib_id) {
                        self.active_window_idx = idx;
                    }
                }
                true
            }

            // ── Scrolling ────────────────────────────────────────────
            KeyCode::Up | KeyCode::Char('k') => {
                let (win, buf) = self.active_window_and_buf_mut();
                crate::ed::movement::move_up(win, buf);
                true
            }
            KeyCode::Down | KeyCode::Char('j') => {
                let (win, buf) = self.active_window_and_buf_mut();
                crate::ed::movement::move_down(win, buf);
                true
            }
            KeyCode::PageUp => {
                let (win, buf) = self.active_window_and_buf_mut();
                let jump = win.position.height.saturating_sub(2).max(1);
                crate::ed::movement::page_up(win, buf, jump);
                true
            }
            KeyCode::PageDown => {
                let (win, buf) = self.active_window_and_buf_mut();
                let jump = win.position.height.saturating_sub(2).max(1);
                crate::ed::movement::page_down(win, buf, jump);
                true
            }

            _ => false,
        }
    }

    // ── GitAction prompt (y/n) ────────────────────────────────────────

    pub fn handle_git_action_prompt_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Char('y') | KeyCode::Char('Y') => {
                let action =
                    std::mem::replace(&mut self.pending_git_action, PendingGitAction::None);
                self.clear_status_msg();

                let repo_root = match self.resolve_repo_root() {
                    Some(r) => r,
                    None => {
                        self.set_status_msg("Not in a git repository", MessageKind::Error);
                        return;
                    }
                };

                match action {
                    PendingGitAction::SwitchBranch(branch) => {
                        self.execute_branch_switch(branch, repo_root);
                    }
                    PendingGitAction::PopStash(stash_ref) => {
                        self.execute_stash_pop(stash_ref, repo_root);
                    }
                    PendingGitAction::None => {}
                }
            }
            KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc => {
                self.pending_git_action = PendingGitAction::None;
                self.set_status_msg("Action cancelled", MessageKind::Info);
            }
            _ => {}
        }
    }

    // ── GitStatus ─────────────────────────────────────────────────────

    pub fn handle_git_status_key(&mut self, key: KeyEvent) -> bool {
        match key.code {
            KeyCode::Char('s') => {
                self.git_status_toggle_staged();
                true
            }
            KeyCode::Char('c') => {
                self.git_commit_stage_all_and_generate();
                true
            }
            KeyCode::Enter => {
                self.git_status_enter();
                true
            }
            KeyCode::Char('z') => {
                self.enter_command();
                self.command = "stash ".to_string();
                self.set_status_msg("Type stash comment and press Enter", MessageKind::Info);
                true
            }
            KeyCode::Char('q') => {
                self.git_status_close();
                true
            }
            _ => false,
        }
    }

    // ── GitCommit ─────────────────────────────────────────────────────

    pub fn handle_git_commit_key(&mut self, key: KeyEvent) -> bool {
        match key.code {
            KeyCode::Char('w') => {
                self.handle_commit_write();
                true
            }
            KeyCode::Char('q') | KeyCode::Esc => {
                self.git_commit_close();
                true
            }
            _ => false,
        }
    }

    // ── GitHunk popup ─────────────────────────────────────────────────

    pub fn handle_git_hunk_popup_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Esc => {
                self.popup.git_hunk = None;
            }
            KeyCode::Char('j') | KeyCode::Down => {
                if let Some(ref mut p) = self.popup.git_hunk {
                    p.move_down();
                }
            }
            KeyCode::Char('k') | KeyCode::Up => {
                if let Some(ref mut p) = self.popup.git_hunk {
                    p.move_up();
                }
            }
            KeyCode::Char('+') => self.yank_hunk_added_lines(),
            KeyCode::Char('-') => self.yank_hunk_deleted_lines(),
            _ => {}
        }
    }

    // ═══════════════════════════════════════════════════════════════════
    // Shared helpers — used by git_ops and git_hunk
    // ═══════════════════════════════════════════════════════════════════

    /// Resolve the git repo root from the active file, falling back to
    /// the current working directory.  Canonicalizes the result.
    pub fn resolve_repo_root(&self) -> Option<std::path::PathBuf> {
        self.active_filename()
            .and_then(|f| crate::git::gutter::find_git_root(std::path::Path::new(f)))
            .or_else(|| crate::git::gutter::find_git_root(std::path::Path::new(".")))
            .map(|r| std::fs::canonicalize(&r).unwrap_or(r))
    }

    /// Reset the active window's viewport to the origin (0, 0).
    pub(super) fn reset_window_viewport(&mut self) {
        let win = self.active_window_mut();
        win.row = 0;
        win.col = 0;
        win.scroll_line = 0;
        win.scroll_col = 0;
        win.desired_col = 0;
    }

    /// Switch the active window to `bid` and reset the viewport.
    pub(super) fn set_window_to_buffer(&mut self, bid: usize) {
        self.active_window_mut().set_buffer_id(bid);
        self.reset_window_viewport();
    }

    /// Extract the (action, repo_root) pair at the cursor row from a
    /// `*git-status*` buffer.
    pub(super) fn git_status_action_at_cursor(
        &self,
    ) -> Option<(crate::git::status::GitStatusLineAction, std::path::PathBuf)> {
        let win = self.active_window();
        let buf = self.buf();
        let state = buf.git_status_state.as_ref()?;
        let action = state.action_for_line(win.row)?;
        Some((action.clone(), state.repo_root.clone()))
    }

    /// Extract the (action, repo_root) pair at the cursor row from a
    /// `*git-log*` buffer.
    pub(super) fn git_log_action_at_cursor(
        &self,
    ) -> Option<(crate::git::log::GitLogLineAction, std::path::PathBuf)> {
        let win = self.active_window();
        let buf = self.buf();
        let state = buf.git_log_state.as_ref()?;
        let action = state.action_for_line(win.row)?;
        Some((action.clone(), state.repo_root.clone()))
    }

    /// Yank only the `+` lines from the git-hunk popup.
    fn yank_hunk_added_lines(&mut self) {
        if let Some(ref p) = self.popup.git_hunk {
            let added: Vec<&str> = p
                .lines
                .iter()
                .filter(|l| l.starts_with('+'))
                .map(|l| &l[1..])
                .collect();
            if !added.is_empty() {
                self.clipboard = Some(format!("{}\n", added.join("\n")));
                self.set_status_msg(
                    &format!("Yanked {} added/modified line(s)", added.len()),
                    MessageKind::Info,
                );
            } else {
                self.set_status_msg("No added/modified lines to yank", MessageKind::Info);
            }
        }
        self.popup.git_hunk = None;
    }

    /// Yank only the `-` lines from the git-hunk popup.
    fn yank_hunk_deleted_lines(&mut self) {
        if let Some(ref p) = self.popup.git_hunk {
            let deleted: Vec<&str> = p
                .lines
                .iter()
                .filter(|l| l.starts_with('-'))
                .map(|l| &l[1..])
                .collect();
            if !deleted.is_empty() {
                self.clipboard = Some(format!("{}\n", deleted.join("\n")));
                self.set_status_msg(
                    &format!("Yanked {} deleted line(s)", deleted.len()),
                    MessageKind::Info,
                );
            } else {
                self.set_status_msg("No deleted lines to yank", MessageKind::Info);
            }
        }
        self.popup.git_hunk = None;
    }
}
