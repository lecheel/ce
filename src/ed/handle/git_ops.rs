//! Git buffer lifecycle (open / refresh / close) and action execution.

use crate::ed::buffer::BufferKind;
use crate::ed::Buffer;
use crate::ed::MessageKind;
use crate::Editor;

impl Editor {
    // ═══════════════════════════════════════════════════════════════════
    // Open / refresh git buffers
    // ═══════════════════════════════════════════════════════════════════

    pub fn open_git_status(&mut self) {
        self.prev_mode = self.mode;

        let repo_root = match self.resolve_repo_root() {
            Some(r) => r,
            None => {
                self.set_status_msg("Not in a git repository", MessageKind::Error);
                return;
            }
        };

        let target_filename = "*git-status*";

        if let Some(id) = self
            .buffers
            .iter()
            .find(|b| b.filename.as_deref() == Some(target_filename))
            .map(|b| b.id)
        {
            self.set_window_to_buffer(id);
            self.set_status_msg(
                "Git Status — c: commit  Enter: open file  q: close",
                MessageKind::Info,
            );
            return;
        }

        let (state, text) = match crate::git::status::GitStatusState::load(&repo_root) {
            Some(r) => r,
            None => {
                self.set_status_msg("Failed to load git status", MessageKind::Error);
                return;
            }
        };

        let bid = self.create_git_buffer(
            target_filename.to_string(),
            &text,
            BufferKind::GitStatus,
            Some(state),
            None,
        );
        self.set_window_to_buffer(bid);
        self.enter_normal();
        self.set_status_msg(
            "Git Status — c: commit  Enter: open file  q: close",
            MessageKind::Info,
        );
    }

    pub fn open_git_log(&mut self, limit: Option<usize>) {
        let repo_root = match self.resolve_repo_root() {
            Some(r) => r,
            None => {
                self.set_status_msg("Not in a git repository", MessageKind::Error);
                return;
            }
        };

        let target_filename = "*git-log*";

        if let Some(id) = self
            .buffers
            .iter()
            .find(|b| b.filename.as_deref() == Some(target_filename))
            .map(|b| b.id)
        {
            self.set_window_to_buffer(id);
            self.set_status_msg(
                "Git Log — Enter: open/diff  o: split  j/k: scroll  q: close",
                MessageKind::Info,
            );
            return;
        }

        let actual_limit = match limit {
            Some(0) => None,
            Some(n) => Some(n),
            None => Some(10),
        };

        let (state, text) = match crate::git::log::GitLogState::load(&repo_root, actual_limit) {
            Some(r) => r,
            None => {
                self.set_status_msg("Failed to load git log (empty repo?)", MessageKind::Error);
                return;
            }
        };

        let bid = self.create_git_buffer(
            target_filename.to_string(),
            &text,
            BufferKind::GitLog,
            None,
            Some(state),
        );
        self.set_window_to_buffer(bid);
        self.enter_normal();
        self.set_status_msg(
            "Git Log — Enter: open/diff  o: split  j/k: scroll  q: close",
            MessageKind::Info,
        );
    }

    pub fn open_git_diff(&mut self, repo_root: &std::path::Path, commit: &str) {
        let diff_text = match crate::git::log::load_commit_diff(repo_root, commit) {
            Some(t) => t,
            None => {
                self.set_status_msg("Failed to load diff", MessageKind::Error);
                return;
            }
        };

        let short_hash = &commit[..7.min(commit.len())];
        let target_filename = format!("git://diff/{}", commit);

        if let Some(id) = self
            .buffers
            .iter()
            .find(|b| b.filename.as_deref() == Some(&target_filename))
            .map(|b| b.id)
        {
            self.set_window_to_buffer(id);
            self.set_status_msg(
                &format!("Diff {} — j/k: scroll  q: close", short_hash),
                MessageKind::Info,
            );
            return;
        }

        let bid =
            self.create_git_buffer(target_filename, &diff_text, BufferKind::GitDiff, None, None);
        self.set_window_to_buffer(bid);
        self.enter_normal();
        self.set_status_msg(
            &format!("Diff {} — j/k: scroll  q: close", short_hash),
            MessageKind::Info,
        );
    }

    pub fn try_process_special_command(&mut self, cmd: &str) -> bool {
        let parts: Vec<&str> = cmd.trim().split_whitespace().collect();
        match parts.first() {
            Some(&"gitlog" | &"tig") => {
                let limit = parts.get(1).and_then(|s| s.parse::<usize>().ok());
                self.open_git_log(limit);
                true
            }
            _ => false,
        }
    }

    // ═══════════════════════════════════════════════════════════════════
    // GitStatus actions
    // ═══════════════════════════════════════════════════════════════════

    pub(super) fn git_status_enter(&mut self) {
        let (action, repo_root) = match self.git_status_action_at_cursor() {
            Some(pair) => pair,
            None => return,
        };

        match action {
            crate::git::status::GitStatusLineAction::OpenFile { path } => {
                let full_path = repo_root.join(&path);
                if !full_path.exists() {
                    self.set_status_msg(&format!("File not found: {}", path), MessageKind::Error);
                    return;
                }
                self.open_buffer(Some(full_path.to_string_lossy().to_string()));
            }
            crate::git::status::GitStatusLineAction::SwitchBranch { branch } => {
                let is_clean = self
                    .buf()
                    .git_status_state
                    .as_ref()
                    .map(|s| {
                        !s.files
                            .iter()
                            .any(|f| f.status != crate::git::status::FileStatusType::Untracked)
                    })
                    .unwrap_or(false);

                if !is_clean {
                    self.set_status_msg(
                        "Cannot switch branch: clean changes first (commit or stash)",
                        MessageKind::Error,
                    );
                    return;
                }
                self.pending_git_action =
                    crate::ed::editor::PendingGitAction::SwitchBranch(branch.clone());
                self.set_status_msg(
                    &format!("Switch to branch '{}'? (y/n)", branch),
                    MessageKind::Info,
                );
            }
            crate::git::status::GitStatusLineAction::PopStash { stash_ref } => {
                self.pending_git_action =
                    crate::ed::editor::PendingGitAction::PopStash(stash_ref.clone());
                self.set_status_msg(
                    &format!("Are you sure you want to pop '{}'? (y/n)", stash_ref),
                    MessageKind::Info,
                );
            }
            crate::git::status::GitStatusLineAction::None => {}
        }
    }

    pub(super) fn git_status_toggle_staged(&mut self) {
        let (action, repo_root) = match self.git_status_action_at_cursor() {
            Some(pair) => pair,
            None => {
                self.set_status_msg("No file on this line", MessageKind::Info);
                return;
            }
        };

        let path = match action {
            crate::git::status::GitStatusLineAction::OpenFile { path } => path,
            _ => return,
        };

        let is_staged = self
            .buf()
            .git_status_state
            .as_ref()
            .and_then(|s| s.files.iter().find(|f| f.path == path))
            .map(|f| f.staged)
            .unwrap_or(false);

        let result = if is_staged {
            std::process::Command::new("git")
                .args(["reset", "HEAD", "--", &path])
                .current_dir(&repo_root)
                .output()
        } else {
            std::process::Command::new("git")
                .args(["add", "--", &path])
                .current_dir(&repo_root)
                .output()
        };

        match result {
            Ok(o) if o.status.success() => {
                self.refresh_git_status_buffer(&repo_root);
                let verb = if is_staged { "Unstaged" } else { "Staged" };
                self.set_status_msg(&format!("{} {}", verb, path), MessageKind::Success);
            }
            Ok(o) => {
                let err = String::from_utf8_lossy(&o.stderr);
                self.set_status_msg(
                    &format!("Failed to toggle stage: {}", err.trim()),
                    MessageKind::Error,
                );
            }
            Err(e) => {
                self.set_status_msg(&format!("Failed to run git: {}", e), MessageKind::Error);
            }
        }
    }

    pub(super) fn git_status_close(&mut self) {
        let target = "*git-status*";
        if let Some(id) = self
            .buffers
            .iter()
            .find(|b| b.filename.as_deref() == Some(target))
            .map(|b| b.id)
        {
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
            self.set_mode(self.prev_mode);
            self.set_status_msg("Git status closed", MessageKind::Info);
        }
    }

    pub fn refresh_git_status_buffer(&mut self, repo_root: &std::path::Path) {
        let target = "*git-status*";
        let buf_id = self
            .buffers
            .iter()
            .find(|b| b.filename.as_deref() == Some(target))
            .map(|b| b.id);

        if let Some(buf_id) = buf_id {
            if let Some((new_state, text)) = crate::git::status::GitStatusState::load(repo_root) {
                if let Some(buf) = self.buf_mut_by_id(buf_id) {
                    buf.rope = ropey::Rope::from_str(&text);
                    buf.git_status_state = Some(new_state);
                }
                for win in &mut self.windows {
                    if win.buffer_id() == buf_id {
                        win.row = win.row.min(text.lines().count().saturating_sub(1));
                    }
                }
            }
        }
    }

    // ═══════════════════════════════════════════════════════════════════
    // GitLog actions
    // ═══════════════════════════════════════════════════════════════════

    pub(super) fn git_log_enter(&mut self) {
        let (action, repo_root) = match self.git_log_action_at_cursor() {
            Some(pair) => pair,
            None => return,
        };

        match action {
            crate::git::log::GitLogLineAction::OpenFile { path, .. } => {
                let full_path = repo_root.join(&path);
                if !full_path.exists() {
                    self.set_status_msg(&format!("File not found: {}", path), MessageKind::Error);
                    return;
                }
                self.open_buffer(Some(full_path.to_string_lossy().to_string()));
            }
            crate::git::log::GitLogLineAction::ShowDiff { commit } => {
                self.open_git_diff(&repo_root, &commit);
            }
        }
    }

    pub(super) fn git_log_open_file_split(&mut self) {
        let (action, repo_root) = match self.git_log_action_at_cursor() {
            Some(pair) => pair,
            None => return,
        };

        match action {
            crate::git::log::GitLogLineAction::OpenFile { path, .. } => {
                let full_path = repo_root.join(&path);
                if !full_path.exists() {
                    self.set_status_msg(&format!("File not found: {}", path), MessageKind::Error);
                    return;
                }
                self.split_horizontal();
                self.open_buffer(Some(full_path.to_string_lossy().to_string()));
            }
            _ => {
                self.set_status_msg("No file on this line", MessageKind::Info);
            }
        }
    }

    // ═══════════════════════════════════════════════════════════════════
    // Execution helpers
    // ═══════════════════════════════════════════════════════════════════

    pub(super) fn execute_branch_switch(&mut self, branch: String, repo_root: std::path::PathBuf) {
        let output = std::process::Command::new("git")
            .args(["checkout", &branch])
            .current_dir(&repo_root)
            .output();

        match output {
            Ok(o) if o.status.success() => {
                self.set_status_msg(
                    &format!("Switched to branch {}", branch),
                    MessageKind::Success,
                );

                // Reload all open Normal buffers from disk
                let mut reloaded = Vec::new();
                for buf in &mut self.buffers {
                    if buf.kind == BufferKind::Normal {
                        // Clone filename BEFORE mutating buf to avoid
                        // simultaneous immutable + mutable borrows
                        let filename_owned = buf.filename.clone();
                        if let Some(ref filename) = filename_owned {
                            let path = std::path::Path::new(filename);
                            if path.exists() && path.is_file() {
                                if let Ok(content) = std::fs::read_to_string(path) {
                                    buf.rope = ropey::Rope::from_str(&content);
                                    buf.modified = false;
                                    buf.parse_syntax();
                                    reloaded.push((buf.id, filename.clone()));
                                }
                            }
                        }
                    }
                }

                for (bid, filename) in reloaded {
                    if let Some(buf) = self.buf_mut_by_id(bid) {
                        let rope = buf.rope.clone();
                        self.async_gutter.request_diff(bid, &rope, Some(&filename));
                    }
                }

                let buffers = &self.buffers;
                for win in &mut self.windows {
                    if let Some(buf) = buffers.iter().find(|b| b.id == win.buffer_id()) {
                        win.row = win.row.min(buf.len_lines().saturating_sub(1));
                        win.col = win.col.min(buf.line_char_len(win.row));
                    }
                }

                self.refresh_git_status_buffer(&repo_root);
            }
            Ok(o) => {
                let err = String::from_utf8_lossy(&o.stderr);
                self.set_status_msg(
                    &format!("Switch failed: {}", err.trim()),
                    MessageKind::Error,
                );
            }
            Err(e) => {
                self.set_status_msg(&format!("Failed to run git: {}", e), MessageKind::Error);
            }
        }
    }

    pub(super) fn execute_stash_pop(&mut self, stash_ref: String, repo_root: std::path::PathBuf) {
        let output = std::process::Command::new("git")
            .args(["stash", "pop", &stash_ref])
            .current_dir(&repo_root)
            .output();

        match output {
            Ok(o) if o.status.success() => {
                self.set_status_msg(&format!("Popped {}", stash_ref), MessageKind::Success);
                self.refresh_git_status_buffer(&repo_root);
                self.refresh_active_gutter();
            }
            Ok(o) => {
                let err = String::from_utf8_lossy(&o.stderr);
                self.set_status_msg(&format!("Pop failed: {}", err.trim()), MessageKind::Error);
            }
            Err(e) => {
                self.set_status_msg(&format!("Failed to run git: {}", e), MessageKind::Error);
            }
        }
    }

    pub fn handle_stash_command(&mut self, comment: &str) {
        let repo_root = match self.resolve_repo_root() {
            Some(r) => r,
            None => {
                self.set_status_msg("Not in a git repository", MessageKind::Error);
                return;
            }
        };

        let comment = comment.trim();
        let args: Vec<&str> = if comment.is_empty() {
            vec!["stash", "push"]
        } else {
            vec!["stash", "push", "-m", comment]
        };

        let output = std::process::Command::new("git")
            .args(&args)
            .current_dir(&repo_root)
            .output();

        match output {
            Ok(o) if o.status.success() => {
                self.set_status_msg("Changes stashed successfully", MessageKind::Success);
                self.refresh_git_status_buffer(&repo_root);
                self.refresh_active_gutter();
            }
            Ok(o) => {
                let err = String::from_utf8_lossy(&o.stderr);
                self.set_status_msg(&format!("Stash failed: {}", err.trim()), MessageKind::Error);
            }
            Err(e) => {
                self.set_status_msg(&format!("Failed to run git: {}", e), MessageKind::Error);
            }
        }
    }

    // ═══════════════════════════════════════════════════════════════════
    // Background git tasks
    // ═══════════════════════════════════════════════════════════════════

    pub fn run_git_tasks(&mut self) {
        for buffer_id in self.git_debounce.poll_and_dispatch() {
            if let Some(buf) = self.buf_by_id(buffer_id) {
                if buf.filename.is_some() {
                    let rope = buf.rope.clone();
                    let filename = buf.filename.clone();
                    self.async_gutter
                        .request_diff(buffer_id, &rope, filename.as_deref());
                }
            }
        }

        for response in self.async_gutter.poll_results() {
            if let Some(buf) = self.buf_mut_by_id(response.buffer_id) {
                buf.git_diffs = response.diffs;
            }
        }
    }

    // ═══════════════════════════════════════════════════════════════════
    // Private helpers
    // ═══════════════════════════════════════════════════════════════════

    fn create_git_buffer(
        &mut self,
        filename: String,
        text: &str,
        kind: BufferKind,
        git_status_state: Option<crate::git::status::GitStatusState>,
        git_log_state: Option<crate::git::log::GitLogState>,
    ) -> usize {
        let id = self.next_buf_id;
        self.next_buf_id += 1;

        let buf = Buffer {
            id,
            rope: ropey::Rope::from_str(text),
            filename: Some(filename),
            modified: false,
            undo_stack: Vec::new(),
            redo_stack: Vec::new(),
            syntax: crate::ed::syntax::SyntaxState::new(),
            bookmarks: std::collections::HashSet::new(),
            git_diffs: std::collections::HashMap::new(),
            named_bookmarks: std::collections::HashMap::new(),
            kind,
            git_log_state,
            git_status_state,
            ripgrep_results: Vec::new(),
            ripgrep_line_map: Vec::new(),
            search_pattern: None,
            diff_alignment: None,
        };

        let bid = buf.id;
        self.buffers.push(buf);
        self.buffers.last_mut().unwrap().parse_syntax();
        bid
    }

    fn refresh_active_gutter(&mut self) {
        let bid = self.active_window().buffer_id();
        if let Some(buf) = self.buf_mut_by_id(bid) {
            let rope = buf.rope.clone();
            let filename = buf.filename.clone();
            self.async_gutter
                .request_diff(bid, &rope, filename.as_deref());
        }
    }
}
