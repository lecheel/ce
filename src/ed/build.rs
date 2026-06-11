//! Build command — run `cargo build --release` and capture errors/warnings
//! into a navigable build buffer (like a quickfix list).

use crate::ed::buffer::BufferKind;
use crate::ed::editor::Editor;
use crate::ed::mode::MessageKind;
use crate::ed::syntax::SyntaxState;
use ropey::Rope;
use std::path::{Path, PathBuf};

// ═══════════════════════════════════════════════════════════════════
// Data types
// ═══════════════════════════════════════════════════════════════════

#[derive(Debug, Clone)]
pub struct BuildDiagnostic {
    pub file_path: PathBuf,
    pub line_number: usize,
    pub column: usize,
    pub severity: BuildSeverity,
    pub message: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BuildSeverity {
    Error,
    Warning,
    Note,
}

pub struct BuildResult {
    pub stdout: String,
    pub stderr: String,
    pub success: bool,
}

#[derive(Default)]
pub struct BuildState {
    pub in_progress: bool,
    pub start_time: Option<std::time::Instant>,
    pub spinner_idx: usize,
    pub diagnostics: Vec<BuildDiagnostic>,
    pub response_rx: Option<std::sync::mpsc::Receiver<BuildResult>>,
    pub buffer_id: Option<usize>,
}

const SPINNER_CHARS: &[char] = &['⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧', '⠇', '⠏'];

// ═══════════════════════════════════════════════════════════════════
// Build buffer key handler
// ═══════════════════════════════════════════════════════════════════

impl Editor {
    pub fn handle_build_buffer_key(&mut self, key: crossterm::event::KeyEvent) -> bool {
        use crossterm::event::KeyCode;

        if key.kind != crossterm::event::KeyEventKind::Press {
            return false;
        }

        match key.code {
            // ── Navigate to error under cursor ───────────────────
            KeyCode::Enter => {
                self.build_goto_error();
                true
            }

            // ── Next / previous error ────────────────────────────
            KeyCode::Char('n') => {
                self.build_next_error();
                true
            }
            KeyCode::Char('N') | KeyCode::Char('p') => {
                self.build_prev_error();
                true
            }

            // ── Yank snippet / brace content ─────────────────────
            KeyCode::Char('y') => {
                self.build_yank_snippet();
                true
            }
            KeyCode::Char('b') => {
                self.build_yank_brace_content();
                true
            }

            // ── Close build buffer ───────────────────────────────
            KeyCode::Char('q') => {
                self.build_close();
                true
            }

            // ── Re-run build ─────────────────────────────────────
            KeyCode::Char('r') => {
                self.run_build();
                true
            }

            // ── Movement keys — fall through to normal handling ──
            KeyCode::Up
            | KeyCode::Down
            | KeyCode::Char('j')
            | KeyCode::Char('k')
            | KeyCode::Char('g')
            | KeyCode::Char('G')
            | KeyCode::PageUp
            | KeyCode::PageDown => false,

            _ => true, // Swallow everything else in build buffer
        }
    }
}

// ═══════════════════════════════════════════════════════════════════
// Build commands
// ═══════════════════════════════════════════════════════════════════

impl Editor {
    /// Run `cargo build --release` and show results in a build buffer.
    pub fn run_build(&mut self) {
        if self.build.in_progress {
            self.set_status_msg("Build already in progress...", MessageKind::Info);
            return;
        }

        let project_root = self.find_cargo_root();

        let (tx, rx): (
            std::sync::mpsc::Sender<BuildResult>,
            std::sync::mpsc::Receiver<BuildResult>,
        ) = std::sync::mpsc::channel();

        let root_clone = project_root.clone();

        self.build.in_progress = true;
        self.build.start_time = Some(std::time::Instant::now());
        self.build.spinner_idx = 0;
        self.build.response_rx = Some(rx);

        std::thread::spawn(move || {
            let output = std::process::Command::new("cargo")
                .args([
                    "build",
                    "--release",
                    "--color=never",
                    "--message-format=short",
                ])
                .current_dir(&root_clone)
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::piped())
                .output();

            let result = match output {
                Ok(o) => BuildResult {
                    stdout: String::from_utf8_lossy(&o.stdout).to_string(),
                    stderr: String::from_utf8_lossy(&o.stderr).to_string(),
                    success: o.status.success(),
                },
                Err(e) => BuildResult {
                    stdout: String::new(),
                    stderr: e.to_string(),
                    success: false,
                },
            };
            let _ = tx.send(result);
        });

        // Create build buffer with animation header
        let header = format!("  [BUILD] cargo build --release — Building... 0.0s ⠋\n  {}\n\n  (compiling, please wait...)\n", "─".repeat(40));
        let build_id = self.ensure_build_buffer(&header);
        self.build.buffer_id = Some(build_id);

        // Switch to the build buffer
        self.switch_window_to_buffer(build_id);

        let win = self.active_window_mut();
        win.row = 0;
        win.col = 0;
        win.scroll_line = 0;

        self.set_status_msg("Building...", MessageKind::Info);
    }

    /// Navigate to the build error under the cursor (Enter key).
    pub fn build_goto_error(&mut self) {
        let (cursor_row, line_text) = match self.current_buffer_line() {
            Some(pair) => pair,
            None => return,
        };

        let mut diag = if let Some(d) = self.parse_location_from_line(&line_text) {
            d
        } else {
            // Walk upward to find the nearest diagnostic line
            let mut found = None;
            for line in (0..cursor_row).rev() {
                let text = self.buf().line_text(line);
                if let Some(d) = self.parse_location_from_line(&text) {
                    found = Some(d);
                    break;
                }
            }
            match found {
                Some(d) => d,
                None => {
                    self.set_status_msg("No error location on this line", MessageKind::Info);
                    return;
                }
            }
        };

        // Resolve relative paths against project root
        if diag.file_path.is_relative() {
            let project_root = self.find_cargo_root();
            diag.file_path = project_root.join(&diag.file_path);
        }

        let path_str = diag.file_path.to_string_lossy().to_string();
        self.open_buffer(Some(path_str));

        // Position cursor at the error location
        let max_line = self.buf().len_lines().saturating_sub(1);
        {
            let win = self.active_window_mut();
            win.row = diag.line_number.saturating_sub(1).min(max_line);
            win.col = diag.column.saturating_sub(1);
            win.desired_col = win.col;
        }
        self.snap_cursor_to_viewport();
    }

    /// Jump to the next build error.
    pub fn build_next_error(&mut self) {
        if self.quickfix_results.is_empty() {
            self.set_status_msg("No build errors", MessageKind::Info);
            return;
        }
        if self.quickfix_index < self.quickfix_results.len() - 1 {
            self.quickfix_index += 1;
        } else {
            self.quickfix_index = 0; // wrap
        }
        self.quickfix_goto_build();
    }

    /// Jump to the previous build error.
    pub fn build_prev_error(&mut self) {
        if self.quickfix_results.is_empty() {
            self.set_status_msg("No build errors", MessageKind::Info);
            return;
        }
        if self.quickfix_index > 0 {
            self.quickfix_index -= 1;
        } else {
            self.quickfix_index = self.quickfix_results.len() - 1; // wrap
        }
        self.quickfix_goto_build();
    }

    /// Close the build buffer and return to a normal code buffer.
    pub fn build_close(&mut self) {
        let build_id = self.build.buffer_id;
        let is_build = self.buf().kind == BufferKind::Build;

        if is_build {
            // Switch to a normal buffer first
            let normal_id = self
                .buffers
                .iter()
                .find(|b| b.kind == BufferKind::Normal && b.filename.is_some())
                .map(|b| b.id);

            if let Some(nid) = normal_id {
                self.switch_window_to_buffer(nid);
            }
        }

        // Optionally remove the build buffer entirely
        if let Some(bid) = build_id {
            // Don't close if it's the current buffer (user might want to keep it)
            if self.buf().id != bid {
                self.close_buffer_by_id(bid);
                self.build.buffer_id = None;
            }
        }

        self.set_status_msg("Build buffer closed", MessageKind::Info);
    }

    /// Yank the snippet from the current build error line.
    pub fn build_yank_snippet(&mut self) {
        let (_, line_text) = match self.current_buffer_line() {
            Some(pair) => pair,
            None => return,
        };

        let snippet = match extract_snippet(&line_text) {
            Some(content) => content,
            None => {
                self.set_status_msg(
                    "No snippet ({ }, ` `, or | code) found on this line",
                    MessageKind::Info,
                );
                return;
            }
        };

        if snippet.is_empty() {
            self.set_status_msg("Snippet is empty", MessageKind::Info);
            return;
        }

        self.clipboard = Some(snippet.clone());
        self.set_status_msg(
            &format!("Yanked snippet: {}...", &snippet[..snippet.len().min(40)]),
            MessageKind::Info,
        );
    }

    /// Yank brace content from the current build error line.
    pub fn build_yank_brace_content(&mut self) {
        let (_, line_text) = match self.current_buffer_line() {
            Some(pair) => pair,
            None => return,
        };

        let brace_content = match extract_brace_content(&line_text) {
            Some(content) => content,
            None => {
                self.set_status_msg("No { } found on this line", MessageKind::Info);
                return;
            }
        };

        if brace_content.is_empty() {
            self.set_status_msg("{ } is empty on this line", MessageKind::Info);
            return;
        }

        self.clipboard = Some(brace_content.clone());
        self.set_status_msg(
            &format!(
                "Yanked brace content: {}...",
                &brace_content[..brace_content.len().min(40)]
            ),
            MessageKind::Info,
        );
    }

    /// Jump to the current quickfix result (build variant).
    fn quickfix_goto_build(&mut self) {
        let result = match self.quickfix_results.get(self.quickfix_index) {
            Some(r) => r.clone(),
            None => return,
        };

        let path_str = result.file_path.to_string_lossy().to_string();
        self.open_buffer(Some(path_str));

        let max_line = self.buf().len_lines().saturating_sub(1);
        {
            let win = self.active_window_mut();
            win.row = result.line_number.saturating_sub(1).min(max_line);
            win.col = 0;
            win.desired_col = 0;
        }

        self.snap_cursor_to_viewport();

        self.set_status_msg(
            &format!(
                "Quickfix {}/{}: {}:{}",
                self.quickfix_index + 1,
                self.quickfix_results.len(),
                result.file_path.display(),
                result.line_number,
            ),
            MessageKind::Info,
        );
    }

    /// Poll the build background thread and update the animation.
    /// Call from the main tick loop.
    pub fn tick_build(&mut self) {
        if self.build.in_progress {
            // Update spinner
            self.build.spinner_idx = (self.build.spinner_idx + 1) % SPINNER_CHARS.len();
            let elapsed = self
                .build
                .start_time
                .map(|t| t.elapsed().as_secs_f32())
                .unwrap_or(0.0);

            let spinner = SPINNER_CHARS[self.build.spinner_idx];
            self.set_status_msg(
                &format!("{} Building ({:.1}s)", spinner, elapsed),
                MessageKind::Info,
            );

            // Update build buffer header with animation
            if let Some(id) = self.build.buffer_id {
                if let Some(buf) = self.buf_mut_by_id(id) {
                    let header = format!("  [BUILD] cargo build — Building... {:.1}s {}\n  {}\n\n  (compiling, please wait...)\n", elapsed, spinner, "─".repeat(40));
                    buf.rope = Rope::from(header.as_str());
                }
            }
        }

        // Check if the background thread finished
        let result = self
            .build
            .response_rx
            .as_ref()
            .and_then(|rx| rx.try_recv().ok());

        if let Some(result) = result {
            self.build.in_progress = false;
            self.build.start_time = None;
            self.build.spinner_idx = 0;

            let full_output = format!("{}{}", result.stdout, result.stderr);
            let project_root = self.find_cargo_root();
            let diagnostics = parse_cargo_output(&full_output, &project_root);

            // Populate quickfix list
            self.quickfix_results = diagnostics
                .iter()
                .filter(|d| d.line_number > 0)
                .map(|d| crate::ed::ripgrep::RipgrepResult {
                    file_path: d.file_path.clone(),
                    line_number: d.line_number,
                    line_text: d.message.clone(),
                })
                .collect();
            self.quickfix_index = 0;
            self.build.diagnostics = diagnostics;

            // Format final buffer text
            let buffer_text = format_build_buffer(&full_output, &self.build.diagnostics);
            let build_id = self.ensure_build_buffer(&buffer_text);
            self.build.buffer_id = Some(build_id);

            // Parse syntax for the build buffer (helps with highlighting)
            if let Some(buf) = self.buf_mut_by_id(build_id) {
                buf.parse_syntax();
            }

            if result.success {
                self.set_status_msg("Build succeeded ✓", MessageKind::Success);
            } else {
                let errors = self
                    .build
                    .diagnostics
                    .iter()
                    .filter(|d| d.severity == BuildSeverity::Error)
                    .count();
                let warns = self
                    .build
                    .diagnostics
                    .iter()
                    .filter(|d| d.severity == BuildSeverity::Warning)
                    .count();
                self.set_status_msg(
                    &format!("Build failed: {} error(s), {} warning(s)", errors, warns),
                    MessageKind::Error,
                );
            }
        }
    }
}

// ═══════════════════════════════════════════════════════════════════
// Editor helper methods
// ═══════════════════════════════════════════════════════════════════

impl Editor {
    /// Create or reuse a Build buffer with the given content.
    fn ensure_build_buffer(&mut self, content: &str) -> usize {
        let existing_id = self
            .buffers
            .iter()
            .find(|b| b.kind == BufferKind::Build)
            .map(|b| b.id);

        if let Some(id) = existing_id {
            if let Some(buf) = self.buf_mut_by_id(id) {
                buf.rope = Rope::from(content);
                buf.modified = false;
            }
            return id;
        }

        let id = self.next_buf_id;
        self.next_buf_id += 1;

        let buf = crate::ed::buffer::Buffer {
            id,
            rope: Rope::from(content),
            filename: Some("*build*".to_string()),
            modified: false,
            undo_stack: Vec::new(),
            redo_stack: Vec::new(),
            diagnostics: Vec::new(),
            syntax: SyntaxState::new(),
            bookmarks: std::collections::HashSet::new(),
            git_diffs: std::collections::HashMap::new(),
            named_bookmarks: std::collections::HashMap::new(),
            kind: BufferKind::Build,
            git_log_state: None,
            git_status_state: None,
            ripgrep_results: Vec::new(),
            ripgrep_line_map: Vec::new(),
            search_pattern: None,
            diff_alignment: None,
            llm_lock_line: 0,
            tab_size: 4,
            wgrep_mode: false,
            wgrep_prefix_lens: Vec::new(),
            wgrep_original_texts: Vec::new(),
        };

        self.buffers.push(buf);
        id
    }

    /// Get (line_index, line_text) of the cursor line in the active buffer.
    pub fn current_buffer_line(&self) -> Option<(usize, String)> {
        let win = self.active_window();
        let buf = self.buf();
        let line_idx = win.row;
        if line_idx >= buf.len_lines() {
            return None;
        }
        let line_text = buf
            .line_text(line_idx)
            .trim_end_matches('\n')
            .trim_end_matches('\r')
            .to_string();
        Some((line_idx, line_text))
    }

    /// Find the best directory to run `cargo build` from.
    pub fn find_cargo_root(&self) -> PathBuf {
        let start_path = self
            .active_filename()
            .map(|s| s.to_string())
            .unwrap_or_else(|| ".".to_string());

        let start = PathBuf::from(&start_path);

        // Canonicalize
        let absolute = if start.is_relative() {
            std::env::current_dir()
                .ok()
                .and_then(|cwd| cwd.join(&start).canonicalize().ok())
                .unwrap_or(start.clone())
        } else {
            start.clone()
        };

        let start_dir = if absolute.is_file() {
            absolute
                .parent()
                .map(|p| p.to_path_buf())
                .unwrap_or(absolute.clone())
        } else {
            absolute
        };

        // 1. Walk upward for Cargo.toml
        let mut current = start_dir.clone();
        loop {
            if current.join("Cargo.toml").exists() {
                return current;
            }
            match current.parent() {
                Some(p) => current = p.to_path_buf(),
                None => break,
            }
        }

        // 2. Fallback: git root
        if let Some(git_root) = crate::git::gutter::find_git_root(&start_dir) {
            if git_root.join("Cargo.toml").exists() {
                return git_root;
            }
        }

        // 3. Ultimate fallback
        start_dir
    }

    /// Try to parse a location from a build output line.
    pub fn parse_location_from_line(&self, line: &str) -> Option<BuildDiagnostic> {
        let trimmed = line.trim();

        // 1. Short format: `path:line:col: severity[: code]: message`
        if let Some((idx, len, severity)) = find_severity_marker(trimmed) {
            let location_str = trimmed[..idx].trim();
            let message = trimmed[idx + len..].trim().to_string();
            return parse_path_line_col(location_str).map(|mut diag| {
                diag.severity = severity;
                diag.message = message;
                diag
            });
        }

        // 2. Human format: `  --> path:line:col`
        if let Some(idx) = trimmed.find("-->") {
            let location_str = trimmed[idx + 3..].trim();
            return parse_path_line_col(location_str);
        }

        None
    }
}

// ═══════════════════════════════════════════════════════════════════
// Parsing helpers (free functions)
// ═══════════════════════════════════════════════════════════════════

fn find_severity_marker(line: &str) -> Option<(usize, usize, BuildSeverity)> {
    let mut best: Option<(usize, usize, BuildSeverity)> = None;

    for (label, severity) in [
        ("error", BuildSeverity::Error),
        ("warning", BuildSeverity::Warning),
        ("note", BuildSeverity::Note),
    ] {
        let marker = format!(": {}", label);
        if let Some(pos) = line.find(&marker) {
            let after = pos + marker.len();
            let rest = line.get(after..).unwrap_or("");

            let marker_len = if rest.starts_with(": ") {
                Some(marker.len() + 2)
            } else if rest.starts_with('[') {
                rest.find("]: ").map(|be| marker.len() + be + 3)
            } else {
                None
            };

            if let Some(len) = marker_len {
                if best.as_ref().map_or(true, |(bp, _, _)| pos < *bp) {
                    best = Some((pos, len, severity));
                }
            }
        }
    }

    best
}

fn parse_path_line_col(s: &str) -> Option<BuildDiagnostic> {
    let s = s.trim_end_matches(|c: char| c.is_whitespace() || c == ':');

    let mut parts = s.rsplitn(3, ':');
    let col: usize = parts.next()?.parse().ok()?;
    let line: usize = parts.next()?.parse().ok()?;
    let path_str = parts.next()?;

    if path_str.is_empty() {
        return None;
    }

    Some(BuildDiagnostic {
        file_path: PathBuf::from(path_str),
        line_number: line,
        column: col,
        severity: BuildSeverity::Error,
        message: String::new(),
    })
}

fn parse_cargo_output(output: &str, project_root: &Path) -> Vec<BuildDiagnostic> {
    let mut diagnostics = Vec::new();

    for line in output.lines() {
        let trimmed = line.trim();

        // Short format
        if let Some((idx, len, severity)) = find_severity_marker(trimmed) {
            let location_str = trimmed[..idx].trim();
            let message = trimmed[idx + len..].trim();

            if let Some(mut diag) = parse_path_line_col(location_str) {
                if diag.file_path.is_relative() {
                    diag.file_path = project_root.join(&diag.file_path);
                }
                diag.message = message.to_string();
                diag.severity = severity;
                diagnostics.push(diag);
                continue;
            }
        }

        // Human format: `  --> path:line:col`
        if let Some(idx) = trimmed.find("-->") {
            let location_str = trimmed[idx + 3..].trim();
            if let Some(mut diag) = parse_path_line_col(location_str) {
                if diag.file_path.is_relative() {
                    diag.file_path = project_root.join(&diag.file_path);
                }
                diag.severity = BuildSeverity::Error;
                diagnostics.push(diag);
            }
        }
    }

    diagnostics
}

fn extract_brace_content(line: &str) -> Option<String> {
    let close = line.rfind('}')?;
    let open = line[..close].rfind('{')?;
    Some(line[open + 1..close].trim().to_string())
}

fn extract_snippet(line: &str) -> Option<String> {
    // 1. Last { } pair
    if let Some(close) = line.rfind('}') {
        if let Some(open) = line[..close].rfind('{') {
            let content = line[open + 1..close].trim().to_string();
            if !content.is_empty() {
                return Some(content);
            }
        }
    }

    // 2. Last ` ` pair (backticks)
    if let Some(close) = line.rfind('`') {
        if let Some(open) = line[..close].rfind('`') {
            if close > open + 1 {
                let content = line[open + 1..close].trim().to_string();
                if !content.is_empty() {
                    return Some(content);
                }
            }
        }
    }

    // 3. Code from source line (`| code`)
    if let Some(pipe_pos) = line.find("| ") {
        let code = line[pipe_pos + 2..].trim();
        if !code.is_empty()
            && !code.starts_with('^')
            && !code.starts_with('~')
            && !code.starts_with('-')
            && !code.starts_with('|')
        {
            return Some(code.to_string());
        }
    }

    None
}

// ═══════════════════════════════════════════════════════════════════
// Buffer formatting
// ═══════════════════════════════════════════════════════════════════

fn format_build_buffer(raw_output: &str, diagnostics: &[BuildDiagnostic]) -> String {
    let errors = diagnostics
        .iter()
        .filter(|d| d.severity == BuildSeverity::Error)
        .count();
    let warns = diagnostics
        .iter()
        .filter(|d| d.severity == BuildSeverity::Warning)
        .count();

    let mut buf = format!(
        "  [BUILD] cargo build --release — {} error(s), {} warning(s)\n",
        errors, warns
    );
    buf.push_str(&format!("  {}\n\n", "─".repeat(40)));

    // Raw compiler output
    buf.push_str(&format!(
        "  {} Raw Output {}\n",
        "─".repeat(14),
        "─".repeat(14)
    ));
    if raw_output.trim().is_empty() {
        buf.push_str("  (no output)\n");
    } else {
        buf.push_str(raw_output);
        if !raw_output.ends_with('\n') {
            buf.push('\n');
        }
    }

    // Keybindings hint
    buf.push('\n');
    buf.push_str(&format!("  {}\n", "─".repeat(40)));
    buf.push_str("  [Enter] goto error  [r] rebuild  [q] close\n");

    buf
}
