//! Completion system integration and LSP communication for the Editor.
//!
//! Handles:
//! - Polling the completion state machine (Codeium / Copilot / LSP / Manual)
//! - Accepting ghost text or popup selections
//! - Triggering manual completions (Alt+/)
//! - LSP completion request throttling and response handling
//! - Vocabulary and buffer word indexing
//! - Copilot authentication and completion polling
//! - LSP file lifecycle notifications (open/close/change/save)
//! - LSP incremental and full-sync change notifications
//! - LSP response polling and dispatch (diagnostics, format, inlay, goto)

use crate::comp::state::CompletionSource;
use crate::config::app_config::Config;
use crate::ed::handle::tag::LSP_GOTO_TIMEOUT_MS;
use crate::ed::mode::MessageKind;
use crate::ed::mode::Mode;
use crate::ed::Editor;
use crate::lsp::{
    path_to_uri, uri_to_path, CompletionItem, FormattingOptions, InlayHint, Location, LspMessage,
    TextEdit,
};
use crate::msgbox::AppMessage;

// ═══════════════════════════════════════════════════════════════════════════
// Completion response handling
// ═══════════════════════════════════════════════════════════════════════════

impl Editor {
    /// Ingest a completion response from an external source (LSP, Codeium, etc.).
    pub fn ingest_completion_response(&mut self, _id: usize, items: Vec<String>, version: u64) {
        self.comp.on_response(items, version);
    }

    /// Notify the completion system that an edit occurred.
    pub fn on_edit(&mut self) {
        self.comp.on_edit();
    }

    /// Cycle through the completion popup in the given direction (+1 or -1).
    pub fn cycle_completion(&mut self, dir: i32) {
        self.comp.cycle(dir);
        // Ensure the popup is visible after cycling
        self.update_completion_popup_state();
    }

    /// Clear all active completions (e.g. on cursor movement).
    pub fn clear_completions(&mut self) {
        // self.comp.on_edit();
        self.comp.reset_to_idle();
    }

    /// Cancel a pending completion request by ID.
    pub fn cancel_pending_request(&mut self) {
        let id = self.comp.request_id;
        self.comp.on_cancel(id);
    }

    /// Touch the last-edit timestamp to debounce AI requests.
    pub fn touch_completion(&mut self) {
        self.comp.last_edit_time = std::time::Instant::now();
    }

    /// Directly set the completion candidates (used by LSP).
    pub fn set_completions(&mut self, items: Vec<String>) {
        self.comp.set_active(items);
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// Completion polling (Codeium / Copilot)
// ═══════════════════════════════════════════════════════════════════════════

impl Editor {
    /// Poll the completion machine on every tick.
    ///
    /// Returns `Some((id, text, offset, lang, version))` when a Codeium
    /// request should be sent by the caller.
    ///
    /// Also fires Copilot inline when the AI idle timer has elapsed, so both
    /// AI backends share the same 500 ms debounce.
    pub fn poll_completion(&mut self) -> Option<(usize, String, usize, String, u64)> {
        let mode = self.mode;
        if mode != Mode::Insert && mode != Mode::Brief {
            return None;
        }

        let (row, col) = {
            let win = self.active_window();
            (win.row, win.col)
        };

        // ── Guard: buffer must have content ─────────────────────────────
        {
            let buf = self.buf();
            if buf.rope.len_chars() <= 1 || row >= buf.len_lines() {
                return None;
            }
        }

        // ── Copilot idle-debounce fire ───────────────────────────────────
        if self.config.copilot_enabled && self.comp.should_fire_ai_request(mode) {
            if let Some(ref handle) = self.copilot_handle {
                if handle.ready.load(std::sync::atomic::Ordering::Relaxed) {
                    let (_, version) = self.comp.start_source_request(CompletionSource::Copilot);

                    let text = self.buf().rope.to_string();
                    let offset = self.buf().rope.line_to_char(row) + col;
                    let lang =
                        crate::ed::buffer::detect_language(self.active_filename().as_deref());

                    let req = crate::ai::copilot::server::CopilotRequest::Completion {
                        text,
                        offset,
                        language: lang,
                        version,
                    };

                    if handle.request_tx.send(req).is_err() {
                        log::debug!("Copilot request channel closed");
                        self.comp
                            .merge_source(CompletionSource::Copilot, Vec::new(), version);
                    } else {
                        self.comp.mark_ai_request_fired();
                    }
                }
            }
        }

        // ── Codeium idle-debounce fire (via maybe_take_request) ──────────
        if self.config.codeium_enabled && self.comp.should_fire_ai_request(mode) {
            let _ = self.comp.start_source_request(CompletionSource::Codeium);
        }

        // ── Codeium tick-poll (may return a request to caller) ───────────
        let (rope_text, rope_len_chars, line_text, cursor_char_offset, filename) = {
            let buf = self.buf();
            (
                buf.rope.to_string(),
                buf.rope.len_chars(),
                buf.line_text(row),
                buf.rope.line_to_char(row) + col,
                buf.filename.clone(),
            )
        };

        self.comp.maybe_take_request(
            rope_text,
            rope_len_chars,
            line_text,
            cursor_char_offset,
            filename.as_deref(),
            mode,
            row,
            col,
        )
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// LSP completion throttling
// ═══════════════════════════════════════════════════════════════════════════

impl Editor {
    /// Called from the main loop tick. Sends the pending LSP completion request
    /// if `throttle_ms` have elapsed since the last edit.
    pub fn poll_completion_lsp(&mut self) {
        if !self.lsp_full_active || !self.config.lsp_completion_enabled {
            self.lsp_completion_pending = None;
            return;
        }

        let pending = match &self.lsp_completion_pending {
            Some(p) => p.clone(),
            None => return,
        };

        // Check throttle debounce
        let throttle = std::time::Duration::from_millis(self.comp.throttle_ms);
        if pending.request_time.elapsed() < throttle {
            return;
        }

        // Clear pending (we're sending now)
        self.lsp_completion_pending = None;

        // Now send the completion request
        self.lsp_request_completion(
            &pending.path,
            pending.line,
            pending.col,
            pending.comp_version,
        );
    }

    /// Ingest LSP completion results (filtered by request ID).
    pub fn ingest_lsp_completion(&mut self, request_id: usize, version: u64, items: Vec<String>) {
        let expected = self.comp.get_pending_request_id(CompletionSource::Lsp);
        match expected {
            Some(id) if id == request_id => {
                self.comp
                    .merge_source(CompletionSource::Lsp, items, version);
            }
            _ => {}
        }
    }

    /// Extract the word fragment after the last trigger character.
    /// For "person.ag" → "ag", for "person." → "", for "hello" → "hello".
    fn post_trigger_prefix(text: &str) -> &str {
        let mut trigger_end = 0usize;

        if let Some(pos) = text.rfind("::") {
            trigger_end = trigger_end.max(pos + 2);
        }
        if let Some(pos) = text.rfind("->") {
            trigger_end = trigger_end.max(pos + 2);
        }
        if let Some(pos) = text.rfind('.') {
            trigger_end = trigger_end.max(pos + 1);
        }

        if trigger_end > 0 && trigger_end <= text.len() {
            text.get(trigger_end..).unwrap_or("")
        } else {
            text
        }
    }

    /// Strips LSP snippet placeholders (e.g., `$0`, `$1`, `${1:placeholder}`).
    /// Without this, editors that don't support snippets will insert `$0` literally.
    fn strip_lsp_snippets(text: &str) -> String {
        let mut result = String::with_capacity(text.len());
        let mut chars = text.chars().peekable();
        while let Some(&c) = chars.peek() {
            if c == '$' {
                chars.next(); // consume '$'
                if chars.peek().map_or(false, |&nc| nc == '{') {
                    // ${N:...} format
                    chars.next(); // consume '{'
                    while let Some(&nc) = chars.peek() {
                        chars.next(); // consume character
                        if nc == '}' {
                            break;
                        }
                    }
                } else {
                    // $N format
                    while chars.peek().map_or(false, |&nc| nc.is_ascii_digit()) {
                        chars.next();
                    }
                }
            } else {
                result.push(c);
                chars.next();
            }
        }
        result
    }

    /// Apply LSP completion results with config-aware filtering.
    pub fn apply_lsp_completion(&mut self, items: Option<Vec<CompletionItem>>, comp_version: u64) {
        let current = self.comp.current_version();

        // Allow responses for current version or one behind
        if comp_version + 1 < current {
            log::debug!(
                "[apply_lsp_completion] Dropping stale: comp_v={}, current_v={}",
                comp_version,
                current
            );
            self.comp
                .merge_source(CompletionSource::Lsp, Vec::new(), comp_version);
            return;
        }

        // Clean up old mappings
        let cutoff = comp_version.saturating_sub(10);
        self.comp_lsp_version_map.retain(|&v, _| v >= cutoff);

        let prefix = self.comp.prefix().to_string();
        let prefix_lower = prefix.to_lowercase();
        let strict_prefix = self.config.lsp_comp_strict_prefix;

        // Check the full line context for triggers (e.g. `foo.`)
        let (row, col) = {
            let win = self.active_window();
            (win.row, win.col)
        };
        let line_before_cursor: String = self.buf().line_text(row).chars().take(col).collect();
        let has_trigger = Self::has_completion_trigger(&line_before_cursor);

        // ── LOG: Incoming results ──────────────────────────────────────
        log::debug!(
            "[apply_lsp_completion] version={}, prefix='{}', line_before='{}', has_trigger={}, strict_prefix={}, incoming_count={}",
            comp_version,
            prefix,
            line_before_cursor,
            has_trigger,
            strict_prefix,
            items.as_ref().map(|i| i.len()).unwrap_or(0)
        );

        let labels: Vec<String> = match items {
            Some(items) => items
                .into_iter()
                .filter_map(|item| {
                    let filter_key = item.filter_text.as_deref().unwrap_or(&item.label);
                    let filter_lower = filter_key.to_lowercase();

                    // Decide whether to include this completion based on config
                    let include = if prefix.is_empty() && !has_trigger {
                        !strict_prefix
                    } else if has_trigger {
                        // Trigger found (., ::, -> anywhere before cursor).
                        // Filter by the word fragment AFTER the last trigger.
                        let post_trigger = Self::post_trigger_prefix(&line_before_cursor);
                        if post_trigger.is_empty() {
                            true // Right after trigger, show all context results
                        } else {
                            let post_lower = post_trigger.to_lowercase();
                            filter_lower.starts_with(&post_lower)
                        }
                    } else if strict_prefix {
                        filter_lower.starts_with(&prefix_lower)
                    } else {
                        true
                    };

                    if include {
                        let raw_text = item.get_insert_text().unwrap_or_else(|| item.label.clone());
                        Some(Self::strip_lsp_snippets(&raw_text))
                    } else {
                        None
                    }
                })
                .collect(),
            None => Vec::new(),
        };

        // ── LOG: Filtered results ──────────────────────────────────────
        log::debug!(
            "[apply_lsp_completion] Filtered down to {} items: {:?}",
            labels.len(),
            labels.iter().take(5).collect::<Vec<_>>() // Only log first 5 to avoid spam
        );

        self.comp
            .merge_source(CompletionSource::Lsp, labels, comp_version);

        // ── Wire LSP completions to popup subsystem ────────────────────
        self.update_completion_popup_state();
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// Completion popup state management
// ═══════════════════════════════════════════════════════════════════════════

impl Editor {
    /// Called after any completion source merges items into the CompletionMachine.
    pub fn update_completion_popup_state(&mut self) {
        // Only relevant in Insert/Brief modes
        if !matches!(self.mode, Mode::Insert | Mode::Brief) {
            return;
        }

        // Don't override higher-priority popups
        if let Some(kind) = &self.popup.kind {
            if *kind != crate::popup::PopupKind::Completion {
                return;
            }
        }

        // Show popup if any candidate's source is configured as Popup
        if self.comp.should_show_popup(&self.config) {
            self.popup.kind = Some(crate::popup::PopupKind::Completion);
        } else if self.popup.kind == Some(crate::popup::PopupKind::Completion) {
            self.popup.kind = None;
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// On-completion-edit (main entry point for edit-triggered completions)
// ═══════════════════════════════════════════════════════════════════════════

impl Editor {
    /// Check if the text contains a completion trigger (., ::, ->).
    /// When true, LSP has already contextually filtered results,
    /// so we must NOT apply local prefix filtering.
    fn has_completion_trigger(text: &str) -> bool {
        // Order matters: check :: before .
        if text.contains("::") {
            return true;
        }
        if text.contains("->") {
            return true;
        }
        if text.contains('.') {
            return true;
        }
        false
    }

    /// Check if the text is EXACTLY a trigger that should force
    /// an immediate LSP request even if min_prefix isn't met.
    fn is_exact_trigger(text: &str) -> bool {
        matches!(text, "." | "::" | "->")
    }

    /// Called by the main loop on every edit while in Insert/Brief mode.
    pub fn on_completion_edit(&mut self) {
        self.comp.on_edit();

        let (row, col) = {
            let win = self.active_window();
            (win.row, win.col)
        };

        let prefix = self.get_current_word_prefix();
        let is_path_prefix = prefix.starts_with("./");

        self.comp.set_prefix(prefix.clone());

        // Immediately re-filter existing results against the new prefix,
        // so the popup doesn't flicker/dismiss while the new LSP request
        // is in flight.  E.g. typing `a` after `person.` narrows 26
        // struct members down to those starting with `a`.
        self.comp.refilter();

        // ── Calculate the full text before cursor for trigger checks ──
        let line_before_cursor: String = self.buf().line_text(row).chars().take(col).collect();
        let ends_with_trigger = line_before_cursor.ends_with('.')
            || line_before_cursor.ends_with("::")
            || line_before_cursor.ends_with("->");

        let min_word_prefix: usize = 4;

        // ── Source 1: FilePaths (synchronous, no debounce needed) ──
        if is_path_prefix && prefix.len() >= 2 {
            let (_req_id, version) = self.comp.start_source_request(CompletionSource::FilePaths);
            let matches = crate::comp::path_complete::complete_path(&prefix);
            self.comp
                .merge_source(CompletionSource::FilePaths, matches, version);
        } else {
            let (_req_id, version) = self.comp.start_source_request(CompletionSource::FilePaths);
            self.comp
                .merge_source(CompletionSource::FilePaths, Vec::new(), version);
        }

        // ── Source 2: Buffer words (synchronous) ───────────────────
        if self.config.buffer_word_scan
            && !self.buffer_words.is_empty()
            && prefix.len() >= min_word_prefix
        {
            let (_req_id, version) = self
                .comp
                .start_source_request(CompletionSource::BufferWords);
            let matches: Vec<String> = self
                .buffer_words
                .iter()
                .filter(|w| w.starts_with(&prefix) && w.as_str() != prefix)
                .take(30)
                .cloned()
                .collect();
            self.comp
                .merge_source(CompletionSource::BufferWords, matches, version);
        }

        // ── Source 3: Vocab words (synchronous) ────────────────────
        if !self.vocab_words.is_empty() && prefix.len() >= min_word_prefix {
            let (_req_id, version) = self.comp.start_source_request(CompletionSource::VocabWords);
            let matches: Vec<String> = self
                .vocab_words
                .iter()
                .filter(|w| w.starts_with(&prefix) && w.as_str() != prefix)
                .take(30)
                .cloned()
                .collect();
            self.comp
                .merge_source(CompletionSource::VocabWords, matches, version);
        }

        // ── Source 4: LSP — DEBOUNCED with config-aware trigger ────
        if self.lsp_full_active && self.config.lsp_completion_enabled {
            let filename = match self.active_filename() {
                Some(f) => f.to_string(),
                None => return,
            };

            // FIX: Use `line_before_cursor` for trigger checks instead of just `prefix`.
            // When typing `foo.`, `prefix` is `""` (word boundary stops at `.`),
            // but `line_before_cursor` is `"foo."` which correctly ends with a trigger.
            let should_trigger = {
                let has_trigger = Self::has_completion_trigger(&line_before_cursor);
                let meets_min_length = prefix.len() >= self.config.lsp_completion_min_prefix;

                let result = ends_with_trigger
                    || has_trigger
                    || meets_min_length
                    || self.config.lsp_completion_min_prefix == 0;

                // ── LOG: Trigger decision ──────────────────────────
                log::debug!(
                    "[compex:lsp_trigger] prefix='{}', line_before='{}', ends_with_trigger={}, has_trigger={}, meets_min_length={}, min_prefix={}, should_trigger={}",
                    prefix,
                    line_before_cursor,
                    ends_with_trigger,
                    has_trigger,
                    meets_min_length,
                    self.config.lsp_completion_min_prefix,
                    result
                );

                result
            };

            if should_trigger {
                let (_req_id, version) = self.comp.start_source_request(CompletionSource::Lsp);

                log::debug!(
                    "[compex:lsp_queue] Queuing LSP request at row={}, col={}, version={}",
                    row,
                    col,
                    version
                );

                self.lsp_completion_pending = Some(crate::ed::editor::LspCompletionPending {
                    comp_version: version,
                    path: std::path::PathBuf::from(filename),
                    line: row as u32,
                    col: col as u32,
                    request_time: std::time::Instant::now(),
                });
            }
        }

        // ── Wire synchronous completions to popup subsystem ──────────
        self.update_completion_popup_state();
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// Accepting completions
// ═══════════════════════════════════════════════════════════════════════════

impl Editor {
    /// Accept the current completion (ghost text or popup selection).
    /// Handles both single-line and multiline completions correctly,
    /// including LSP dot-completions where the replacement differs
    /// from the typed prefix (e.g. `ag` → `get_age()`).
    pub fn accept_completion(&mut self) {
        let (row, col) = {
            let win = self.active_window();
            (win.row, win.col)
        };

        // 1. Always get the FULL replacement text from candidates
        let replacement = match self.comp.candidates().get(self.comp.completion_idx()) {
            Some(c) => c.text.clone(),
            None => {
                self.comp.reset_to_idle();
                return;
            }
        };

        // Clear the ghost text display state so it doesn't linger
        let _ = self.comp.ghost_text.take();

        // 2. Extract buffer data, then drop the borrow
        let (line_text, line_start_char) = {
            let buf = self.buf();
            if row >= buf.len_lines() {
                self.comp.reset_to_idle();
                return;
            }
            (buf.line_text(row), buf.rope.line_to_char(row))
        };

        // 3. Compute overlaps from the cloned strings (no borrow conflict)
        let before: String = line_text.chars().take(col).collect();
        let after: String = line_text.chars().skip(col).collect();

        let prefix_overlap = crate::comp::state::find_prefix_overlap(&before, &replacement);
        let ghost_suffix: String = replacement.chars().skip(prefix_overlap).collect();

        let suffix_overlap = Self::common_prefix_len(&after, &ghost_suffix);
        let to_insert: String = ghost_suffix.chars().skip(suffix_overlap).collect();

        // 4. Calculate how many characters to delete BEFORE the cursor.
        // This is needed when the replacement text does not start with the
        // word prefix that was typed (e.g. typing `ag` and LSP returns `get_age()`).
        let delete_back = if prefix_overlap == 0 && !self.comp.prefix().is_empty() {
            let current_prefix = self.comp.prefix();
            // Find the last trigger character to determine how much of the
            // prefix belongs to the current completion context.
            let mut trigger_end = 0usize;
            if let Some(pos) = current_prefix.rfind("::") {
                trigger_end = trigger_end.max(pos + 2);
            }
            if let Some(pos) = current_prefix.rfind("->") {
                trigger_end = trigger_end.max(pos + 2);
            }
            if let Some(pos) = current_prefix.rfind('.') {
                trigger_end = trigger_end.max(pos + 1);
            }

            if trigger_end > 0 {
                // Delete only the part after the trigger (e.g., "ag" in "person.ag")
                current_prefix.chars().skip(trigger_end).count()
            } else {
                // No trigger; delete the whole prefix (e.g., "per" for "person")
                current_prefix.chars().count()
            }
        } else {
            0
        };

        self.comp.reset_to_idle();

        // 5. Now mutably borrow window + buffer for the actual edit
        {
            let (win, buf) = self.active_window_and_buf_mut();
            buf.push_undo(row, col);

            let mut insert_col = col;

            // A. Delete forward (suffix overlap, e.g., closing parenthesis)
            if suffix_overlap > 0 {
                let remove_start = line_start_char + col;
                let remove_end = remove_start + suffix_overlap;
                if remove_end <= buf.rope.len_chars() {
                    buf.rope.remove(remove_start..remove_end);
                }
            }

            // B. Delete backward (unmatched prefix like "ag")
            if delete_back > 0 {
                let remove_end = line_start_char + col;
                let remove_start = remove_end.saturating_sub(delete_back);
                if remove_start < buf.rope.len_chars() {
                    buf.rope.remove(remove_start..remove_end);
                }
                insert_col = col.saturating_sub(delete_back);
            }

            // C. Insert the new text
            if !to_insert.is_empty() {
                buf.rope.insert(line_start_char + insert_col, &to_insert);
            }

            buf.mark_modified();
            buf.parse_syntax();

            // Handle multiline cursor positioning
            if to_insert.contains('\n') {
                let newlines = to_insert.matches('\n').count();
                let last_line_len = to_insert
                    .split('\n')
                    .last()
                    .map(|s| s.chars().count())
                    .unwrap_or(0);
                win.row = (row + newlines).min(buf.len_lines().saturating_sub(1));
                win.col = last_line_len;
            } else {
                win.col = insert_col + to_insert.chars().count();
            }
            win.desired_col = win.col;
        }

        // 6. Notify LSP and refresh state
        if to_insert.contains('\n') {
            if let Some(filename) = self.active_filename() {
                self.lsp_notify_change_full(std::path::PathBuf::from(filename));
            }
        } else if !to_insert.is_empty() {
            self.lsp_notify_insert_text(&to_insert);
        }

        self.maybe_refresh_buffer_words();
        self.git_debounce.notify_edit(self.buf().id);
    }

    /// Returns the length of the common prefix of two strings (in chars).
    fn common_prefix_len(a: &str, b: &str) -> usize {
        a.chars().zip(b.chars()).take_while(|(x, y)| x == y).count()
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// Word under cursor
// ═══════════════════════════════════════════════════════════════════════════

impl Editor {
    /// Locates the word under (or immediately after) the active window's
    /// cursor position on the line.
    pub fn get_word_under_cursor(&self) -> Option<String> {
        let win = self.active_window();
        let buf = self.buf();

        if win.row >= buf.len_lines() {
            return None;
        }

        let line_text = buf.line_text(win.row);
        let chars: Vec<char> = line_text.chars().collect();
        if chars.is_empty() {
            return None;
        }

        let col = win.col.min(chars.len().saturating_sub(1));
        let mut start = col;

        while start < chars.len() && !chars[start].is_alphanumeric() && chars[start] != '_' {
            start += 1;
        }

        if start >= chars.len() {
            return None;
        }

        let mut end = start;

        while start > 0 && (chars[start - 1].is_alphanumeric() || chars[start - 1] == '_') {
            start -= 1;
        }

        while end < chars.len() && (chars[end].is_alphanumeric() || chars[end] == '_') {
            end += 1;
        }

        if start < end {
            Some(chars[start..end].iter().collect())
        } else {
            None
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// Manual completion trigger
// ═══════════════════════════════════════════════════════════════════════════

impl Editor {
    /// Manual completion (Alt+/) — scans synchronously, merges with
    /// any existing results from the current version.
    pub fn trigger_manual_completion(&mut self) {
        if self.mode != Mode::Insert && self.mode != Mode::Brief {
            return;
        }

        let prefix = self.get_current_word_prefix();
        if prefix.is_empty() {
            self.set_status_msg("No word prefix under cursor", MessageKind::Info);
            return;
        }

        self.comp.set_prefix(prefix.clone());

        let min_len = prefix.len().max(3);
        let mut matches: Vec<String> = Vec::new();
        let mut seen = std::collections::HashSet::new();

        // ── Buffer words ──────────────────────────────────────────
        if !self.buffer_words.is_empty() {
            for word in &self.buffer_words {
                if matches.len() >= 50 {
                    break;
                }
                if word.starts_with(&prefix) && word.as_str() != prefix {
                    if seen.insert(word.clone()) {
                        matches.push(word.clone());
                    }
                }
            }
        } else {
            let buf = self.buf();
            let total = buf.len_lines();
            for i in 0..total {
                if matches.len() >= 50 {
                    break;
                }
                for w in buf
                    .line_text(i)
                    .split(|c: char| !c.is_alphanumeric() && c != '_')
                {
                    if matches.len() >= 50 {
                        break;
                    }
                    if w.len() >= min_len && w.starts_with(&prefix) && w != prefix {
                        if seen.insert(w.to_string()) {
                            matches.push(w.to_string());
                        }
                    }
                }
            }
        }

        // ── Vocab words ─────────────────────────────────────────
        for word in &self.vocab_words {
            if matches.len() >= 50 {
                break;
            }
            if word.starts_with(&prefix) && word.as_str() != prefix {
                if seen.insert(word.clone()) {
                    matches.push(word.clone());
                }
            }
        }

        if matches.is_empty() {
            self.set_status_msg("No completions found", MessageKind::Info);
            return;
        }

        matches.sort();
        matches.dedup();

        let version = self.comp.current_version();
        self.comp
            .merge_source(CompletionSource::Manual, matches, version);
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// Vocabulary & Word Index
// ═══════════════════════════════════════════════════════════════════════════

impl Editor {
    /// Refresh the buffer word cache from the current buffer.
    pub fn refresh_buffer_words(&mut self) {
        if !self.config.buffer_word_scan {
            self.buffer_words.clear();
            return;
        }
        let total = self.buf().len_lines();
        let mut words = std::collections::HashSet::new();
        let buf = self.buf();
        for i in 0..total {
            for w in buf
                .line_text(i)
                .split(|c: char| !c.is_alphanumeric() && c != '_')
            {
                if w.len() >= 6 {
                    words.insert(w.to_string());
                }
            }
        }
        self.buffer_words = words.into_iter().collect();
    }

    /// Call `refresh_buffer_words()` only when the config allows it.
    pub fn maybe_refresh_buffer_words(&mut self) {
        if self.config.buffer_word_scan {
            self.refresh_buffer_words();
        }
    }

    /// Pre-load vocabulary from the user's wordlist file.
    pub fn preload_vocabulary() -> std::collections::HashSet<String> {
        if let Ok(config) = Config::load() {
            if !config.vocab_wordlist {
                return std::collections::HashSet::new();
            }
        }
        let mut words = std::collections::HashSet::new();
        if let Ok(dir) = Config::config_dir() {
            let path = dir.join("wordlist.txt");
            if let Ok(content) = std::fs::read_to_string(path) {
                for line in content.lines() {
                    let t = line.trim();
                    if !t.is_empty() {
                        words.insert(t.to_string());
                    }
                }
            }
        }
        words
    }

    /// Add a word to the user's vocabulary and persist to disk.
    pub fn add_vocab_word(&mut self, word: &str) -> anyhow::Result<()> {
        let trimmed = word.trim().to_string();
        if trimmed.is_empty() {
            return Ok(());
        }
        if self.vocab_words.insert(trimmed.clone()) {
            if let Ok(dir) = Config::config_dir() {
                use std::io::Write;
                let mut file = std::fs::OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(dir.join("wordlist.txt"))?;
                writeln!(file, "{}", trimmed)?;
            }
        }
        Ok(())
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// Copilot Authentication
// ═══════════════════════════════════════════════════════════════════════════

impl Editor {
    /// Kick off the Copilot authentication flow in a background thread.
    /// Only starts when `copilot_enabled` is true in config.
    pub fn copilot_auth(&mut self) {
        if !self.config.copilot_enabled {
            self.set_status_msg(
                "Copilot is disabled. Set copilot_enabled: true in config.",
                MessageKind::Error,
            );
            return;
        }

        if self.copilot_auth_rx.is_some() {
            self.set_status_msg("Copilot auth already in progress...", MessageKind::Info);
            return;
        }

        let (tx, rx) = std::sync::mpsc::channel::<crate::ai::copilot::auth::CopilotAuthMsg>();
        self.copilot_auth_rx = Some(rx);

        self.set_status_msg("Requesting Copilot device code...", MessageKind::Info);

        std::thread::spawn(move || {
            let rt = tokio::runtime::Runtime::new().unwrap();
            rt.block_on(async {
                let client = reqwest::Client::new();

                // 1. Request device code
                let res = client
                    .post("https://github.com/login/device/code")
                    .header("User-Agent", "copilot-cli/0.1.0")
                    .header("Accept", "application/json")
                    .form(&[
                        ("client_id", "Iv1.b507a08c87ecfe98"),
                        ("scope", "read:user"),
                    ])
                    .send()
                    .await;

                let resp = match res {
                    Ok(r) => r,
                    Err(e) => {
                        let _ = tx.send(crate::ai::copilot::auth::CopilotAuthMsg::Error(format!(
                            "Auth request failed: {}",
                            e
                        )));
                        return;
                    }
                };

                let device: crate::ai::copilot::types::DeviceCodeResponse =
                    match resp.json().await {
                        Ok(d) => d,
                        Err(e) => {
                            let _ = tx.send(crate::ai::copilot::auth::CopilotAuthMsg::Error(
                                format!("Parse device code failed: {}", e),
                            ));
                            return;
                        }
                    };

                let _ = tx.send(crate::ai::copilot::auth::CopilotAuthMsg::DeviceCode(
                    device.verification_uri.clone(),
                    device.user_code.clone(),
                ));

                // Try to open browser
                if let Err(e) = crate::ai::copilot::auth::open_browser(&device.verification_uri) {
                    log::debug!("Could not open browser automatically: {}", e);
                }

                // 2. Poll for access token
                let deadline = std::time::Instant::now()
                    + std::time::Duration::from_secs(device.expires_in as u64);
                let mut interval = device.interval.max(5) as u64;

                loop {
                    std::thread::sleep(std::time::Duration::from_secs(interval));

                    if std::time::Instant::now() > deadline {
                        let _ = tx.send(crate::ai::copilot::auth::CopilotAuthMsg::Error(
                            "Copilot auth timed out".into(),
                        ));
                        return;
                    }

                    let poll_res = client
                        .post("https://github.com/login/oauth/access_token")
                        .header("User-Agent", "copilot-cli/0.1.0")
                        .header("Accept", "application/json")
                        .form(&[
                            ("client_id", "Iv1.b507a08c87ecfe98"),
                            ("device_code", &device.device_code),
                            ("grant_type", "urn:ietf:params:oauth:grant-type:device_code"),
                        ])
                        .send()
                        .await;

                    match poll_res {
                        Ok(poll_resp) => {
                            if let Ok(result) = poll_resp
                                .json::<crate::ai::copilot::types::DeviceAccessTokenResponse>()
                                .await
                            {
                                if let Some(error) = &result.error {
                                    match error.as_str() {
                                        "authorization_pending" => continue,
                                        "slow_down" => {
                                            interval += 5;
                                            continue;
                                        }
                                        _ => {
                                            let _ = tx.send(
                                                crate::ai::copilot::auth::CopilotAuthMsg::Error(
                                                    format!("Auth error: {}", error),
                                                ),
                                            );
                                            return;
                                        }
                                    }
                                } else if let Some(access_token) = result.access_token {
                                    let _ =
                                        tx.send(crate::ai::copilot::auth::CopilotAuthMsg::Success(
                                            access_token,
                                        ));
                                    return;
                                } else {
                                    let _ =
                                        tx.send(crate::ai::copilot::auth::CopilotAuthMsg::Error(
                                            "No token received".into(),
                                        ));
                                    return;
                                }
                            }
                        }
                        Err(_) => continue, // Network error, retry
                    }
                }
            });
        });
    }

    /// Poll for Copilot completion responses and merge them into the
    /// completion machine. Call this from the main tick handler.
    pub fn poll_copilot_completions(&mut self) {
        if !self.config.copilot_enabled {
            if let Some(rx) = &mut self.copilot_response_rx {
                while rx.try_recv().is_ok() {}
            }
            return;
        }

        let messages: Vec<_> = {
            let Some(rx) = &mut self.copilot_response_rx else {
                return;
            };
            std::iter::from_fn(|| rx.try_recv().ok()).collect()
        };

        for msg in messages {
            match msg {
                crate::ai::copilot::server::CopilotResponse::Items { version, items } => {
                    self.comp
                        .merge_source(CompletionSource::Copilot, items, version);
                }
                crate::ai::copilot::server::CopilotResponse::Error { version, error } => {
                    log::debug!("Copilot completion error: {}", error);
                    self.comp
                        .merge_source(CompletionSource::Copilot, Vec::new(), version);
                }
                crate::ai::copilot::server::CopilotResponse::ReauthResult { ok } => {
                    if ok {
                        log::info!("Copilot re-authenticated successfully.");
                    } else {
                        log::warn!("Copilot re-auth failed — token may still be invalid.");
                    }
                }
            }
        }
    }

    /// Poll for Copilot auth messages (call this from the main loop).
    pub fn poll_copilot_auth(&mut self) {
        if !self.config.copilot_enabled {
            // Drain and discard any stale messages
            if let Some(rx) = &mut self.copilot_auth_rx {
                while rx.try_recv().is_ok() {}
                self.copilot_auth_rx = None;
            }
            return;
        }
        let messages: Vec<_> = {
            let Some(rx) = &mut self.copilot_auth_rx else {
                return;
            };
            std::iter::from_fn(|| rx.try_recv().ok()).collect()
        };

        for msg in messages {
            match msg {
                crate::ai::copilot::auth::CopilotAuthMsg::DeviceCode(uri, code) => {
                    let msg = format!("Copilot Code: {} | Visit: {}", code, uri);
                    self.set_status_msg(&msg, MessageKind::Info);
                }
                crate::ai::copilot::auth::CopilotAuthMsg::Success(access_token) => {
                    self.copilot_auth_rx = None;
                    match crate::ai::copilot::auth::AuthManager::save_token_to_hosts(&access_token)
                    {
                        Ok(path) => {
                            self.config.api_key = Some(access_token);
                            self.set_status_msg(
                                &format!("Copilot authenticated! Token saved to {:?}", path),
                                MessageKind::Success,
                            );
                        }
                        Err(e) => {
                            self.set_status_msg(
                                &format!("Auth succeeded but failed to save token: {}", e),
                                MessageKind::Error,
                            );
                        }
                    }
                }
                crate::ai::copilot::auth::CopilotAuthMsg::Error(e) => {
                    self.copilot_auth_rx = None;
                    self.set_status_msg(&format!("Copilot auth failed: {}", e), MessageKind::Error);
                }
            }
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// LSP Communication — file lifecycle, change notifications, requests
// ═══════════════════════════════════════════════════════════════════════════

impl Editor {
    /// Get the LSP sender, if available.
    fn lsp_sender(&self) -> Option<&tokio::sync::mpsc::UnboundedSender<LspMessage>> {
        self.lsp_tx.as_ref()
    }

    // ── File lifecycle ────────────────────────────────────────────

    pub fn lsp_notify_open(&mut self, path: std::path::PathBuf) {
        if let Some(tx) = self.lsp_sender() {
            let text = self.buf().rope.to_string();
            let _ = tx.send(LspMessage::OpenFile(path, text));
        }
    }

    pub fn lsp_notify_close(&mut self, path: std::path::PathBuf) {
        if let Some(tx) = self.lsp_sender() {
            let _ = tx.send(LspMessage::CloseFile(path));
        }
    }

    pub fn lsp_notify_change(&mut self, path: std::path::PathBuf) {
        if self.lsp_tx.is_none() {
            return;
        }
        let uri = path_to_uri(&path);
        let version = {
            let v = self.lsp_file_versions.entry(uri).or_insert(1);
            *v += 1;
            *v
        };
        let text = self.buf().rope.to_string();
        if let Some(tx) = self.lsp_tx.as_ref() {
            let _ = tx.send(LspMessage::ChangeFile(path, String::new(), text, version));
        }
    }

    /// Notify LSP of text insertion at cursor (for tabs, etc.).
    pub fn lsp_notify_insert_text(&mut self, text: &str) {
        if self.lsp_tx.is_none() || text.is_empty() {
            return;
        }

        let path = match self.active_filename() {
            Some(f) => std::path::PathBuf::from(f),
            None => return,
        };

        let win = self.active_window();
        let col_start = win.col;
        let col_end = col_start + text.chars().count();

        let buf = self.buf();
        let encoding = self.lsp_offset_encoding;

        let line_start = buf.rope.line_to_char(win.row);
        let start_pos = crate::lsp::pos_to_lsp_pos(&buf.rope, line_start + col_start, encoding);
        let end_pos = crate::lsp::pos_to_lsp_pos(&buf.rope, line_start + col_end, encoding);

        let uri = crate::lsp::path_to_uri(&path);
        let version = {
            let v = self.lsp_file_versions.entry(uri).or_insert(1);
            *v += 1;
            *v
        };

        if let Some(tx) = self.lsp_tx.as_ref() {
            let _ = tx.send(LspMessage::ChangeFileIncremental {
                path,
                version,
                start_line: start_pos.line,
                start_char: start_pos.character,
                end_line: end_pos.line,
                end_char: end_pos.character,
                new_text: text.to_string(),
            });
        }
    }

    /// Notify LSP of a single-character insert at the cursor.
    pub fn lsp_notify_insert_edit(&mut self, ch: char) {
        if self.lsp_tx.is_none() {
            return;
        }

        let filename = match self.active_filename() {
            Some(f) => f.to_string(),
            None => return,
        };
        let path = std::path::PathBuf::from(filename);

        let win = self.active_window();
        let col_after = win.col;
        let col_start = col_after.saturating_sub(1);

        let buf = self.buf();
        let encoding = self.lsp_offset_encoding;

        let line_start = buf.rope.line_to_char(win.row);
        let start_pos = crate::lsp::pos_to_lsp_pos(&buf.rope, line_start + col_start, encoding);
        let end_pos = start_pos;

        let uri = crate::lsp::path_to_uri(&path);
        let version = {
            let v = self.lsp_file_versions.entry(uri.clone()).or_insert(1);
            *v += 1;
            *v
        };

        if let Some(tx) = self.lsp_tx.as_ref() {
            let _ = tx.send(LspMessage::ChangeFileIncremental {
                path,
                version,
                start_line: start_pos.line,
                start_char: start_pos.character,
                end_line: end_pos.line,
                end_char: end_pos.character,
                new_text: ch.to_string(),
            });
        }
    }

    /// Notify LSP of a backspace deletion.
    pub fn lsp_notify_backspace(&mut self, deleted_range: Option<(usize, usize, usize, usize)>) {
        if self.lsp_tx.is_none() {
            return;
        }

        let filename = match self.active_filename() {
            Some(f) => f.to_string(),
            None => return,
        };
        let path = std::path::PathBuf::from(filename);

        match deleted_range {
            Some((start_row, start_col, end_row, end_col)) => {
                let buf = self.buf();
                let encoding = self.lsp_offset_encoding;

                let line_start = buf.rope.line_to_char(start_row);
                let start_pos =
                    crate::lsp::pos_to_lsp_pos(&buf.rope, line_start + start_col, encoding);
                let end_pos = crate::lsp::pos_to_lsp_pos(&buf.rope, line_start + end_col, encoding);

                let uri = crate::lsp::path_to_uri(&path);
                let version = {
                    let v = self.lsp_file_versions.entry(uri).or_insert(1);
                    *v += 1;
                    *v
                };

                if let Some(tx) = self.lsp_tx.as_ref() {
                    let _ = tx.send(LspMessage::ChangeFileIncremental {
                        path,
                        version,
                        start_line: start_pos.line,
                        start_char: start_pos.character,
                        end_line: end_pos.line,
                        end_char: end_pos.character,
                        new_text: String::new(),
                    });
                }
            }
            None => {
                self.lsp_notify_change_full(path);
            }
        }
    }

    /// Notify LSP of a delete-char-forward operation.
    pub fn lsp_notify_delete_forward(
        &mut self,
        deleted_range: Option<(usize, usize, usize, usize)>,
    ) {
        if self.lsp_tx.is_none() {
            return;
        }

        let filename = match self.active_filename() {
            Some(f) => f.to_string(),
            None => return,
        };
        let path = std::path::PathBuf::from(filename);

        match deleted_range {
            Some((start_row, start_col, end_row, end_col)) => {
                let buf = self.buf();
                let encoding = self.lsp_offset_encoding;

                let line_start = buf.rope.line_to_char(start_row);
                let start_pos =
                    crate::lsp::pos_to_lsp_pos(&buf.rope, line_start + start_col, encoding);
                let end_pos = crate::lsp::pos_to_lsp_pos(&buf.rope, line_start + end_col, encoding);

                let uri = crate::lsp::path_to_uri(&path);
                let version = {
                    let v = self.lsp_file_versions.entry(uri).or_insert(1);
                    *v += 1;
                    *v
                };

                if let Some(tx) = self.lsp_tx.as_ref() {
                    let _ = tx.send(LspMessage::ChangeFileIncremental {
                        path,
                        version,
                        start_line: start_pos.line,
                        start_char: start_pos.character,
                        end_line: end_pos.line,
                        end_char: end_pos.character,
                        new_text: String::new(),
                    });
                }
            }
            None => {
                self.lsp_notify_change_full(path);
            }
        }
    }

    /// Notify LSP of a deletion at the cursor.
    pub fn lsp_notify_delete_edit(&mut self, start_char: usize, end_char: usize) {
        if self.lsp_tx.is_none() {
            return;
        }

        let filename = match self.active_filename() {
            Some(f) => f.to_string(),
            None => return,
        };
        let path = std::path::PathBuf::from(filename);

        let row = self.active_window().row;
        let buf = self.buf();
        let encoding = self.lsp_offset_encoding;

        let line_start = buf.rope.line_to_char(row);
        let start_pos = crate::lsp::pos_to_lsp_pos(&buf.rope, line_start + start_char, encoding);
        let end_pos = crate::lsp::pos_to_lsp_pos(&buf.rope, line_start + end_char, encoding);

        let uri = crate::lsp::path_to_uri(&path);
        let version = {
            let v = self.lsp_file_versions.entry(uri.clone()).or_insert(1);
            *v += 1;
            *v
        };

        if let Some(tx) = self.lsp_tx.as_ref() {
            let _ = tx.send(LspMessage::ChangeFileIncremental {
                path,
                version,
                start_line: start_pos.line,
                start_char: start_pos.character,
                end_line: end_pos.line,
                end_char: end_pos.character,
                new_text: String::new(),
            });
        }
    }

    pub fn lsp_notify_save(&mut self, path: std::path::PathBuf) {
        if let Some(tx) = self.lsp_sender() {
            let _ = tx.send(LspMessage::SaveFile(path));
        }
    }

    /// Notify LSP of an incremental text change.
    pub fn lsp_notify_change_incremental(
        &mut self,
        path: std::path::PathBuf,
        edit: &crate::ed::editing::Edit,
    ) {
        if self.lsp_tx.is_none() {
            return;
        }

        let uri = path_to_uri(&path);
        let version = {
            let v = self.lsp_file_versions.entry(uri.clone()).or_insert(0);
            *v += 1;
            *v
        };

        let buf = self.buf();
        let encoding = self.lsp_offset_encoding;

        let (start_line, start_char) = {
            let pos = crate::lsp::pos_to_lsp_pos(&buf.rope, edit.start, encoding);
            (pos.line, pos.character)
        };
        let (end_line, end_char) = {
            let pos = crate::lsp::pos_to_lsp_pos(&buf.rope, edit.end, encoding);
            (pos.line, pos.character)
        };

        if let Some(tx) = self.lsp_tx.as_ref() {
            let _ = tx.send(LspMessage::ChangeFileIncremental {
                path,
                version,
                start_line,
                start_char,
                end_line,
                end_char,
                new_text: edit.inserted_text.clone(),
            });
        }
    }

    /// Legacy full-sync fallback (use only for bulk operations like formatting).
    pub fn lsp_notify_change_full(&mut self, path: std::path::PathBuf) {
        if self.lsp_tx.is_none() {
            return;
        }
        let uri = path_to_uri(&path);
        let version = {
            let v = self.lsp_file_versions.entry(uri).or_insert(1);
            *v += 1;
            *v
        };
        let text = self.buf().rope.to_string();
        if let Some(tx) = self.lsp_tx.as_ref() {
            let _ = tx.send(LspMessage::ChangeFile(path, String::new(), text, version));
        }
    }

    /// Request LSP completions, using the completion machine's version
    /// as the authoritative version (since it's bumped on every edit).
    pub fn lsp_request_completion(
        &mut self,
        path: &std::path::PathBuf,
        line: u32,
        col: u32,
        comp_version: u64,
    ) {
        let uri = path_to_uri(path);

        {
            let doc_version = self.lsp_file_versions.entry(uri.clone()).or_insert(0);
            if (*doc_version as u64) < comp_version {
                *doc_version = comp_version as i32;
            }
        }

        let doc_version = self.lsp_file_versions.get(&uri).copied().unwrap_or(0);

        self.comp_lsp_version_map
            .insert(comp_version, (uri.clone(), doc_version));

        if let Some(tx) = self.lsp_sender() {
            let _ = tx.send(LspMessage::RequestCompletion(
                path.clone(),
                line,
                col,
                None,
                comp_version,
            ));
        }
    }

    // ── Signature help ────────────────────────────────────────────

    pub fn lsp_request_signature_help(&mut self, path: &std::path::PathBuf, line: u32, col: u32) {
        if let Some(tx) = self.lsp_sender() {
            let _ = tx.send(LspMessage::RequestSignatureHelp(path.clone(), line, col));
        }
    }

    // ── Formatting ────────────────────────────────────────────────
    pub fn lsp_request_formatting(
        &mut self,
        path: std::path::PathBuf,
        buffer_idx: usize,
        save_after: bool,
    ) {
        if let Some(tx) = self.lsp_sender() {
            let win = self.active_window();
            let cursor = Some((win.row, win.col));
            let text = self.buf().rope.to_string();
            let tab_size = self.config.tab_size as u32;
            let options = FormattingOptions {
                tab_size,
                insert_spaces: self.config.insert_spaces,
                trim_trailing_whitespace: Some(true),
                insert_final_newline: Some(true),
                trim_final_newlines: Some(true),
            };
            let _ = tx.send(LspMessage::RequestFormatting(
                path, text, options, buffer_idx, cursor, save_after,
            ));
        }
    }

    // ── Inlay hints ───────────────────────────────────────────────

    pub fn lsp_request_inlay_hints(&mut self, path: &std::path::PathBuf, version: i32) {
        if let Some(tx) = self.lsp_sender() {
            let total_lines = self.buf().len_lines();
            let _ = tx.send(LspMessage::RequestInlayHintsRange(
                path.clone(),
                0,
                total_lines as usize,
                version,
            ));
        }
    }

    fn apply_lsp_diagnostics(
        &mut self,
        uri: &str,
        diagnostics: Vec<crate::ed::buffer::Diagnostic>,
    ) {
        let path = uri_to_path(uri);
        for buf in &mut self.buffers {
            if buf.filename.as_deref() == path.to_str() {
                buf.diagnostics = diagnostics;
                return;
            }
        }
    }

    fn apply_lsp_format(
        &mut self,
        result: Result<Option<Vec<TextEdit>>, String>,
        buffer_idx: usize,
        cursor_state: Option<(usize, usize)>,
        save_after: bool,
    ) {
        let edits = match result {
            Ok(Some(edits)) => edits,
            Ok(None) => return,
            Err(e) => {
                self.set_status_msg(&format!("Format error: {}", e), MessageKind::Error);
                return;
            }
        };

        if edits.is_empty() {
            return;
        }

        if buffer_idx >= self.buffers.len() {
            return;
        }

        let (line_count, filename) = {
            let buf = &mut self.buffers[buffer_idx];

            let mut sorted_edits = edits.clone();
            sorted_edits.sort_by(|a, b| {
                let a_start =
                    a.range.start.line as usize * 1_000_000 + a.range.start.character as usize;
                let b_start =
                    b.range.start.line as usize * 1_000_000 + b.range.start.character as usize;
                b_start.cmp(&a_start)
            });

            for edit in &sorted_edits {
                let start_line = edit.range.start.line as usize;
                let start_char = edit.range.start.character as usize;
                let end_line = edit.range.end.line as usize;
                let end_char = edit.range.end.character as usize;

                if start_line >= buf.len_lines() || end_line >= buf.len_lines() {
                    continue;
                }

                let start_offset = buf.rope.line_to_char(start_line)
                    + start_char.min(buf.line_char_len(start_line));
                let end_offset =
                    buf.rope.line_to_char(end_line) + end_char.min(buf.line_char_len(end_line));

                if end_offset > start_offset {
                    buf.rope.remove(start_offset..end_offset);
                }
                if !edit.new_text.is_empty() {
                    buf.rope.insert(start_offset, &edit.new_text);
                }
            }

            buf.mark_modified();
            buf.parse_syntax();

            (buf.len_lines(), buf.filename.clone())
        };

        if let Some((row, col)) = cursor_state {
            let win = self.active_window_mut();
            win.row = row.min(line_count.saturating_sub(1));
            win.col = col;
        }

        if save_after && filename.is_some() {
            let _ = self.save_active_buffer();
        }
    }

    fn apply_lsp_inlay_hints(&mut self, uri: String, hints: Vec<InlayHint>, _version: i32) {
        self.inlay_hints.insert(uri, hints);
    }

    // ── Shutdown ──────────────────────────────────────────────────

    pub fn lsp_shutdown(&mut self) {
        if let Some(tx) = self.lsp_sender() {
            let _ = tx.send(LspMessage::Shutdown);
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// LSP Response Polling & Dispatch
// ═══════════════════════════════════════════════════════════════════════════

impl Editor {
    pub fn poll_lsp_responses(&mut self) {
        let messages: Vec<_> = {
            let Some(rx) = &mut self.lsp_rx else { return };
            std::iter::from_fn(|| rx.try_recv().ok()).collect()
        };
        for msg in messages {
            self.handle_lsp_message(msg);
        }

        self.check_lsp_goto_timeout();
    }

    fn handle_lsp_message(&mut self, msg: AppMessage) {
        match msg {
            AppMessage::LspDiagnostics {
                uri, diagnostics, ..
            } => {
                self.apply_lsp_diagnostics(&uri, diagnostics);
            }

            AppMessage::LspFormatResult {
                result,
                buffer_idx,
                cursor_state,
                save_after,
            } => {
                self.apply_lsp_format(result, buffer_idx, cursor_state, save_after);
            }

            AppMessage::LspInlayHints {
                uri,
                hints,
                version,
            } => {
                self.apply_lsp_inlay_hints(uri, hints, version);
            }

            AppMessage::LspSignatureHelp(state) => {
                self.signature_help = state;
            }

            AppMessage::LspCompletion { items, version } => {
                self.apply_lsp_completion(items, version);
            }

            AppMessage::LspCompletionResolved(item) => {
                log::debug!("Resolved completion item: {:?}", item.label);
            }

            AppMessage::LspError(e) => {
                log::warn!("LSP error: {}", e);
            }

            AppMessage::LspGotoDefinitionResult { locations } => {
                self.handle_lsp_goto_result(locations);
            }
        }
    }

    /// LSP responded to go-to-definition.
    fn handle_lsp_goto_result(&mut self, locations: Vec<Location>) {
        let pending = match self.pending_lsp_goto.take() {
            Some(p) => p,
            None => return,
        };

        if locations.is_empty() {
            if pending.pushed_tag_stack {
                self.tag_manager.pop();
            }
            self.tag_goto_fallback(&pending.symbol);
            return;
        }

        if locations.len() == 1 {
            self.lsp_do_goto_jump(&locations[0], &pending.symbol, 1);
        } else {
            self.lsp_goto_picker(locations, &pending.symbol);
        }
    }

    /// LSP timeout — fall back to ctagd / ctags.
    pub fn check_lsp_goto_timeout(&mut self) {
        let timed_out = self.pending_lsp_goto.as_ref().map_or(false, |p| {
            p.created_at.elapsed() > std::time::Duration::from_millis(LSP_GOTO_TIMEOUT_MS)
        });

        if !timed_out {
            return;
        }

        let pending = self.pending_lsp_goto.take().unwrap();
        if pending.pushed_tag_stack {
            self.tag_manager.pop();
        }

        self.set_status_msg(
            &format!("LSP timeout — falling back for '{}'", pending.symbol),
            MessageKind::Info,
        );

        self.tag_goto_fallback(&pending.symbol);
    }

    /// Fall back to ctagd → ctags (used when LSP times out or returns empty).
    fn tag_goto_fallback(&mut self, symbol: &str) {
        if self.ctagd.is_available() {
            if let Some(()) = self.ctagd_definition(symbol) {
                return;
            }
        }
        self.ctags_jump(symbol);
    }
}
