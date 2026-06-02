//! Tag jump / tag-back handlers.
//!
//! Supports two backends (tried in order):
//! 1. **ctagd** — ctags daemon for fast definitions via SQLite.
//! 2. **ctags / gentag** — traditional tags file fallback.
//!
//! Keybindings:
//! - `C-]` / `gd`  → `tag_under_cursor` — jump to definition
//! - `:tag <name>`  → `jump_to_tag` — jump to a named tag
//! - `C-t`          → `tag_back` — return from tag jump
//! - `:symbols <q>` → `symbols_search` — workspace symbol search

use crate::ed::tag::{TagEntry, TagStackEntry};
use crate::ed::MessageKind;
use crate::Editor;
use crossterm::event::KeyCode;

impl Editor {
    // ═══════════════════════════════════════════════════════════════════
    // Public API
    // ═══════════════════════════════════════════════════════════════════

    /// `C-]` / `gd` — jump to the definition of the word under the cursor.
    pub fn tag_under_cursor(&mut self) {
        let word = match self.get_word_under_cursor() {
            Some(w) => w,
            None => {
                self.set_status_msg("No word under cursor", MessageKind::Error);
                return;
            }
        };

        // ── Attempt 1: ctagd `definition` ─────────────────────────
        if self.ctagd.is_available() {
            if let Some(()) = self.ctagd_definition(&word) {
                return;
            }
        }

        // ── Attempt 2: ctags fallback ────────────────────────────────
        self.ctags_jump(&word);
    }

    /// `:tag <name>` — jump to a named tag.
    pub fn jump_to_tag(&mut self, name: &str) {
        // ── Attempt 1: ctagd `goto` ───────────────────────────────
        if self.ctagd.is_available() {
            if let Some(()) = self.ctagd_goto(name) {
                return;
            }
        }

        // ── Attempt 2: ctags fallback ────────────────────────────────
        self.ctags_jump(name);
    }

    /// `:symbols <query>` — search workspace symbols via ctagd.
    pub fn symbols_search(&mut self, query: &str) {
        let repo_root = match self.resolve_repo_root() {
            Some(r) => r,
            None => {
                self.set_status_msg("Not in a git repository", MessageKind::Error);
                return;
            }
        };

        self.ctagd.refresh_availability();

        if !self.ctagd.is_available() {
            self.set_status_msg(
                "ctagd not available. Start the daemon or install it for symbol search.",
                MessageKind::Error,
            );
            return;
        }

        let results = self.ctagd.workspace_symbols(&repo_root, query);

        match results {
            Some(syms) if syms.is_empty() => {
                self.set_status_msg(
                    &format!("No symbols matching '{}'", query),
                    MessageKind::Info,
                );
            }
            Some(syms) if syms.len() == 1 => {
                let sym = &syms[0];
                let _ = self.push_tag_stack();
                self.ctagd_do_jump(
                    &sym.relative_path,
                    sym.line,
                    sym.column,
                    &repo_root,
                    &sym.name,
                    &format!("[{}]", sym.kind),
                    1,
                );
            }
            Some(syms) => {
                let _ = self.push_tag_stack();
                self.popup.open_workspace_symbols(syms);
                self.set_status_msg(
                    &format!("{} symbols found. Select one.", query),
                    MessageKind::Info,
                );
            }
            None => {
                self.set_status_msg("ctagd connection failed", MessageKind::Error);
            }
        }
    }

    /// Key handler for the Workspace Symbols popup.
    pub fn handle_workspace_symbols_key(&mut self, key: crate::event::KeyEvent) {
        match key.code {
            KeyCode::Up | KeyCode::Char('k') => {
                if let Some(ref mut p) = self.popup.workspace_symbols {
                    p.move_up();
                }
            }
            KeyCode::Down | KeyCode::Char('j') => {
                if let Some(ref mut p) = self.popup.workspace_symbols {
                    p.move_down();
                }
            }
            KeyCode::Enter => {
                let selected = self.popup.workspace_symbols.as_ref().and_then(
                    |p: &crate::popup::workspace_symbols::WorkspaceSymbolsPopup| {
                        p.selected_entry().cloned()
                    },
                );

                let repo_root = self.resolve_repo_root();

                self.popup.close();

                if let (Some(sym), Some(root)) = (selected, repo_root) {
                    self.ctagd_do_jump(
                        &sym.relative_path,
                        sym.line,
                        sym.column,
                        &root,
                        &sym.name,
                        &format!("[{}]", sym.kind),
                        1,
                    );
                }
            }
            KeyCode::Esc => {
                self.popup.close();
                self.tag_manager.pop();
                self.set_status_msg("Symbol search cancelled", MessageKind::Info);
            }
            _ => {}
        }
    }

    /// `C-t` — return to the previous position from the tag stack.
    pub fn tag_back(&mut self) {
        let entry = match self.tag_manager.pop() {
            Some(e) => e,
            None => {
                self.set_status_msg("Tag stack empty", MessageKind::Info);
                return;
            }
        };

        let depth = self.tag_manager.stack_depth();

        let target_bid = self
            .buffers
            .iter()
            .find(|b| b.filename.as_deref() == entry.filename.as_deref())
            .map(|b| b.id);

        if let Some(bid) = target_bid {
            self.switch_window_to_buffer(bid);
        } else if let Some(ref filename) = entry.filename {
            self.open_buffer(Some(filename.to_string()));
        }

        let (win, buf) = self.active_window_and_buf_mut();
        let max_row = buf.len_lines().saturating_sub(1);
        win.row = entry.row.min(max_row);
        win.col = entry.col.min(buf.line_char_len(win.row).saturating_sub(1));
        win.desired_col = win.col;
        self.scroll_active_window_to_cursor();

        self.set_status_msg(
            &format!("Tag back (stack depth: {})", depth),
            MessageKind::Info,
        );
    }

    /// `:retag` — regenerate the tags file for the current repo.
    pub fn retag(&mut self) {
        let repo_root = match self.resolve_repo_root() {
            Some(r) => r,
            None => {
                self.set_status_msg("Not in a git repository", MessageKind::Error);
                return;
            }
        };

        if self.ctagd.is_available() {
            if let Some((root, file)) = self.repo_root_and_relative_file() {
                let content = self.buf().rope.to_string();
                self.ctagd.notify_saved(&root, &file, &content);
            }
        }

        if self.tag_manager.load_for_repo(&repo_root) {
            let count = self.tag_manager.tag_count();
            self.set_status_msg(
                &format!("Tags regenerated ({} tags loaded)", count),
                MessageKind::Success,
            );
        } else {
            self.set_status_msg(
                "Failed to generate tags. Install gentag or ctags.",
                MessageKind::Error,
            );
        }
    }

    /// `:tags` — show tag manager status.
    pub fn show_tag_info(&mut self) {
        let count = self.tag_manager.tag_count();
        let depth = self.tag_manager.stack_depth();
        let root = self
            .tag_manager
            .loaded_root()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|| "none".to_string());

        let daemon = if self.ctagd.is_available() {
            if let Some(repo_root) = self.resolve_repo_root() {
                match self.ctagd.info(&repo_root) {
                    Some(info) => format!("{} ({})", info.backend, info.index_status),
                    None => "available".to_string(),
                }
            } else {
                "available".to_string()
            }
        } else {
            "unavailable".to_string()
        };

        self.set_status_msg(
            &format!(
                "Tags: {} | Stack: {} | Root: {} | ctagd: {}",
                count,
                depth,
                short_repo_name(&root),
                daemon
            ),
            MessageKind::Info,
        );
    }

    // ═══════════════════════════════════════════════════════════════════
    // ctagd helpers
    // ═══════════════════════════════════════════════════════════════════

    fn ctagd_definition(&mut self, symbol: &str) -> Option<()> {
        let (repo_root, file) = self.repo_root_and_relative_file()?;
        let (line, col) = {
            let win = self.active_window();
            (win.row, win.col)
        };

        self.ctagd.refresh_availability();

        let result = self
            .ctagd
            .definition(&repo_root, &file, line, col, symbol)?;

        let _ = self.push_tag_stack();

        self.ctagd_do_jump(
            &result.file,
            result.line,
            result.column,
            &repo_root,
            symbol,
            result.display.as_deref().unwrap_or(""),
            1,
        );

        Some(())
    }

    fn ctagd_goto(&mut self, name: &str) -> Option<()> {
        let repo_root = self.resolve_repo_root()?;

        self.ctagd.refresh_availability();

        let result = self.ctagd.goto(&repo_root, name)?;

        let _ = self.push_tag_stack();

        self.ctagd_do_jump(
            &result.file,
            result.line,
            result.column,
            &repo_root,
            name,
            result.display.as_deref().unwrap_or(""),
            1,
        );

        Some(())
    }

    // ═══════════════════════════════════════════════════════════════════
    // Smart column positioning
    // ═══════════════════════════════════════════════════════════════════

    fn refine_jump_position(&mut self, symbol_name: &str, target_line: usize, original_col: usize) {
        if original_col > 0 {
            let buf = self.buf();
            if target_line < buf.len_lines() {
                let line_text = buf.line_text(target_line);
                let byte_offset = char_offset_to_byte_offset(&line_text, original_col);
                if byte_offset < line_text.len() {
                    let slice = &line_text[byte_offset..];
                    if slice.starts_with(symbol_name) {
                        return;
                    }
                }
            }
        }

        let search_radius = 5i32;
        let (win, buf) = self.active_window_and_buf_mut();
        let max_row = buf.len_lines().saturating_sub(1) as i32;
        let start = ((target_line as i32) - search_radius).max(0) as usize;
        let end = ((target_line as i32) + search_radius).min(max_row) as usize;

        for r in start..=end {
            let line_text = buf.line_text(r);
            if let Some(col) = find_word_in_line(&line_text, symbol_name) {
                win.row = r;
                win.col = col;
                win.desired_col = col;
                return;
            }
        }

        for r in start..=end {
            let line_text = buf.line_text(r);
            if let Some(byte_pos) = line_text.find(symbol_name) {
                let col = byte_to_char_offset(&line_text, byte_pos);
                win.row = r;
                win.col = col;
                win.desired_col = col;
                return;
            }
        }
    }

    fn ctagd_do_jump(
        &mut self,
        relative_file: &str,
        target_line: usize,
        target_column: usize,
        repo_root: &std::path::Path,
        name: &str,
        display: &str,
        _total: usize,
    ) {
        match self.resolve_tag_file(relative_file, repo_root) {
            Some(full_path) => {
                let path_str = full_path.to_string_lossy().to_string();

                let existing_bid = self
                    .buffers
                    .iter()
                    .find(|b| b.filename.as_deref() == Some(&path_str))
                    .map(|b| b.id);

                if let Some(bid) = existing_bid {
                    self.switch_window_to_buffer(bid);
                } else {
                    self.open_buffer(Some(path_str));
                }

                {
                    let (win, buf) = self.active_window_and_buf_mut();
                    let max_row = buf.len_lines().saturating_sub(1);
                    win.row = target_line.min(max_row);
                    win.col = target_column;
                    win.desired_col = win.col;
                    win.save_jump_position();
                }

                self.refine_jump_position(name, target_line, target_column);
                self.center_viewport_on_cursor();

                let display_suffix = if display.is_empty() {
                    String::new()
                } else {
                    format!(" {}", display)
                };

                self.set_status_msg(
                    &format!(
                        "ctagd: {} → {}:{}{}",
                        name,
                        relative_file,
                        target_line + 1,
                        display_suffix
                    ),
                    MessageKind::Info,
                );
            }
            None => {
                self.tag_manager.pop();
                self.set_status_msg(
                    &format!(
                        "Tag file not found: {} (root: {})",
                        relative_file,
                        repo_root.display()
                    ),
                    MessageKind::Error,
                );
            }
        }
    }

    // ═══════════════════════════════════════════════════════════════════
    // ctags fallback
    // ═══════════════════════════════════════════════════════════════════

    fn ctags_jump(&mut self, name: &str) {
        let repo_root = match self.resolve_repo_root() {
            Some(r) => r,
            None => {
                self.set_status_msg("Not in a git repository", MessageKind::Error);
                return;
            }
        };

        if !self.tag_manager.ensure_loaded(&repo_root) {
            self.set_status_msg(
                "No tags file found. Install gentag or ctags, then run :retag",
                MessageKind::Error,
            );
            return;
        }

        let matches = self.tag_manager.lookup(name).to_vec();
        if matches.is_empty() {
            self.set_status_msg(&format!("Tag not found: {}", name), MessageKind::Error);
            return;
        }

        let _ = self.push_tag_stack();

        if matches.len() == 1 {
            self.do_ctags_jump(&matches[0], &repo_root, name, 1);
        } else {
            self.popup.tag_candidates = Some(
                crate::popup::tag_candidates::TagCandidatesPopup::new(matches),
            );
            self.set_status_msg(
                &format!("{} matching tags found. Select one.", name),
                MessageKind::Info,
            );
        }
    }

    fn do_ctags_jump(
        &mut self,
        entry: &TagEntry,
        repo_root: &std::path::Path,
        name: &str,
        total: usize,
    ) {
        let relative_file = entry.file.to_string_lossy().to_string();

        match self.resolve_tag_file(&relative_file, repo_root) {
            Some(full_path) => {
                let path_str = full_path.to_string_lossy().to_string();

                let existing_bid = self
                    .buffers
                    .iter()
                    .find(|b| b.filename.as_deref() == Some(&path_str))
                    .map(|b| b.id);

                if let Some(bid) = existing_bid {
                    self.switch_window_to_buffer(bid);
                } else {
                    self.open_buffer(Some(path_str));
                }

                let target_row;
                {
                    let (win, buf) = self.active_window_and_buf_mut();
                    target_row = entry
                        .line
                        .saturating_sub(1)
                        .min(buf.len_lines().saturating_sub(1));
                    win.row = target_row;
                    win.col = 0;
                    win.desired_col = 0;
                    win.save_jump_position();
                }

                self.refine_jump_position(name, target_row, 0);
                self.center_viewport_on_cursor();

                let kind_suffix = match &entry.kind {
                    Some(k) => format!(" [{}]", k),
                    None => String::new(),
                };
                let multi_suffix = if total > 1 {
                    format!(" ({} matches, :tn for next)", total)
                } else {
                    String::new()
                };

                self.set_status_msg(
                    &format!(
                        "Tag: {} → {}:{}{}{}",
                        name, relative_file, entry.line, kind_suffix, multi_suffix
                    ),
                    MessageKind::Info,
                );
            }
            None => {
                self.tag_manager.pop();
                self.set_status_msg(
                    &format!(
                        "Tag file not found: {} (root: {})",
                        relative_file,
                        repo_root.display()
                    ),
                    MessageKind::Error,
                );
            }
        }
    }

    /// Key handler for the Tag Candidates popup (ctags).
    pub fn handle_tag_candidates_key(&mut self, key: crate::event::KeyEvent) {
        match key.code {
            KeyCode::Up | KeyCode::Char('k') => {
                if let Some(ref mut p) = self.popup.tag_candidates {
                    p.move_up();
                }
            }
            KeyCode::Down | KeyCode::Char('j') => {
                if let Some(ref mut p) = self.popup.tag_candidates {
                    p.move_down();
                }
            }
            KeyCode::Enter => {
                let selected_entry = self
                    .popup
                    .tag_candidates
                    .as_ref()
                    .and_then(|p| p.selected_entry().cloned());

                let repo_root = self.resolve_repo_root();

                self.popup.close();

                if let (Some(entry), Some(root)) = (selected_entry, repo_root) {
                    self.do_ctags_jump(&entry, &root, &entry.name, 1);
                }
            }
            KeyCode::Esc => {
                self.popup.close();
                self.tag_manager.pop();
                self.set_status_msg("Tag jump cancelled", MessageKind::Info);
            }
            _ => {}
        }
    }

    // ═══════════════════════════════════════════════════════════════════
    // Shared helpers
    // ═══════════════════════════════════════════════════════════════════

    fn push_tag_stack(&mut self) -> Option<()> {
        let win = self.active_window();
        self.tag_manager.push(TagStackEntry {
            buffer_id: win.buffer_id(),
            row: win.row,
            col: win.col,
            filename: self.active_filename().map(|s| s.to_string()),
        });
        Some(())
    }

    fn repo_root_and_relative_file(&mut self) -> Option<(std::path::PathBuf, String)> {
        let repo_root = self.resolve_repo_root()?;
        let filename = self.active_filename()?.to_string();

        let abs_path = match std::fs::canonicalize(&filename) {
            Ok(p) => p,
            Err(_) => std::path::PathBuf::from(&filename),
        };

        let canon_root = match std::fs::canonicalize(&repo_root) {
            Ok(p) => p,
            Err(_) => repo_root.clone(),
        };

        let relative = abs_path
            .strip_prefix(&canon_root)
            .ok()
            .map(|p| {
                let s = p.to_string_lossy();
                s.strip_prefix('/').unwrap_or(&s).to_string()
            })
            .unwrap_or_else(|| {
                let s = abs_path.to_string_lossy();
                let root_s = canon_root.to_string_lossy();
                s.strip_prefix(root_s.as_ref())
                    .map(|rest| rest.strip_prefix('/').unwrap_or(rest).to_string())
                    .unwrap_or(filename)
            });

        Some((canon_root, relative))
    }

    fn resolve_tag_file(
        &self,
        relative_file: &str,
        repo_root: &std::path::Path,
    ) -> Option<std::path::PathBuf> {
        let rel_path = std::path::Path::new(relative_file);

        if rel_path.is_absolute() && rel_path.exists() {
            return Some(rel_path.to_path_buf());
        }

        let full_path = repo_root.join(relative_file);
        if full_path.exists() {
            return Some(full_path);
        }

        if let Ok(canon_root) = std::fs::canonicalize(repo_root) {
            let canon_path = canon_root.join(relative_file);
            if canon_path.exists() {
                return Some(canon_path);
            }
        }

        for buf in &self.buffers {
            if let Some(ref filename) = buf.filename {
                let fpath = std::path::Path::new(filename);
                if fpath.ends_with(rel_path) && fpath.exists() {
                    return Some(fpath.to_path_buf());
                }
            }
        }

        let walker = ignore::WalkBuilder::new(repo_root)
            .hidden(true)
            .git_ignore(true)
            .build();

        let mut checked = 0u32;
        for entry in walker.filter_map(|e: Result<ignore::DirEntry, _>| e.ok()) {
            checked += 1;
            if checked > 5000 {
                break;
            }
            let path = entry.path();
            if path.ends_with(rel_path) && path.exists() {
                return Some(path.to_path_buf());
            }
        }

        None
    }

    pub fn notify_ctagd_saved(&mut self) {
        if !self.ctagd.is_available() {
            return;
        }

        let repo_root = match self.resolve_repo_root() {
            Some(r) => r,
            None => return,
        };

        let filename = match self.active_filename() {
            Some(f) => f.to_string(),
            None => return,
        };

        let abs_path = match std::fs::canonicalize(&filename) {
            Ok(p) => p,
            Err(_) => std::path::PathBuf::from(&filename),
        };

        let canon_root = match std::fs::canonicalize(&repo_root) {
            Ok(p) => p,
            Err(_) => repo_root,
        };

        let relative = abs_path
            .strip_prefix(&canon_root)
            .ok()
            .map(|p| {
                let s = p.to_string_lossy();
                s.strip_prefix('/').unwrap_or(&s).to_string()
            })
            .unwrap_or_else(|| {
                let s = abs_path.to_string_lossy();
                let root_s = canon_root.to_string_lossy();
                s.strip_prefix(root_s.as_ref())
                    .map(|rest| rest.strip_prefix('/').unwrap_or(rest).to_string())
                    .unwrap_or(filename.clone())
            });

        let content = self.buf().rope.to_string();
        log::debug!(
            "ctagd notify_saved: root={}, file={}, content_len={}",
            canon_root.display(),
            relative,
            content.len()
        );
        self.ctagd.notify_saved(&canon_root, &relative, &content);
    }

    pub fn daemon_info(&mut self) {
        let repo_root = match self.resolve_repo_root() {
            Some(r) => r,
            None => {
                self.set_status_msg("Not in a git repository", MessageKind::Error);
                return;
            }
        };

        self.ctagd.refresh_availability();

        if !self.ctagd.is_available() {
            self.set_status_msg("ctagd daemon not running", MessageKind::Error);
            return;
        }

        match self.ctagd.info(&repo_root) {
            Some(info) => {
                let index_icon = match info.index_status.as_str() {
                    "ready" => "✓",
                    "scanning" => "⟳",
                    _ => "…",
                };
                self.set_status_msg(
                    &format!(
                        "ctagd {} | Backend: {} | Index: {} {} files, {} symbols",
                        short_repo_name(&info.repo_root),
                        info.backend,
                        index_icon,
                        info.indexed_files,
                        info.indexed_symbols,
                    ),
                    MessageKind::Info,
                );
            }
            None => {
                self.set_status_msg(
                    "ctagd: failed to connect or parse response",
                    MessageKind::Error,
                );
            }
        }
    }

    pub fn daemon_scan(&mut self) {
        let repo_root = match self.resolve_repo_root() {
            Some(r) => r,
            None => {
                self.set_status_msg("Not in a git repository", MessageKind::Error);
                return;
            }
        };

        self.ctagd.refresh_availability();

        if !self.ctagd.is_available() {
            self.set_status_msg(
                "ctagd daemon not running (socket not found at /tmp/.ctagd.sock)",
                MessageKind::Error,
            );
            return;
        }

        if self.ctagd.scan(&repo_root) {
            self.set_status_msg(
                &format!(
                    "ctagd: re-indexing {} in background...",
                    short_repo_name(&repo_root.to_string_lossy())
                ),
                MessageKind::Info,
            );
        } else {
            self.set_status_msg("ctagd: failed to trigger scan", MessageKind::Error);
        }
    }

    pub fn daemon_status(&mut self) {
        self.ctagd.refresh_availability();

        if !self.ctagd.is_available() {
            self.set_status_msg(
                "ctagd daemon not running (socket not found at /tmp/.ctagd.sock)",
                MessageKind::Error,
            );
            return;
        }

        match self.ctagd.sessions() {
            Some(sessions) if sessions.is_empty() => {
                self.set_status_msg(
                    "ctagd: daemon running, no active sessions",
                    MessageKind::Info,
                );
            }
            Some(sessions) => {
                let mut lines = Vec::new();
                lines.push("ctagd — Active Sessions".to_string());
                lines.push("─".repeat(50).to_string());

                for s in &sessions {
                    let icon = match s.index_status.as_str() {
                        "ready" => "✓",
                        "scanning" => "⟳",
                        _ => "…",
                    };
                    lines.push(format!(
                        "{} {}  Backend: {}  Symbols: {}",
                        icon, s.repo_root, s.backend, s.indexed_symbols,
                    ));
                }

                lines.push("".to_string());
                lines.push(format!("Total: {} session(s)", sessions.len()));

                if lines.len() <= 6 {
                    let compact: Vec<String> = sessions
                        .iter()
                        .map(|s| {
                            let icon = match s.index_status.as_str() {
                                "ready" => "✓",
                                _ => "⟳",
                            };
                            format!("{}{}", icon, short_repo_name(&s.repo_root))
                        })
                        .collect();
                    self.set_status_msg(
                        &format!("ctagd sessions: {}", compact.join("  ")),
                        MessageKind::Info,
                    );
                } else {
                    self.popup.open_error(&lines.join("\n"));
                }
            }
            None => {
                self.set_status_msg(
                    "ctagd: failed to connect or parse response",
                    MessageKind::Error,
                );
            }
        }
    }
}

fn short_repo_name(path: &str) -> String {
    std::path::Path::new(path)
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| path.to_string())
}

fn find_word_in_line(line: &str, word: &str) -> Option<usize> {
    let word_bytes = word.as_bytes();
    let line_bytes = line.as_bytes();
    let mut start = 0;

    while start + word_bytes.len() <= line_bytes.len() {
        if let Some(pos) = line[start..].find(word) {
            let abs_pos = start + pos;
            let after = abs_pos + word_bytes.len();

            let before_ok = abs_pos == 0
                || !line_bytes[abs_pos - 1].is_ascii_alphanumeric()
                    && line_bytes[abs_pos - 1] != b'_';
            let after_ok = after >= line_bytes.len()
                || !line_bytes[after].is_ascii_alphanumeric() && line_bytes[after] != b'_';

            if before_ok && after_ok {
                return Some(byte_to_char_offset(line, abs_pos));
            }
            start = abs_pos + 1;
        } else {
            break;
        }
    }
    None
}

fn byte_to_char_offset(s: &str, byte_offset: usize) -> usize {
    s[..byte_offset].chars().count()
}

fn char_offset_to_byte_offset(s: &str, char_offset: usize) -> usize {
    s.char_indices()
        .nth(char_offset)
        .map(|(i, _)| i)
        .unwrap_or(s.len())
}
