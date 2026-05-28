//! Git key dispatch — thin match arms that delegate to business logic
//! in [`super::git_ops`] and [`super::git_hunk`].

use crate::ed::editor::PendingGitAction;
use crate::ed::MessageKind;
use crate::event::KeyEvent;
use crate::Editor;
use crossterm::event::KeyCode;

impl Editor {
    // ── GitLog ────────────────────────────────────────────────────────

    pub fn handle_git_log_key(&mut self, key: KeyEvent) -> bool {
        match key.code {
            KeyCode::Enter => {
                self.git_log_enter();
                true
            }
            KeyCode::Char('o') => {
                self.git_log_open_file_split();
                true
            }
            KeyCode::Char('q') => {
                self.close_buffer();
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
            KeyCode::Char('q') | KeyCode::Esc => {
                self.git_status_close();
                true
            }
            _ => false,
        }
    }

    // ── GitDiff ───────────────────────────────────────────────────────

    pub fn handle_git_diff_key(&mut self, key: KeyEvent) -> bool {
        match key.code {
            KeyCode::Char('q') => {
                self.close_buffer();
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
