use crate::ed::buffer::BufferKind;
use crate::ed::editor::PendingGitAction;
use crate::ed::editor::QuitPrompt;
use crate::ed::misc_helper::get_head_file_content;
use crate::ed::misc_helper::group_signed_rows;
use crate::ed::Buffer;
use crate::ed::MessageKind;
use crate::event::KeyEvent;
use crate::event::KeyModifiers;
use crate::popup::PopupContent;
use crate::Editor;
use crossterm::event::KeyCode;

impl Editor {
    // ═══════════════════════════════════════════════════════════════════
    // GitLog key handler
    // ═══════════════════════════════════════════════════════════════════

    pub fn handle_git_log_key(&mut self, key: KeyEvent) -> bool {
        match key.code {
            // ── Git-log specific triggers ────────────────────────────
            KeyCode::Enter => {
                self.git_log_enter();
                true
            }
            KeyCode::Char('o') => {
                // Open file in a split
                self.git_log_open_file_split();
                true
            }

            // ── Close ────────────────────────────────────────────────
            KeyCode::Char('q') => {
                self.close_buffer();
                true
            }

            _ => false, // Fall through to inherit normal navigation
        }
    }

    /// Handles y/n confirmations for pending git prompts.
    pub fn handle_git_action_prompt_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Char('y') | KeyCode::Char('Y') => {
                // Take the action out of state
                let action =
                    std::mem::replace(&mut self.pending_git_action, PendingGitAction::None);
                self.clear_status_msg();

                // Get the repo root safely
                let repo_root = {
                    if let Some(fname) = self.active_filename() {
                        let path = std::path::Path::new(fname);
                        crate::git::gutter::find_git_root(path)
                    } else {
                        None
                    }
                    .or_else(|| crate::git::gutter::find_git_root(std::path::Path::new(".")))
                };

                let repo_root = match repo_root {
                    Some(r) => r,
                    None => {
                        self.set_status_msg("Not in a git repository", MessageKind::Error);
                        return;
                    }
                };

                // Execute the confirmed action
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

    /// Isolated branch checkout execution
    fn execute_branch_switch(&mut self, branch: String, repo_root: std::path::PathBuf) {
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

                // Reload all open file buffers from disk
                let mut reloaded_bids = Vec::new();
                for buf in &mut self.buffers {
                    if buf.kind == crate::ed::buffer::BufferKind::Normal {
                        if let Some(filename) = buf.filename.clone() {
                            let path = std::path::Path::new(&filename);
                            if path.exists() && path.is_file() {
                                if let Ok(content) = std::fs::read_to_string(path) {
                                    buf.rope = ropey::Rope::from_str(&content);
                                    buf.modified = false;
                                    buf.parse_syntax();
                                    reloaded_bids.push((buf.id, filename));
                                }
                            }
                        }
                    }
                }

                // Refresh git gutters
                for (bid, filename) in reloaded_bids {
                    if let Some(buf) = self.buf_mut_by_id(bid) {
                        let rope = buf.rope.clone();
                        self.async_gutter.request_diff(bid, &rope, Some(&filename));
                    }
                }

                // Clamp cursors
                let buffers = &self.buffers;
                for win in &mut self.windows {
                    if let Some(buf) = buffers.iter().find(|b| b.id == win.buffer_id()) {
                        let max_row = buf.len_lines().saturating_sub(1);
                        win.row = win.row.min(max_row);
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

    /// Isolated stash pop execution
    fn execute_stash_pop(&mut self, stash_ref: String, repo_root: std::path::PathBuf) {
        let output = std::process::Command::new("git")
            .args(["stash", "pop", &stash_ref])
            .current_dir(&repo_root)
            .output();

        match output {
            Ok(o) if o.status.success() => {
                self.set_status_msg(&format!("Popped {}", stash_ref), MessageKind::Success);
                self.refresh_git_status_buffer(&repo_root);

                let active_bid = self.active_window().buffer_id();
                if let Some(buf) = self.buf_mut_by_id(active_bid) {
                    let rope = buf.rope.clone();
                    let filename = buf.filename.clone();
                    self.async_gutter
                        .request_diff(active_bid, &rope, filename.as_deref());
                }
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

    // ═══════════════════════════════════════════════════════════════════
    // GitStatus key handler
    // ═══════════════════════════════════════════════════════════════════
    pub fn handle_git_status_key(&mut self, key: KeyEvent) -> bool {
        match key.code {
            // ── Toggle staged ─────────────────────────────────
            KeyCode::Char('s') => {
                self.git_status_toggle_staged();
                true
            }
            // ── Commit with LLM ────────────────────────────────
            KeyCode::Char('c') => {
                self.git_commit_stage_all_and_generate();
                true
            }
            // ── Open file OR Switch Branch ─────────────────────
            KeyCode::Enter => {
                self.git_status_enter();
                true
            }
            // ── Trigger quick stash with a comment ─────────────
            KeyCode::Char('z') => {
                self.enter_command();
                self.command = "stash ".to_string(); // Prefills the prompt
                self.set_status_msg("Type stash comment and press Enter", MessageKind::Info);
                true
            }
            // ── Close ──────────────────────────────────────────
            KeyCode::Char('q') | KeyCode::Esc => {
                self.git_status_close();
                true
            }
            _ => false,
        }
    }

    fn git_status_enter(&mut self) {
        let (action, repo_root) = {
            let win = self.active_window();
            let buf = self.buf();
            let row = win.row;
            let state = match &buf.git_status_state {
                Some(s) => s,
                None => return,
            };
            let action = match state.action_for_line(row) {
                Some(a) => a.clone(),
                None => return,
            };
            (action, state.repo_root.clone())
        };

        match action {
            crate::git::status::GitStatusLineAction::OpenFile { path } => {
                let full_path = repo_root.join(&path);
                if !full_path.exists() {
                    self.set_status_msg(&format!("File not found: {}", path), MessageKind::Error);
                    return;
                }
                let path_str = full_path.to_string_lossy().to_string();
                self.open_buffer(Some(path_str));
            }

            crate::git::status::GitStatusLineAction::SwitchBranch { branch } => {
                // (Optional Confirmation check)
                // You can bypass confirmation for branch switches if you prefer,
                // but if you want confirmation, assign it to the pending state:
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

                // Ask for confirmation instead of running immediately
                self.pending_git_action = PendingGitAction::SwitchBranch(branch.clone());
                self.set_status_msg(
                    &format!("Switch to branch '{}'? (y/n)", branch),
                    MessageKind::Info,
                );
            }

            crate::git::status::GitStatusLineAction::PopStash { stash_ref } => {
                // Put the stash into pending confirmation (highly recommended)
                self.pending_git_action = PendingGitAction::PopStash(stash_ref.clone());
                self.set_status_msg(
                    &format!("Are you sure you want to pop '{}'? (y/n)", stash_ref),
                    MessageKind::Info,
                );
            }

            crate::git::status::GitStatusLineAction::None => {}
        }
    }

    fn git_status_close(&mut self) {
        let target_filename = "*git-status*";
        if let Some(id) = self
            .buffers
            .iter()
            .find(|b| b.filename.as_deref() == Some(target_filename))
            .map(|b| b.id)
        {
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

            // Restore the previous mode (such as Brief Mode)
            let prev = self.prev_mode;
            self.set_mode(prev);

            self.set_status_msg("Git status closed", MessageKind::Info);
        }
    }

    /// Toggle the staged state of the file under the cursor.
    fn git_status_toggle_staged(&mut self) {
        let (action, repo_root) = {
            let win = self.active_window();
            let buf = self.buf();
            let row = win.row;
            let state = match &buf.git_status_state {
                Some(s) => s,
                None => return,
            };
            let action = match state.action_for_line(row) {
                Some(a) => a.clone(),
                None => {
                    self.set_status_msg("No file on this line", MessageKind::Info);
                    return;
                }
            };
            (action, state.repo_root.clone())
        };

        // Add the SwitchBranch pattern here to return early
        let path = match action {
            crate::git::status::GitStatusLineAction::OpenFile { path } => path,
            crate::git::status::GitStatusLineAction::SwitchBranch { .. } => return,
            crate::git::status::GitStatusLineAction::PopStash { .. } => return,
            crate::git::status::GitStatusLineAction::None => return,
        };

        // Determine if currently staged by searching the state files list
        let is_staged = self
            .buf()
            .git_status_state
            .as_ref()
            .and_then(|s| s.files.iter().find(|f| f.path == path))
            .map(|f| f.staged)
            .unwrap_or(false);

        let result = if is_staged {
            // Unstage
            std::process::Command::new("git")
                .args(["reset", "HEAD", "--", &path])
                .current_dir(&repo_root)
                .output()
        } else {
            // Stage
            std::process::Command::new("git")
                .args(["add", "--", &path])
                .current_dir(&repo_root)
                .output()
        };

        match result {
            Ok(output) if output.status.success() => {
                // Refresh buffer in place without closing
                self.refresh_git_status_buffer(&repo_root);
                let action_str = if is_staged { "Unstaged" } else { "Staged" };
                self.set_status_msg(&format!("{} {}", action_str, path), MessageKind::Success);
            }
            Ok(output) => {
                let err = String::from_utf8_lossy(&output.stderr);
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

    /// Safely refresh the `*git-status*` buffer if it exists in the editor,
    /// without touching or overwriting the currently active code buffer.
    fn refresh_git_status_buffer(&mut self, repo_root: &std::path::Path) {
        let target_filename = "*git-status*";

        // Find the git status buffer by its unique filename
        let status_buf_id = self
            .buffers
            .iter()
            .find(|b| b.filename.as_deref() == Some(target_filename))
            .map(|b| b.id);

        if let Some(buf_id) = status_buf_id {
            if let Some((new_state, text)) = crate::git::status::GitStatusState::load(repo_root) {
                // Mutate the status buffer in place by ID
                if let Some(buf) = self.buf_mut_by_id(buf_id) {
                    buf.rope = ropey::Rope::from_str(&text);
                    buf.git_status_state = Some(new_state);
                }

                // Clamp any window viewports currently showing this status buffer
                for win in &mut self.windows {
                    if win.buffer_id() == buf_id {
                        let max_row = text.lines().count().saturating_sub(1);
                        win.row = win.row.min(max_row);
                    }
                }
            }
        }
    }

    /// Open a git status viewer for the repository of the active file.
    pub fn open_git_status(&mut self) {
        // Save the active mode (e.g., Brief Mode) before switching to Normal
        self.prev_mode = self.mode;
        // Determine repo root from the active file or cwd
        let repo_root = {
            if let Some(fname) = self.active_filename() {
                let path = std::path::Path::new(fname);
                crate::git::gutter::find_git_root(path)
            } else {
                None
            }
            .or_else(|| crate::git::gutter::find_git_root(std::path::Path::new(".")))
        };

        let repo_root = match repo_root {
            Some(r) => std::fs::canonicalize(&r).unwrap_or(r),
            None => {
                self.set_status_msg("Not in a git repository", MessageKind::Error);
                return;
            }
        };

        let target_filename = "*git-status*".to_string();

        let existing_id = self
            .buffers
            .iter()
            .find(|buf| buf.filename.as_deref() == Some(&target_filename))
            .map(|buf| buf.id);

        if let Some(id) = existing_id {
            let win = self.active_window_mut();
            win.set_buffer_id(id);
            win.row = 0;
            win.col = 0;
            win.scroll_line = 0;
            win.scroll_col = 0;
            win.desired_col = 0;
            self.set_status_msg(
                "Git Status — c: commit  Enter: open file  q: close",
                MessageKind::Info,
            );
            return;
        }

        // Load fresh status
        let (state, text) = match crate::git::status::GitStatusState::load(&repo_root) {
            Some(result) => result,
            None => {
                self.set_status_msg("Failed to load git status", MessageKind::Error);
                return;
            }
        };

        let id = self.next_buf_id;
        self.next_buf_id += 1;

        let buf = Buffer {
            id,
            rope: ropey::Rope::from_str(&text),
            filename: Some(target_filename),
            modified: false,
            undo_stack: Vec::new(),
            syntax: crate::ed::syntax::SyntaxState::new(),
            bookmarks: std::collections::HashSet::new(),
            git_diffs: std::collections::HashMap::new(),
            named_bookmarks: std::collections::HashMap::new(),
            kind: BufferKind::GitStatus,
            git_log_state: None,
            git_status_state: Some(state),
            ripgrep_results: Vec::new(),
            ripgrep_line_map: Vec::new(),
            search_pattern: None,
            diff_alignment: None,
        };

        let bid = buf.id;
        self.buffers.push(buf);
        self.buffers.last_mut().unwrap().parse_syntax();
        self.active_window_mut().set_buffer_id(bid);
        self.active_window_mut().row = 0;
        self.active_window_mut().col = 0;
        self.active_window_mut().scroll_line = 0;
        self.active_window_mut().scroll_col = 0;
        self.active_window_mut().desired_col = 0;

        self.enter_normal();
        self.set_status_msg(
            "Git Status — c: commit  Enter: open file  q: close",
            MessageKind::Info,
        );
    }

    // ═══════════════════════════════════════════════════════════════════
    // GitDiff key handler
    // ═══════════════════════════════════════════════════════════════════
    /// Handles keys specifically when the GitCommit buffer is focused.
    pub fn handle_git_commit_key(&mut self, key: KeyEvent) -> bool {
        match key.code {
            // ── Confirm commit ────────────────────────────
            KeyCode::Char('w') => {
                self.handle_commit_write();
                true
            }
            // ── Cancel ────────────────────────────────────
            KeyCode::Char('q') | KeyCode::Esc => {
                self.git_commit_close();
                true
            }
            _ => false,
        }
    }

    /// Executes `git stash push` safely from any buffer in the editor,
    /// refreshing git status buffers and active gutter highlights automatically.
    pub fn handle_stash_command(&mut self, comment: &str) {
        // Resolve git root dynamically from active file or fallback to current directory
        let repo_root = {
            if let Some(fname) = self.active_filename() {
                let path = std::path::Path::new(fname);
                crate::git::gutter::find_git_root(path)
            } else {
                None
            }
            .or_else(|| crate::git::gutter::find_git_root(std::path::Path::new(".")))
        };

        let repo_root = match repo_root {
            Some(r) => std::fs::canonicalize(&r).unwrap_or(r),
            None => {
                self.set_status_msg("Not in a git repository", MessageKind::Error);
                return;
            }
        };

        // Build command arguments
        let comment = comment.trim();
        let args = if comment.is_empty() {
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

                // Safely update the status buffer if it is open in any window
                self.refresh_git_status_buffer(&repo_root);

                // Force clear git gutter indicators in the active buffer immediately
                let active_bid = self.active_window().buffer_id();
                if let Some(buf) = self.buf_mut_by_id(active_bid) {
                    let rope = buf.rope.clone();
                    let filename = buf.filename.clone();
                    self.async_gutter
                        .request_diff(active_bid, &rope, filename.as_deref());
                }
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

    pub fn handle_git_diff_key(&mut self, key: KeyEvent) -> bool {
        match key.code {
            KeyCode::Char('q') => {
                self.close_buffer();
                true
            }
            _ => false, // Fall through to inherit normal navigation
        }
    }

    // ═══════════════════════════════════════════════════════════════════
    // GitLog action handlers
    // ═══════════════════════════════════════════════════════════════════

    /// Enter key in git log buffer: open file or show diff depending
    /// on the current line.
    fn git_log_enter(&mut self) {
        let (action, repo_root) = {
            let win = self.active_window();
            let buf = self.buf();
            let row = win.row;
            let state = match &buf.git_log_state {
                Some(s) => s,
                None => return,
            };
            let action = match state.action_for_line(row) {
                Some(a) => a.clone(),
                None => return,
            };
            (action, state.repo_root.clone())
        };

        match action {
            crate::git::log::GitLogLineAction::OpenFile { path, .. } => {
                let full_path = repo_root.join(&path);
                if !full_path.exists() {
                    self.set_status_msg(&format!("File not found: {}", path), MessageKind::Error);
                    return;
                }
                let path_str = full_path.to_string_lossy().to_string();
                self.open_buffer(Some(path_str));
            }
            crate::git::log::GitLogLineAction::ShowDiff { commit } => {
                self.open_git_diff(&repo_root, &commit);
            }
        }
    }

    /// `o` key in git log buffer: open file in a horizontal split.
    fn git_log_open_file_split(&mut self) {
        let (action, repo_root) = {
            let win = self.active_window();
            let buf = self.buf();
            let row = win.row;
            let state = match &buf.git_log_state {
                Some(s) => s,
                None => return,
            };
            let action = match state.action_for_line(row) {
                Some(a) => a.clone(),
                None => return,
            };
            (action, state.repo_root.clone())
        };

        match action {
            crate::git::log::GitLogLineAction::OpenFile { path, .. } => {
                let full_path = repo_root.join(&path);
                if !full_path.exists() {
                    self.set_status_msg(&format!("File not found: {}", path), MessageKind::Error);
                    return;
                }
                let path_str = full_path.to_string_lossy().to_string();
                self.split_horizontal();
                self.open_buffer(Some(path_str));
            }
            _ => {
                self.set_status_msg("No file on this line", MessageKind::Info);
            }
        }
    }

    // ═══════════════════════════════════════════════════════════════════
    // Open git log / git diff buffers
    // ═══════════════════════════════════════════════════════════════════

    /// Open a tig-like git log viewer for the repository of the active
    /// file (or the current working directory).
    pub fn open_git_log(&mut self) {
        // Determine repo root from the active file or cwd
        let repo_root = {
            if let Some(fname) = self.active_filename() {
                let path = std::path::Path::new(fname);
                crate::git::gutter::find_git_root(path)
            } else {
                None
            }
            .or_else(|| crate::git::gutter::find_git_root(std::path::Path::new(".")))
        };

        let repo_root = match repo_root {
            Some(r) => std::fs::canonicalize(&r).unwrap_or(r),
            None => {
                self.set_status_msg("Not in a git repository", MessageKind::Error);
                return;
            }
        };

        // ── CHANGE: Set target_filename to "*git-log*" ─────────────────
        let target_filename = "*git-log*".to_string();

        let existing_id = self
            .buffers
            .iter()
            .find(|buf| buf.filename.as_deref() == Some(&target_filename))
            .map(|buf| buf.id);

        if let Some(id) = existing_id {
            let win = self.active_window_mut();
            win.set_buffer_id(id);
            win.row = 0;
            win.col = 0;
            win.scroll_line = 0;
            win.scroll_col = 0;
            win.desired_col = 0;
            self.set_status_msg(
                "Git Log — Enter: open/diff  o: split  j/k: scroll  q: close",
                MessageKind::Info,
            );
            return;
        }

        // Load fresh log
        let (state, text) = match crate::git::log::GitLogState::load(&repo_root) {
            Some(result) => result,
            None => {
                self.set_status_msg("Failed to load git log (empty repo?)", MessageKind::Error);
                return;
            }
        };

        let id = self.next_buf_id;
        self.next_buf_id += 1;

        let buf = Buffer {
            id,
            rope: ropey::Rope::from_str(&text),
            filename: Some(target_filename), // <-- Assigned to Some("*git-log*")
            modified: false,
            undo_stack: Vec::new(),
            syntax: crate::ed::syntax::SyntaxState::new(),
            bookmarks: std::collections::HashSet::new(),
            git_diffs: std::collections::HashMap::new(),
            named_bookmarks: std::collections::HashMap::new(),
            kind: BufferKind::GitLog,
            git_log_state: Some(state),
            git_status_state: None,
            ripgrep_results: Vec::new(),
            ripgrep_line_map: Vec::new(),
            search_pattern: None,
            diff_alignment: None,
        };

        let bid = buf.id;
        self.buffers.push(buf);
        self.buffers.last_mut().unwrap().parse_syntax();
        self.active_window_mut().set_buffer_id(bid);
        self.active_window_mut().row = 0;
        self.active_window_mut().col = 0;
        self.active_window_mut().scroll_line = 0;
        self.active_window_mut().scroll_col = 0;
        self.active_window_mut().desired_col = 0;

        self.enter_normal();
        self.set_status_msg(
            "Git Log — Enter: open/diff  o: split  j/k: scroll  q: close",
            MessageKind::Info,
        );
    }

    /// Open a read-only git-diff buffer showing the full patch for
    /// `commit`.
    pub fn open_git_diff(&mut self, repo_root: &std::path::Path, commit: &str) {
        let diff_text = match crate::git::log::load_commit_diff(repo_root, commit) {
            Some(t) => t,
            None => {
                self.set_status_msg("Failed to load diff", MessageKind::Error);
                return;
            }
        };

        let short_hash = &commit[..7.min(commit.len())];

        // Reuse existing diff buffer for this commit if one exists
        let target_filename = format!("git://diff/{}", commit);
        let existing_id = self
            .buffers
            .iter()
            .find(|buf| buf.filename.as_deref() == Some(&target_filename))
            .map(|buf| buf.id);

        if let Some(id) = existing_id {
            let win = self.active_window_mut();
            win.set_buffer_id(id);
            win.row = 0;
            win.col = 0;
            win.scroll_line = 0;
            win.scroll_col = 0;
            win.desired_col = 0;
            self.set_status_msg(
                &format!("Diff {} — j/k: scroll  q: close", short_hash),
                MessageKind::Info,
            );
            return;
        }

        let id = self.next_buf_id;
        self.next_buf_id += 1;

        let buf = Buffer {
            id,
            rope: ropey::Rope::from_str(&diff_text),
            filename: Some(target_filename),
            modified: false,
            undo_stack: Vec::new(),
            syntax: crate::ed::syntax::SyntaxState::new(),
            bookmarks: std::collections::HashSet::new(),
            git_diffs: std::collections::HashMap::new(),
            named_bookmarks: std::collections::HashMap::new(),
            kind: BufferKind::GitDiff,
            git_log_state: None,
            git_status_state: None,

            ripgrep_results: Vec::new(),
            ripgrep_line_map: Vec::new(),
            search_pattern: None,
            diff_alignment: None,
        };

        let bid = buf.id;
        self.buffers.push(buf);
        self.buffers.last_mut().unwrap().parse_syntax();
        self.active_window_mut().set_buffer_id(bid);
        self.active_window_mut().row = 0;
        self.active_window_mut().col = 0;
        self.active_window_mut().scroll_line = 0;
        self.active_window_mut().scroll_col = 0;
        self.active_window_mut().desired_col = 0;

        self.enter_normal();
        self.set_status_msg(
            &format!("Diff {} — j/k: scroll  q: close", short_hash),
            MessageKind::Info,
        );
    }

    /// Process `:gitlog` / `:tig` commands.  Returns `true` if the
    /// command was handled.
    ///
    /// Call this from the command-dispatch table:
    /// ```ignore
    /// if editor.try_process_special_command(&cmd) { return; }
    /// ```
    pub fn try_process_special_command(&mut self, cmd: &str) -> bool {
        match cmd.trim() {
            "gitlog" | "tig" => {
                self.open_git_log();
                true
            }
            _ => false,
        }
    }

    pub fn handle_popup_key(&mut self, key: KeyEvent) {
        // Special intercept for Scankey mode
        if matches!(self.popup.content, Some(PopupContent::Scankey { .. })) {
            match key.code {
                crossterm::event::KeyCode::Esc | crossterm::event::KeyCode::Char('q') => {
                    self.popup.close();
                    self.scankey_info = None;
                    self.clear_status_msg();
                }
                _ => {
                    let formatted = crate::keybind::binding_ex::format_key(key);

                    let implicit_shift = matches!(key.code, KeyCode::Char(c) if c.is_ascii_uppercase())
                        && !key.modifiers.contains(KeyModifiers::SHIFT);

                    let display_key = if implicit_shift {
                        format!("Shift+{}", formatted)
                    } else {
                        formatted.clone()
                    };

                    // Build raw diagnostic info
                    let char_display = match key.code {
                        KeyCode::Char(c) => format!("'{}'", c),
                        KeyCode::F(n) => format!("F{}", n),
                        KeyCode::Enter => "Enter".into(),
                        KeyCode::Esc => "Esc".into(),
                        _ => format!("{:?}", key.code),
                    };

                    let mut mods = Vec::new();
                    if key.modifiers.contains(KeyModifiers::SHIFT) || implicit_shift {
                        mods.push("Shift");
                    }
                    if key.modifiers.contains(KeyModifiers::ALT) {
                        mods.push("Alt");
                    }
                    if key.modifiers.contains(KeyModifiers::CONTROL) {
                        mods.push("Ctrl");
                    }
                    let mods_display = if mods.is_empty() {
                        "None".to_string()
                    } else {
                        mods.join(" + ")
                    };
                    let raw_info = format!("Mods: {} | Char: {}", mods_display, char_display);

                    // Look up action
                    let key_str = crate::keybind::binding_ex::format_key(key);
                    let mut action_str = crate::keybind::binding_ex::lookup_key_action(
                        &self.config,
                        &key_str,
                        self.mode,
                        key,
                    );
                    if action_str == "No binding" {
                        action_str = "NONE".to_string();
                    }

                    // Store cloned copies for scankey_info, move originals to popup
                    self.scankey_info =
                        Some((display_key.clone(), action_str.clone(), raw_info.clone()));
                    self.popup.open_scankey(display_key, action_str, raw_info);
                }
            }
            return;
        }
        match key.code {
            KeyCode::Esc => {
                self.popup.close();
            }
            KeyCode::Char('j') | KeyCode::Down => {
                self.popup.config_next();
            }
            KeyCode::Char('k') | KeyCode::Up => {
                self.popup.config_prev();
            }
            KeyCode::Char(' ') => {
                if let Some(PopupContent::Config { items, selected }) = &self.popup.content {
                    if let Some(item) = items.get(*selected) {
                        let data = item.data;
                        if let Some(key) = self.config_bool_keys.get(data) {
                            let key = key.clone();
                            if let Ok(mut json_val) = serde_json::to_value(&self.config) {
                                if let Some(field) = json_val.get_mut(&key) {
                                    if let serde_json::Value::Bool(ref mut v) = field {
                                        *v = !*v;
                                    }
                                }
                                if let Ok(updated) = serde_json::from_value(json_val) {
                                    self.config = updated;
                                    let _ = self.config.save();
                                }
                            }
                            self.open_config_popup();
                        }
                    }
                }
            }
            _ => {}
        }
    }

    pub fn handle_quit_prompt_key(&mut self, key: KeyEvent) {
        match self.quit_prompt {
            QuitPrompt::BufferQuit => match key.code {
                KeyCode::Char('y') | KeyCode::Char('Y') => {
                    if let Err(e) = self.save_active_buffer() {
                        self.set_status_msg(&format!("Save failed: {}", e), MessageKind::Error);
                    } else if self.buffers.len() > 1 {
                        self.close_buffer();
                    } else {
                        self.save_all_window_positions();
                        self.should_quit = true;
                    }
                    self.quit_prompt = QuitPrompt::None;
                }
                KeyCode::Char('n') | KeyCode::Char('N') => {
                    if self.buffers.len() > 1 {
                        self.close_buffer();
                    } else {
                        self.save_all_window_positions();
                        self.should_quit = true;
                    }
                    self.quit_prompt = QuitPrompt::None;
                }
                KeyCode::Char('c') | KeyCode::Char('C') | KeyCode::Esc => {
                    self.quit_prompt = QuitPrompt::None;
                    self.clear_status_msg();
                }
                _ => {}
            },
            QuitPrompt::QuitAllConfirm => {
                match key.code {
                    // [y]: Save, then check next dirty buffer
                    KeyCode::Char('y') | KeyCode::Char('Y') => {
                        if let Err(e) = self.save_active_buffer() {
                            self.set_status_msg(&format!("Save failed: {}", e), MessageKind::Error);
                            self.quit_prompt = QuitPrompt::None; // Cancel quit on write failure
                        } else {
                            self.quit_all_check();
                        }
                    }
                    // [n]: Discard changes, then check next dirty buffer
                    KeyCode::Char('n') | KeyCode::Char('N') => {
                        self.buf_mut().modified = false;
                        self.quit_all_check();
                    }
                    // [c] or [Esc]: Cancel entire quit operation
                    KeyCode::Char('c') | KeyCode::Char('C') | KeyCode::Esc => {
                        self.quit_prompt = QuitPrompt::None;
                        self.clear_status_msg();
                        self.set_status_msg("Quit aborted", MessageKind::Info);
                    }
                    _ => {}
                }
            }
            QuitPrompt::None => {}
        }
    }

    /// Handles keys specifically when the Git Hunk popup is focused.
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
            KeyCode::Char('+') => {
                // Yank only the added/modified lines (those starting with '+')
                if let Some(ref p) = self.popup.git_hunk {
                    let added_lines: Vec<&str> = p
                        .lines
                        .iter()
                        .filter(|l: &&String| l.starts_with('+'))
                        .map(|l: &String| &l[1..])
                        .collect();

                    if !added_lines.is_empty() {
                        self.clipboard = Some(format!("{}\n", added_lines.join("\n")));
                        self.set_status_msg(
                            &format!("Yanked {} added/modified line(s)", added_lines.len()),
                            MessageKind::Info,
                        );
                    } else {
                        self.set_status_msg("No added/modified lines to yank", MessageKind::Info);
                    }
                }
                self.popup.git_hunk = None;
            }
            KeyCode::Char('-') => {
                // Yank the deleted lines (those starting with '-')
                // This is the original text from HEAD that was removed.
                if let Some(ref p) = self.popup.git_hunk {
                    let deleted_lines: Vec<&str> = p
                        .lines
                        .iter()
                        .filter(|l: &&String| l.starts_with('-'))
                        .map(|l: &String| &l[1..])
                        .collect();

                    if !deleted_lines.is_empty() {
                        self.clipboard = Some(format!("{}\n", deleted_lines.join("\n")));
                        self.set_status_msg(
                            &format!("Yanked {} deleted line(s)", deleted_lines.len()),
                            MessageKind::Info,
                        );
                    } else {
                        self.set_status_msg("No deleted lines to yank", MessageKind::Info);
                    }
                }
                self.popup.git_hunk = None;
            }
            _ => {}
        }
    }

    /// Open a bottom-up popup showing the unified diff of the git hunk under the cursor.
    ///
    /// Uses git2 to read the HEAD blob (read-only) and compute the diff —
    /// no disk writes, no shell spawns. Works with unsaved buffer changes.
    pub fn open_hunk_popup(&mut self) {
        let filename = match self.buf().filename.clone() {
            Some(f) => f,
            None => {
                self.set_status_msg("No file associated with buffer", MessageKind::Error);
                return;
            }
        };

        let current_row = self.active_window().row;

        // ── 1. Check cursor is on a hunk ──────────────────────────────
        let git_diffs = self.buf().git_diffs.clone();
        let mut signed_rows: Vec<usize> = git_diffs.keys().cloned().collect();
        if signed_rows.is_empty() {
            self.set_status_msg("No git hunks found", MessageKind::Info);
            return;
        }
        signed_rows.sort_unstable();
        let hunks = group_signed_rows(&signed_rows);
        let on_hunk = hunks
            .iter()
            .any(|h| current_row >= *h.first().unwrap() && current_row <= *h.last().unwrap());
        if !on_hunk {
            self.set_status_msg("Cursor is not on a git hunk", MessageKind::Error);
            return;
        }

        // ── 2. Open git repo (read-only) ──────────────────────────────
        let path = std::path::Path::new(&filename);
        let repo = match git2::Repository::discover(path) {
            Ok(r) => r,
            Err(_) => {
                self.set_status_msg("Not in a git repository", MessageKind::Error);
                return;
            }
        };

        let workdir = match repo.workdir() {
            Some(wd) => wd,
            None => {
                self.set_status_msg("Bare repository", MessageKind::Error);
                return;
            }
        };

        let canon = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
        let rel_path = match canon.strip_prefix(workdir) {
            Ok(rp) => rp.to_string_lossy().to_string(),
            Err(_) => path.to_string_lossy().to_string(),
        };

        // ── 3. Get HEAD content ───────────────────────────────────────
        let head_content = match get_head_file_content(&repo, &rel_path) {
            Some(c) => c,
            None => {
                self.set_status_msg("Could not read HEAD content", MessageKind::Error);
                return;
            }
        };

        // ── 4. Compute patch in-memory ────────────────────────────────
        let current_content = self.buf().rope.to_string();

        let patch = match git2::Patch::from_buffers(
            head_content.as_bytes(),
            Some(std::path::Path::new(&rel_path)),
            current_content.as_bytes(),
            Some(std::path::Path::new(&rel_path)),
            None,
        ) {
            Ok(p) => p,
            Err(_) => {
                self.set_status_msg("Could not compute diff", MessageKind::Error);
                return;
            }
        };

        // ── 5. Find the hunk covering current_row ─────────────────────
        let num_hunks = patch.num_hunks();
        let mut target_hunk_idx = None;

        for i in 0..num_hunks {
            let hunk = match patch.hunk(i) {
                Ok(h) => h,
                Err(_) => continue,
            };

            let new_start = hunk.0.new_start().max(0) as usize;
            let new_count = hunk.0.new_lines().max(0) as usize;

            let covers = if new_count > 0 {
                let start_0 = new_start.saturating_sub(1);
                let end_0 = start_0 + new_count;
                current_row >= start_0 && current_row < end_0
            } else {
                let anchor = new_start.saturating_sub(1);
                current_row >= anchor.saturating_sub(1) && current_row <= anchor + 1
            };

            if covers {
                target_hunk_idx = Some(i);
                break;
            }
        }

        let hunk_idx = match target_hunk_idx {
            Some(idx) => idx,
            None => {
                self.set_status_msg("Could not locate hunk in diff", MessageKind::Error);
                return;
            }
        };

        // ── 6. Extract hunk header ────────────────────────────────────
        let hunk_header = match patch.hunk(hunk_idx) {
            Ok(h) => h.0,
            Err(_) => {
                self.set_status_msg("Failed to read hunk header", MessageKind::Error);
                return;
            }
        };

        let mut hunk_lines = Vec::new();
        hunk_lines.push(format!(
            "@@ -{},{} +{},{} @@",
            hunk_header.old_start(),
            hunk_header.old_lines(),
            hunk_header.new_start(),
            hunk_header.new_lines()
        ));

        // ── 7. Extract diff lines from the hunk ───────────────────────
        if let Ok(num_lines) = patch.num_lines_in_hunk(hunk_idx) {
            for i in 0..num_lines {
                if let Ok(line) = patch.line_in_hunk(hunk_idx, i) {
                    let origin = line.origin();
                    let content = String::from_utf8_lossy(line.content()).to_string();
                    let trimmed = content.trim_end_matches('\n');

                    match origin {
                        '+' => hunk_lines.push(format!("+{}", trimmed)),
                        '-' => hunk_lines.push(format!("-{}", trimmed)),
                        ' ' => hunk_lines.push(format!(" {}", trimmed)),
                        _ => hunk_lines.push(trimmed.to_string()),
                    }
                }
            }
        }

        if hunk_lines.is_empty() {
            self.set_status_msg("Empty hunk", MessageKind::Error);
            return;
        }

        self.popup.git_hunk = Some(crate::popup::git_hunk::GitHunkPopup::new(hunk_lines));
    }

    pub fn run_git_tasks(&mut self) {
        // 1. Check if any editing timers have exceeded 500ms and dispatch
        let dispatch_ids = self.git_debounce.poll_and_dispatch();

        for buffer_id in dispatch_ids {
            if let Some(buf) = self.buf_by_id(buffer_id) {
                let rope = buf.rope.clone();
                let filename = buf.filename.clone();

                // Real Git check: if there is a filename, request real diff
                if filename.is_some() {
                    self.async_gutter
                        .request_diff(buffer_id, &rope, filename.as_deref());
                }
            }
        }

        // 2. Poll for finished background diff updates
        for response in self.async_gutter.poll_results() {
            if let Some(buf) = self.buf_mut_by_id(response.buffer_id) {
                buf.git_diffs = response.diffs;
            }
        }
    }

    // -----------------------------------------------------------------------
    // Git hunk revert
    // -----------------------------------------------------------------------

    /// Revert the git hunk under the cursor (buffer-only, no disk writes).
    ///
    /// Uses git2 to read the HEAD blob (read-only) and compute the diff,
    /// then applies the reverse edit directly to the in-memory rope.
    /// The buffer is marked as modified — nothing is written to disk
    /// until the user explicitly saves.
    pub fn revert_hunk(&mut self) {
        let filename = match self.buf().filename.clone() {
            Some(f) => f,
            None => {
                self.set_status_msg("No file associated with buffer", MessageKind::Error);
                return;
            }
        };

        let current_row = self.active_window().row;

        // ── 1. Check cursor is on a hunk ──────────────────────────────
        let git_diffs = self.buf().git_diffs.clone();
        let mut signed_rows: Vec<usize> = git_diffs.keys().cloned().collect();
        if signed_rows.is_empty() {
            self.set_status_msg("No git hunks found", MessageKind::Info);
            return;
        }
        signed_rows.sort_unstable();
        let hunks = group_signed_rows(&signed_rows);
        let on_hunk = hunks
            .iter()
            .any(|h| current_row >= *h.first().unwrap() && current_row <= *h.last().unwrap());
        if !on_hunk {
            self.set_status_msg("Cursor is not on a git hunk", MessageKind::Error);
            return;
        }

        // ── 2. Open git repo (read-only, no shell) ────────────────────
        let path = std::path::Path::new(&filename);
        let repo = match git2::Repository::discover(path) {
            Ok(r) => r,
            Err(_) => {
                self.set_status_msg("Not in a git repository", MessageKind::Error);
                return;
            }
        };

        let workdir = match repo.workdir() {
            Some(wd) => wd,
            None => {
                self.set_status_msg("Bare repository (no workdir)", MessageKind::Error);
                return;
            }
        };

        let canon = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
        let rel_path = match canon.strip_prefix(workdir) {
            Ok(rp) => rp.to_string_lossy().to_string(),
            Err(_) => path.to_string_lossy().to_string(),
        };

        // ── 3. Get HEAD content (reads git object DB, no disk write) ──
        let head_content = match get_head_file_content(&repo, &rel_path) {
            Some(c) => c,
            None => {
                self.set_status_msg("Could not read HEAD content", MessageKind::Error);
                return;
            }
        };

        // ── 4. Compute patch in-memory ────────────────────────────────
        let current_content = self.buf().rope.to_string();

        let patch = match git2::Patch::from_buffers(
            head_content.as_bytes(),
            Some(std::path::Path::new(&rel_path)),
            current_content.as_bytes(),
            Some(std::path::Path::new(&rel_path)),
            None,
        ) {
            Ok(p) => p,
            Err(_) => {
                self.set_status_msg("Could not compute diff", MessageKind::Error);
                return;
            }
        };

        // ── 5. Find the hunk covering current_row ─────────────────────
        let num_hunks = patch.num_hunks();
        let mut target_hunk_idx = None;

        for i in 0..num_hunks {
            let hunk = match patch.hunk(i) {
                Ok(h) => h,
                Err(_) => continue,
            };

            let new_start = hunk.0.new_start().max(0) as usize; // 1-based
            let new_count = hunk.0.new_lines().max(0) as usize;

            let covers = if new_count > 0 {
                let start_0 = new_start.saturating_sub(1);
                let end_0 = start_0 + new_count;
                current_row >= start_0 && current_row < end_0
            } else {
                // Pure-deletion hunk: new_start is the anchor line
                let anchor = new_start.saturating_sub(1);
                current_row >= anchor.saturating_sub(1) && current_row <= anchor + 1
            };

            if covers {
                target_hunk_idx = Some(i);
                break;
            }
        }

        let hunk_idx = match target_hunk_idx {
            Some(idx) => idx,
            None => {
                self.set_status_msg("Could not locate hunk in diff", MessageKind::Error);
                return;
            }
        };

        // ── 6. Extract hunk metadata ──────────────────────────────────
        let hunk = patch.hunk(hunk_idx).unwrap();
        let new_start_1 = hunk.0.new_start().max(0) as usize;
        let new_count = hunk.0.new_lines().max(0) as usize;
        let old_start_1 = hunk.0.old_start().max(0) as usize;
        let old_count = hunk.0.old_lines().max(0) as usize;

        // ── 7. Build reverted text from HEAD content ──────────────────
        let head_lines: Vec<&str> = head_content.lines().collect();
        let old_start_0 = old_start_1.saturating_sub(1);
        let old_end = old_start_0 + old_count;

        let mut reverted_text = String::new();
        for line in head_lines.get(old_start_0..old_end).unwrap_or(&[]) {
            reverted_text.push_str(line);
            reverted_text.push('\n');
        }

        // ── 8. Apply reverse edit to the rope ─────────────────────────
        {
            let (win, buf) = self.active_window_and_buf_mut();
            buf.push_undo(win.row, win.col);

            let new_start_0 = new_start_1.saturating_sub(1);
            let new_end_0 = new_start_0 + new_count;

            // Remove current hunk lines
            if new_count > 0 && new_end_0 <= buf.len_lines() {
                let start_char = buf.rope.line_to_char(new_start_0);
                let end_char = buf.rope.line_to_char(new_end_0);
                if end_char > start_char {
                    buf.rope.remove(start_char..end_char);
                }
            }

            // Insert reverted (HEAD) lines
            if !reverted_text.is_empty() {
                let insert_line = new_start_0.min(buf.len_lines());
                let insert_char = buf.rope.line_to_char(insert_line);
                buf.rope.insert(insert_char, &reverted_text);
            }

            buf.modified = true;
            buf.parse_syntax();

            // Position cursor at start of reverted region
            let cursor_row = new_start_0.min(buf.len_lines().saturating_sub(1));
            win.row = cursor_row;
            win.col = 0;
            win.desired_col = 0;
            self.scroll_active_window_to_cursor();
        }

        // ── 9. Re-request gutter diff ─────────────────────────────────
        let bid = self.buf().id;
        let rope = self.buf().rope.clone();
        self.async_gutter.request_diff(bid, &rope, Some(&filename));

        self.set_status_msg("Reverted hunk", MessageKind::Success);
    }

    /// Jump the cursor to the start of the next git hunk.
    pub fn jump_to_next_hunk(&mut self) {
        let current_row = self.active_window().row;

        let mut signed_rows: Vec<usize> = self.buf().git_diffs.keys().cloned().collect();
        if signed_rows.is_empty() {
            self.set_status_msg("No git hunks found", MessageKind::Info);
            return;
        }
        signed_rows.sort_unstable();

        let hunk_starts = group_signed_rows(&signed_rows)
            .into_iter()
            .map(|h| h[0])
            .collect::<Vec<_>>();

        let target_row = hunk_starts
            .iter()
            .find(|&&r| r > current_row)
            .cloned()
            .or_else(|| hunk_starts.first().cloned());

        if let Some(row) = target_row {
            self.active_window_mut().save_jump_position();
            let win = self.active_window_mut();
            win.row = row;
            win.col = 0;
            win.desired_col = 0;
            self.scroll_active_window_to_cursor();

            self.set_status_msg(
                &format!("Jumped to hunk at line {}", row + 1),
                MessageKind::Info,
            );
        }
    }

    /// Jump the cursor to the start of the previous git hunk.
    pub fn jump_to_prev_hunk(&mut self) {
        let current_row = self.active_window().row;

        let mut signed_rows: Vec<usize> = self.buf().git_diffs.keys().cloned().collect();
        if signed_rows.is_empty() {
            self.set_status_msg("No git hunks found", MessageKind::Info);
            return;
        }
        signed_rows.sort_unstable();

        let hunk_starts = group_signed_rows(&signed_rows)
            .into_iter()
            .map(|h| h[0])
            .collect::<Vec<_>>();

        let target_row = hunk_starts
            .iter()
            .rev()
            .find(|&&r| r < current_row)
            .cloned()
            .or_else(|| hunk_starts.last().cloned());

        if let Some(row) = target_row {
            self.active_window_mut().save_jump_position();
            let win = self.active_window_mut();
            win.row = row;
            win.col = 0;
            win.desired_col = 0;
            self.scroll_active_window_to_cursor();

            self.set_status_msg(
                &format!("Jumped to hunk at line {}", row + 1),
                MessageKind::Info,
            );
        }
    }

    /// Returns the current git branch name for the active buffer.
    /// Uses git2 for fast local resolution (no shell spawning).
    pub fn current_git_branch(&self) -> Option<String> {
        let filename = self.buf().filename.as_deref()?;
        let path = std::path::Path::new(filename);
        let repo = git2::Repository::discover(path).ok()?;
        let head = repo.head().ok()?;
        head.shorthand().map(String::from)
    }

    /// Returns the raw stats for the active buffer's git diffs.
    pub fn git_diff_stats(&self) -> (usize, usize, usize) {
        self.buf().git_diff_stats()
    }
}
