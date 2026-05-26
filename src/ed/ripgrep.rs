//--+ ./ed/ripgrep.rs
// src/ed/ripgrep.rs
//! Ripgrep integration: searching, result navigation, and result buffer management.

use std::fs;
use std::path::{Path, PathBuf};

use crate::ed::buffer::{Buffer, BufferKind};
use crate::ed::editor::Editor;
use crate::ed::mode::MessageKind;
use crate::git::gutter::find_git_root;

// ---------------------------------------------------------------------------
// Self-contained Ripgrep Runner and Models
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct RipgrepResult {
    pub file_path: PathBuf, // Saved as an absolute path for persistent reuse
    pub line_number: usize,
    pub line_text: String,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct RipgrepOutput {
    pub pattern: String,
    pub root_dir: PathBuf,
    pub results: Vec<RipgrepResult>,
}

impl RipgrepOutput {
    /// Formats search results to be displayed within the dedicated *ripgrep* buffer.
    /// File headers are shown as relative paths against `root_dir` for clean rendering.
    pub fn format_for_buffer(&self) -> String {
        let mut formatted = String::new();
        formatted.push_str(&format!(
            "  [RG] Search results for '{}' in {}\n",
            self.pattern,
            self.root_dir.display()
        ));
        formatted.push_str("  ───\n\n");

        let mut current_file = None;
        for result in &self.results {
            if Some(&result.file_path) != current_file {
                current_file = Some(&result.file_path);

                // Keep the buffer display clean by stripping the absolute path prefix
                let display_path = result
                    .file_path
                    .strip_prefix(&self.root_dir)
                    .unwrap_or(&result.file_path);

                formatted.push_str(&format!("{}:\n", display_path.display()));
            }
            formatted.push_str(&format!("{}: {}\n", result.line_number, result.line_text));
        }
        formatted
    }

    /// Maps each buffer line index back to a specific Ripgrep result index, skipping headers.
    pub fn build_line_map(&self) -> Vec<Option<usize>> {
        let mut line_map = Vec::new();
        // Line 0:   [RG] Search results for... -> None
        line_map.push(None);
        // Line 1:   ─── -> None
        line_map.push(None);
        // Line 2: (empty space) -> None
        line_map.push(None);

        let mut current_file = None;
        for (idx, result) in self.results.iter().enumerate() {
            if Some(&result.file_path) != current_file {
                current_file = Some(&result.file_path);
                // File path header line -> None
                line_map.push(None);
            }
            // Actual matching result line -> Some(idx)
            line_map.push(Some(idx));
        }
        line_map
    }
}

/// Escapes regular expression special characters to perform literal search matches.
pub fn escape_regex(pattern: &str) -> String {
    let mut escaped = String::new();
    for c in pattern.chars() {
        match c {
            '\\' | '.' | '+' | '*' | '?' | '(' | ')' | '|' | '[' | ']' | '{' | '}' | '^' | '$' => {
                escaped.push('\\');
                escaped.push(c);
            }
            _ => escaped.push(c),
        }
    }
    escaped
}

/// Extracts the identifier word beneath a specific visual cursor index.
pub fn word_under_cursor(line: &str, col: usize) -> String {
    let chars: Vec<char> = line.chars().collect();
    if chars.is_empty() {
        return String::new();
    }
    let col = col.min(chars.len().saturating_sub(1));

    let mut start = col;
    while start > 0 && (chars[start - 1].is_alphanumeric() || chars[start - 1] == '_') {
        start -= 1;
    }

    let mut end = col;
    while end < chars.len() && (chars[end].is_alphanumeric() || chars[end] == '_') {
        end += 1;
    }

    if start < end {
        chars[start..end].iter().collect()
    } else {
        String::new()
    }
}

/// Spawns the system `rg` executable, parsing standard `--vimgrep` lines and converting
/// file paths to absolute coordinates relative to the search root.
/// Spawns the system `rg` executable, parsing standard `--vimgrep` lines and converting
/// file paths to absolute coordinates relative to the search root.
pub fn run_ripgrep(pattern: &str, root_dir: &Path) -> Result<RipgrepOutput, String> {
    use std::process::Command;

    // Resolve the root directory to an absolute path without resolving symlinks
    let abs_root_dir = if root_dir.is_absolute() {
        root_dir.to_path_buf()
    } else {
        std::env::current_dir()
            .unwrap_or_else(|_| PathBuf::from("."))
            .join(root_dir)
    };

    let output = Command::new("rg")
        .arg("--vimgrep")
        .arg("--color=never")
        .arg(pattern)
        .current_dir(&abs_root_dir)
        .output()
        .map_err(|e| format!("Failed to execute rg: {}", e))?;

    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut results = Vec::new();

    for line in stdout.lines() {
        let parts: Vec<&str> = line.split(':').collect();
        if parts.len() < 4 {
            continue;
        }

        // Handle potential Windows-style absolute paths (e.g., C:\...)
        let has_drive_letter = parts[0].len() == 1
            && parts[0].chars().next().unwrap().is_ascii_alphabetic()
            && (parts[1].starts_with('\\') || parts[1].starts_with('/'));

        let (file_path_parts, line_num_str, _col_num_str, text_start_idx) = if has_drive_letter {
            if parts.len() < 5 {
                continue;
            }
            (&parts[0..2], parts[2], parts[3], 4)
        } else {
            (&parts[0..1], parts[1], parts[2], 3)
        };

        let file_path_str = file_path_parts.join(":");
        let raw_path = PathBuf::from(file_path_str);

        // Convert any relative match paths into stable absolute paths
        let abs_file_path = if raw_path.is_absolute() {
            raw_path
        } else {
            abs_root_dir.join(raw_path)
        };

        if let Ok(line_number) = line_num_str.parse::<usize>() {
            let line_text = parts[text_start_idx..].join(":");
            results.push(RipgrepResult {
                file_path: abs_file_path,
                line_number,
                line_text,
            });
        }
    }

    Ok(RipgrepOutput {
        pattern: pattern.to_string(),
        root_dir: abs_root_dir,
        results,
    })
}

// ---------------------------------------------------------------------------
// Editor Implementation & Buffer State Actions
// ---------------------------------------------------------------------------

/// File name for persisted last rg output.
const LAST_RG_FILE: &str = "last_rg.json";

/// Save the last ripgrep output to disk.
fn save_last_rg_output(output: &RipgrepOutput) -> Result<(), String> {
    let config_dir = crate::config::app_config::Config::config_dir().map_err(|e| e.to_string())?;
    fs::create_dir_all(&config_dir).map_err(|e| format!("Cannot create config dir: {}", e))?;
    let path = config_dir.join(LAST_RG_FILE);
    let data = serde_json::to_vec(output).map_err(|e| format!("Serialize error: {}", e))?;
    fs::write(&path, data).map_err(|e| format!("Write error: {}", e))?;
    Ok(())
}

/// Load the last ripgrep output from disk.
fn load_last_rg_output() -> Option<RipgrepOutput> {
    let config_dir = crate::config::app_config::Config::config_dir().ok()?;
    let path = config_dir.join(LAST_RG_FILE);
    let data = fs::read(&path).ok()?;
    serde_json::from_slice(&data).ok()
}

impl Editor {
    /// Search for the word under the cursor using ripgrep.
    pub fn ripgrep_under_cursor(&mut self) {
        let is_rg_buffer = self.buf().kind == BufferKind::Ripgrep;
        if is_rg_buffer {
            self.ripgrep_close_buffer();
        }

        let line_text = self.get_current_line_text();
        let col = self.active_col();
        let pattern = word_under_cursor(&line_text, col);

        if pattern.is_empty() {
            self.set_status_msg("No word under cursor", MessageKind::Error);
            return;
        }

        let root_dir = self
            .buf()
            .filename
            .as_ref()
            .and_then(|f| find_git_root(Path::new(f)))
            .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));

        self.ripgrep_search_internal(&pattern, &root_dir);
    }

    /// Search for a specific pattern using ripgrep.
    pub fn ripgrep_search(&mut self, pattern: &str) {
        if pattern.is_empty() {
            self.set_status_msg("Empty search pattern", MessageKind::Error);
            return;
        }

        let root_dir = self
            .buf()
            .filename
            .as_ref()
            .and_then(|f| find_git_root(Path::new(f)))
            .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));

        self.ripgrep_search_internal(pattern, &root_dir);
    }

    fn ripgrep_search_internal(&mut self, pattern: &str, root_dir: &Path) {
        self.set_status_msg(
            &format!("Searching for '{}'...", pattern),
            MessageKind::Info,
        );

        let escaped = escape_regex(pattern);
        match run_ripgrep(&escaped, root_dir) {
            Ok(output) => {
                let output: RipgrepOutput = output;
                self.last_rg_pattern = Some(pattern.to_string());
                self.last_rg_root_dir = Some(root_dir.to_path_buf());
                self.last_rg_output = Some(output.clone());
                let _ = save_last_rg_output(&output);

                self.populate_ripgrep_buffer(pattern, output);
            }
            Err(e) => {
                self.set_status_msg(&format!("Ripgrep failed: {}", e), MessageKind::Error);
            }
        }
    }

    /// Reopen the last ripgrep results buffer (no re-search).
    pub fn ripgrep_last(&mut self) {
        let existing_rg = self
            .buffers
            .iter()
            .find(|b| b.kind == BufferKind::Ripgrep)
            .map(|b| b.id);

        if let Some(rg_id) = existing_rg {
            self.switch_window_to_buffer(rg_id);
            return;
        }

        // Deserialize last state if none exists in cache
        if self.last_rg_output.is_none() {
            self.load_last_rg_state();
        }

        if let Some(rg_output) = self.last_rg_output.clone() {
            let pattern = self.last_rg_pattern.clone().unwrap_or_default();
            self.populate_ripgrep_buffer(&pattern, rg_output);
        } else {
            self.set_status_msg("No previous ripgrep search", MessageKind::Error);
        }
    }

    /// Re-run the last ripgrep search.
    pub fn ripgrep_last_rerun(&mut self) {
        if self.last_rg_pattern.is_none() || self.last_rg_root_dir.is_none() {
            self.load_last_rg_state();
        }

        let (pattern, root_dir) = match (&self.last_rg_pattern, &self.last_rg_root_dir) {
            (Some(p), Some(d)) => (p.clone(), d.clone()),
            _ => {
                self.set_status_msg("No previous ripgrep search", MessageKind::Error);
                return;
            }
        };

        self.ripgrep_search_internal(&pattern, &root_dir);
    }

    /// Jump to the result under the cursor in a ripgrep buffer.
    pub fn ripgrep_goto_result(&mut self) {
        let row = self.active_window().row;
        let buf = self.buf();

        if buf.kind != BufferKind::Ripgrep {
            return;
        }

        let result_idx = match buf.ripgrep_line_map.get(row) {
            Some(Some(idx)) => *idx,
            _ => {
                self.set_status_msg(
                    "Cursor is on a header line — move to a match line and press Enter.",
                    MessageKind::Info,
                );
                return;
            }
        };

        let result = match buf.ripgrep_results.get(result_idx) {
            Some(r) => r,
            None => return,
        };

        let file_path = result.file_path.clone();
        let line_number = result.line_number;

        self.open_file_at_line(&file_path, line_number);
    }

    /// Close the ripgrep buffer if the active window is viewing one.
    pub fn ripgrep_close_buffer(&mut self) {
        if self.buf().kind != BufferKind::Ripgrep {
            self.set_status_msg("Not in a ripgrep buffer", MessageKind::Error);
            return;
        }

        let target_id = self
            .buffers
            .iter()
            .find(|b| b.kind == BufferKind::Normal && b.filename.is_some())
            .or_else(|| self.buffers.iter().find(|b| b.kind == BufferKind::Normal))
            .map(|b| b.id);

        let target_id = match target_id {
            Some(id) => id,
            None => {
                self.set_status_msg("Created new buffer", MessageKind::Info);
                self.buffers.first().map(|b| b.id).unwrap_or(0)
            }
        };

        self.switch_window_to_buffer(target_id);
    }

    /// Open a file at a specific 1‑based line number.
    pub fn open_file_at_line(&mut self, path: &Path, line: usize) {
        self.active_window_mut().save_jump_position();
        let path_str = path.to_string_lossy().to_string();
        self.open_buffer(Some(path_str));

        let (win, buf) = self.active_window_and_buf_mut();
        win.row = line
            .saturating_sub(1)
            .min(buf.len_lines().saturating_sub(1));
        win.col = 0;
        win.desired_col = 0;
        self.scroll_active_window_to_cursor();
    }

    pub(crate) fn populate_ripgrep_buffer(&mut self, pattern: &str, rg_output: RipgrepOutput) {
        let existing_rg = self
            .buffers
            .iter()
            .find(|b| b.kind == BufferKind::Ripgrep)
            .map(|b| b.id);

        let rg_buffer_id = if let Some(id) = existing_rg {
            id
        } else {
            let id = self.next_buf_id;
            self.next_buf_id += 1;

            let buf = Buffer {
                id,
                rope: ropey::Rope::from_str(""),
                filename: Some("*ripgrep*".to_string()),
                modified: false,
                undo_stack: Vec::new(),
                syntax: crate::ed::syntax::SyntaxState::new(),
                bookmarks: std::collections::HashSet::new(),
                git_diffs: std::collections::HashMap::new(),
                named_bookmarks: std::collections::HashMap::new(),
                kind: BufferKind::Ripgrep,
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

        if let Some(buffer) = self.buf_mut_by_id(rg_buffer_id) {
            let formatted = rg_output.format_for_buffer();
            buffer.rope = ropey::Rope::from_str(&formatted);
            buffer.ripgrep_results = rg_output.results.clone();
            buffer.ripgrep_line_map = rg_output.build_line_map();
            buffer.search_pattern = Some(pattern.to_string());
            buffer.parse_syntax();
        }

        // Assign to quickfix fields outside the buffer mutable borrow scope
        self.quickfix_results = rg_output.results.clone();
        self.quickfix_index = 0;

        // Cache in memory and persist on local disk
        self.last_rg_pattern = Some(pattern.to_string());
        self.last_rg_root_dir = Some(rg_output.root_dir.clone());
        self.last_rg_output = Some(rg_output.clone());
        let _ = save_last_rg_output(&rg_output);

        self.switch_window_to_buffer(rg_buffer_id);

        let count = rg_output.results.len();
        let file_count = rg_output
            .results
            .iter()
            .map(|r| r.file_path.clone())
            .collect::<std::collections::HashSet<_>>()
            .len();

        if count == 0 {
            self.set_status_msg(
                &format!(
                    "No matches for '{}' in {}",
                    pattern,
                    rg_output.root_dir.display()
                ),
                MessageKind::Info,
            );
        } else {
            self.set_status_msg(
                &format!(
                    "[RG] '{}' — {} match{} in {} file{}  (Enter=jump, q=close)",
                    pattern,
                    count,
                    if count == 1 { "" } else { "es" },
                    file_count,
                    if file_count == 1 { "" } else { "s" },
                ),
                MessageKind::Info,
            );
        }
    }

    fn load_last_rg_state(&mut self) {
        if let Some(mut output) = load_last_rg_output() {
            // Resolve the loaded root directory to an absolute path without resolving symlinks
            let abs_root = if output.root_dir.is_absolute() {
                output.root_dir.clone()
            } else {
                std::env::current_dir()
                    .unwrap_or_else(|_| PathBuf::from("."))
                    .join(&output.root_dir)
            };
            output.root_dir = abs_root.clone();

            // Refill / resolve any relative paths into fully qualified absolute coordinates
            for result in &mut output.results {
                if result.file_path.is_relative() {
                    result.file_path = abs_root.join(&result.file_path);
                }
            }

            self.last_rg_pattern = Some(output.pattern.clone());
            self.last_rg_root_dir = Some(output.root_dir.clone());
            self.last_rg_output = Some(output);
        }
    }

    // ── Ripgrep result navigation ─────────────────────────
    pub fn ripgrep_next_result(&mut self) {
        self.quickfix_next();
    }

    pub fn ripgrep_prev_result(&mut self) {
        self.quickfix_prev();
    }

    pub fn quickfix_next(&mut self) {
        if self.quickfix_results.is_empty() {
            self.set_status_msg("No ripgrep results. Run :rg first.", MessageKind::Error);
            return;
        }

        if self.quickfix_index + 1 >= self.quickfix_results.len() {
            self.set_status_msg("Already at last result", MessageKind::Info);
            return;
        }

        self.quickfix_index += 1;
        let result = self.quickfix_results[self.quickfix_index].clone();
        self.open_file_at_line(&result.file_path, result.line_number);

        let display_name = self.buf().display_name();
        self.set_status_msg(
            &format!(
                "Result {}/{}: {}:{}",
                self.quickfix_index + 1,
                self.quickfix_results.len(),
                display_name,
                result.line_number
            ),
            MessageKind::Info,
        );
    }

    pub fn quickfix_prev(&mut self) {
        if self.quickfix_results.is_empty() {
            self.set_status_msg("No ripgrep results. Run :rg first.", MessageKind::Error);
            return;
        }

        if self.quickfix_index == 0 {
            self.set_status_msg("Already at first result", MessageKind::Info);
            return;
        }

        self.quickfix_index -= 1;
        let result = self.quickfix_results[self.quickfix_index].clone();
        self.open_file_at_line(&result.file_path, result.line_number);

        let display_name = self.buf().display_name();
        self.set_status_msg(
            &format!(
                "Result {}/{}: {}:{}",
                self.quickfix_index + 1,
                self.quickfix_results.len(),
                display_name,
                result.line_number
            ),
            MessageKind::Info,
        );
    }
}
