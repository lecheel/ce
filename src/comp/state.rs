//! Completion state machine (ghost-text).
//!
//! `CompletionMachine` is the single owner of all completion lifecycle.
//! The editor calls one method per event; the machine decides what to do.
//!
//! ## Source taxonomy
//!
//! | Source            | Trigger              | Min-prefix | Ghost style        |
//! |-------------------|----------------------|------------|--------------------|
//! | LSP               | keystroke (via edit) | 0          | single-line popup  |
//! | Codeium / Copilot | 500 ms idle          | 0          | multi-line ghost   |
//! | BufferWords       | keystroke            | 4          | single-line ghost  |
//! | VocabWords        | keystroke            | 4          | single-line ghost  |
//! | FilePaths         | keystroke ("./")     | 2          | single-line ghost  |
//! | Manual            | Alt+/                | any        | single-line ghost  |

use crate::ed::buffer::{detect_language, Buffer};
use crate::ed::mode::Mode;
use ratatui::style::Color;
use std::collections::HashMap;

// ---------------------------------------------------------------------------
// CompletionSource
// ---------------------------------------------------------------------------

/// Identifies which source provided a completion candidate.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CompletionSource {
    Lsp,
    Codeium,
    Copilot,
    BufferWords,
    VocabWords,
    Manual,
    FilePaths,
}

impl CompletionSource {
    pub fn badge(&self) -> &'static str {
        match self {
            Self::Lsp => "LSP",
            Self::Codeium => "COD",
            Self::Copilot => "COP",
            Self::BufferWords => "BUF",
            Self::VocabWords => "VOC",
            Self::Manual => "M",
            Self::FilePaths => "Path",
        }
    }

    pub fn badge_color(&self) -> Color {
        match self {
            Self::Lsp => Color::Cyan,
            Self::Codeium => Color::LightCyan,
            Self::Copilot => Color::LightMagenta,
            Self::BufferWords => Color::Green,
            Self::VocabWords => Color::Magenta,
            Self::Manual => Color::Yellow,
            Self::FilePaths => Color::Cyan,
        }
    }

    /// AI sources fire on idle-timeout, not on every keystroke.
    /// They tolerate an empty word-prefix and produce multi-line suggestions.
    #[inline]
    pub fn is_ai(&self) -> bool {
        matches!(self, Self::Codeium | Self::Copilot)
    }
}

// ---------------------------------------------------------------------------
// Supporting types
// ---------------------------------------------------------------------------

/// A single candidate with source provenance and score.
#[derive(Debug, Clone)]
pub struct CompletionCandidate {
    pub text: String,
    pub source: CompletionSource,
    /// Lower is better. Prefix-length matches rank higher.
    pub score: usize,
}

/// Per-source result bucket, version-gated.
#[derive(Debug, Clone)]
struct SourceBucket {
    version: u64,
    items: Vec<String>,
}

/// Per-source pending request tracking.
#[derive(Debug, Clone, Copy)]
struct SourcePending {
    request_id: usize,
    version: u64,
}

// ---------------------------------------------------------------------------
// Phase
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Phase {
    Idle,
    /// Fast-sources (LSP/buffer/vocab) are being awaited.
    Throttling,
    /// A request has been dispatched but no result yet.
    Pending { row: usize, col: usize },
    /// At least one result is available and being displayed.
    Active,
}

// ---------------------------------------------------------------------------
// CompletionMachine
// ---------------------------------------------------------------------------

#[derive(Debug)]
pub struct CompletionMachine {
    // ── Core state ──────────────────────────────────────────────
    pub phase: Phase,
    pub last_edit_time: std::time::Instant,
    /// Debounce for fast (non-AI) sources, e.g. LSP.
    pub throttle_ms: u64,
    pub request_id: usize,
    pub ghost_text: Option<String>,
    pub completion_idx: usize,

    // ── Multi-source aggregation ─────────────────────────────────
    /// Monotonically increasing version, bumped on every edit.
    prefix_version: u64,
    source_results: HashMap<CompletionSource, SourceBucket>,
    source_pending: HashMap<CompletionSource, SourcePending>,
    merged: Vec<CompletionCandidate>,
    current_prefix: String,

    // ── AI-specific idle debounce ────────────────────────────────
    /// Timestamp of the most recent edit. AI sources fire only when
    /// `ai_idle_ms` have elapsed since this instant without another edit.
    last_ai_trigger_time: std::time::Instant,
    /// How long (ms) the editor must be idle before an AI request fires.
    /// Default: 500 ms — mirrors VS Code / Neovim Copilot behaviour.
    pub ai_idle_ms: u64,
    /// Version at which the last AI request was dispatched.
    /// Used to prevent double-firing on the same version.
    last_ai_request_version: u64,

    // ── Multi-line ghost text ────────────────────────────────────
    /// Full multi-line ghost suggestion (may contain '\n').
    /// `ghost_text` stores the complete text; `ghost_text_display()`
    /// returns only the first line for the cursor row.
    /// `ghost_lines_below()` returns lines 2..N for continuation rows.
    ghost_full: Option<String>,
}

impl Default for CompletionMachine {
    fn default() -> Self {
        Self::new()
    }
}

impl CompletionMachine {
    pub fn new() -> Self {
        let now = std::time::Instant::now();
        Self {
            phase: Phase::Idle,
            last_edit_time: now,
            throttle_ms: 80,   // LSP / buffer sources fire fast
            request_id: 0,
            ghost_text: None,
            completion_idx: 0,
            prefix_version: 0,
            source_results: HashMap::new(),
            source_pending: HashMap::new(),
            merged: Vec::new(),
            current_prefix: String::new(),
            last_ai_trigger_time: now,
            ai_idle_ms: 500,
            last_ai_request_version: u64::MAX,
            ghost_full: None,
        }
    }

    // ═══════════════════════════════════════════════════════════════════════
    // Edit lifecycle
    // ═══════════════════════════════════════════════════════════════════════

    /// Called on every buffer edit while in Insert / Brief mode.
    ///
    /// * Bumps the prefix version so stale results are discarded.
    /// * Clears non-AI ghost text immediately (AI ghost fades after idle).
    /// * Resets the AI idle timer.
    pub fn on_edit(&mut self) {
        let old_version = self.prefix_version;
        self.prefix_version += 1;
        self.phase = Phase::Throttling;
        self.completion_idx = 0;
        self.last_edit_time = std::time::Instant::now();
        self.last_ai_trigger_time = self.last_edit_time;

        // Clear fast-source results immediately.
        self.source_results
            .retain(|src, _| src.is_ai());

        // Keep AI pending entries alive — their in-flight requests
        // are still valid; merge_source version-gates them on arrival.
        self.source_pending
            .retain(|src, _| src.is_ai());

        self.merged.clear();
        self.current_prefix.clear();

        // Dismiss non-AI ghost text so it doesn't linger while typing.
        // AI ghost text is kept briefly (until a new result arrives or
        // ai_idle_ms triggers a fresh request).
        let ghost_is_ai = self
            .ghost_full
            .as_ref()
            .map(|_| {
                // If the current best candidate came from an AI source,
                // keep the ghost; otherwise clear.
                self.merged
                    .first()
                    .map(|c| c.source.is_ai())
                    .unwrap_or(false)
            })
            .unwrap_or(false);

        if !ghost_is_ai {
            self.ghost_text = None;
            self.ghost_full = None;
        }

        log::debug!(
            "[comp:on_edit] version {} → {}, AI pending retained: {}",
            old_version,
            self.prefix_version,
            self.source_pending.len(),
        );
    }

    // ═══════════════════════════════════════════════════════════════════════
    // AI idle-trigger query
    // ═══════════════════════════════════════════════════════════════════════

    /// Returns `true` when AI sources (Codeium / Copilot) should fire a new
    /// request. Called from the main-loop tick via `poll_completion`.
    ///
    /// Conditions:
    /// 1. Mode is Insert or Brief.
    /// 2. At least `ai_idle_ms` have elapsed since the last edit.
    /// 3. We haven't already dispatched a request for this version.
    pub fn should_fire_ai_request(&self, mode: Mode) -> bool {
        if mode != Mode::Insert && mode != Mode::Brief {
            return false;
        }
        if self.last_ai_trigger_time.elapsed()
            < std::time::Duration::from_millis(self.ai_idle_ms)
        {
            return false;
        }
        // Don't re-fire for the same version.
        self.last_ai_request_version != self.prefix_version
    }

    /// Mark the current version as having had an AI request dispatched.
    /// Call this right after sending the request to Codeium / Copilot.
    pub fn mark_ai_request_fired(&mut self) {
        self.last_ai_request_version = self.prefix_version;
    }

    // ═══════════════════════════════════════════════════════════════════════
    // Source request / merge
    // ═══════════════════════════════════════════════════════════════════════

    /// Register that `source` has started a request.
    /// Returns `(request_id, version)` that the source must echo back.
    pub fn start_source_request(&mut self, source: CompletionSource) -> (usize, u64) {
        self.request_id += 1;
        let version = self.prefix_version;
        self.source_pending.insert(
            source,
            SourcePending {
                request_id: self.request_id,
                version,
            },
        );
        (self.request_id, version)
    }

    /// Merge results from `source`.
    ///
    /// **Version-gated:** stale results are silently discarded.
    /// AI sources get a one-version tolerance for in-flight latency.
    pub fn merge_source(&mut self, source: CompletionSource, items: Vec<String>, version: u64) {
        let version_ok = if source.is_ai() {
            // Accept if within one version behind (user typed one more char).
            version == self.prefix_version || version + 1 == self.prefix_version
        } else {
            version == self.prefix_version
        };

        if !version_ok {
            log::debug!(
                "[comp:merge] DROPPED {} results from {:?}: stale v{} != current v{}",
                items.len(),
                source,
                version,
                self.prefix_version,
            );
            return;
        }

        self.source_pending.remove(&source);

        self.source_results.insert(
            source,
            SourceBucket {
                version: self.prefix_version,
                items,
            },
        );

        self.rebuild_merged();

        if !self.merged.is_empty() {
            self.phase = Phase::Active;
            self.update_ghost_text();
        } else if self.source_pending.is_empty() {
            self.phase = Phase::Idle;
            self.ghost_text = None;
            self.ghost_full = None;
        } else {
            self.phase = Phase::Pending { row: 0, col: 0 };
        }
    }

    // ═══════════════════════════════════════════════════════════════════════
    // Merged candidate list
    // ═══════════════════════════════════════════════════════════════════════

    fn source_priority(s: CompletionSource) -> usize {
        match s {
            CompletionSource::Lsp => 0,
            CompletionSource::Codeium => 0,
            CompletionSource::Copilot => 0,
            CompletionSource::Manual => 1,
            CompletionSource::FilePaths => 2,
            CompletionSource::BufferWords => 3,
            CompletionSource::VocabWords => 4,
        }
    }

    fn rebuild_merged(&mut self) {
        let prefix = &self.current_prefix;
        let version = self.prefix_version;
        let prefix_lower = prefix.to_lowercase();

        let mut seen: HashMap<String, CompletionSource> = HashMap::new();
        let mut candidates: Vec<CompletionCandidate> = Vec::new();

        for (source, bucket) in &self.source_results {
            if bucket.version != version {
                continue;
            }

            for text in &bucket.items {
                if text == prefix {
                    continue;
                }

                // AI sources may supply completions that start before the
                // cursor (full-line replacements). Accept them as long as
                // the prefix is empty OR the text contains the prefix.
                let prefix_ok = if source.is_ai() {
                    prefix.is_empty()
                        || text.to_lowercase().contains(&prefix_lower)
                        || text.to_lowercase().starts_with(&prefix_lower)
                } else {
                    prefix.is_empty() || text.to_lowercase().starts_with(&prefix_lower)
                };

                if !prefix_ok {
                    continue;
                }

                let score = text.len();

                if let Some(existing) = seen.get(text) {
                    if Self::source_priority(*source) < Self::source_priority(*existing) {
                        seen.insert(text.clone(), *source);
                    }
                } else {
                    seen.insert(text.clone(), *source);
                    candidates.push(CompletionCandidate {
                        text: text.clone(),
                        source: *source,
                        score,
                    });
                }
            }
        }

        candidates.sort_by(|a, b| {
            // AI multi-line suggestions first (they have the highest value).
            let a_multiline = a.text.contains('\n');
            let b_multiline = b.text.contains('\n');
            b_multiline
                .cmp(&a_multiline)
                .then_with(|| a.score.cmp(&b.score))
                .then_with(|| {
                    Self::source_priority(a.source).cmp(&Self::source_priority(b.source))
                })
                .then_with(|| a.text.cmp(&b.text))
        });

        self.merged = candidates;

        if self.completion_idx >= self.merged.len() {
            self.completion_idx = 0;
        }
    }

    fn update_ghost_text(&mut self) {
        if self.merged.is_empty() {
            self.ghost_text = None;
            self.ghost_full = None;
            return;
        }

        let prefix = &self.current_prefix;
        let best = &self.merged[self.completion_idx].text;

        if prefix.is_empty() || best.starts_with(prefix) || best.contains(prefix.as_str()) {
            self.ghost_full = Some(best.clone());
            // First line goes to the cursor-row ghost slot.
            let first_line = best.split('\n').next().unwrap_or(best);
            self.ghost_text = Some(first_line.to_string());
        } else {
            self.ghost_text = None;
            self.ghost_full = None;
        }
    }

    // ═══════════════════════════════════════════════════════════════════════
    // Public accessors
    // ═══════════════════════════════════════════════════════════════════════

    pub fn completions(&self) -> Vec<String> {
        self.merged.iter().map(|c| c.text.clone()).collect()
    }

    pub fn candidates(&self) -> &[CompletionCandidate] {
        &self.merged
    }

    pub fn completion_idx(&self) -> usize {
        self.completion_idx
    }

    pub fn ghost_text(&self) -> Option<&str> {
        self.ghost_text.as_deref()
    }

    pub fn has_ghost(&self) -> bool {
        self.ghost_text.is_some()
    }

    pub fn is_throttling(&self) -> bool {
        self.phase == Phase::Throttling
    }

    /// First line of ghost text for inline display at the cursor row.
    /// Returns `None` if empty.
    pub fn ghost_text_display(&self) -> Option<String> {
        let full = self.ghost_full.as_deref()?;
        let first = full.split('\n').next().unwrap_or(full);
        if first.is_empty() { None } else { Some(first.to_string()) }
    }

    /// Lines 2..N of a multi-line AI ghost suggestion.
    /// These are rendered below the cursor row as continuation ghost lines.
    pub fn ghost_lines_below(&self) -> Vec<String> {
        let full = match &self.ghost_full {
            Some(f) => f,
            None => return Vec::new(),
        };
        let mut lines: Vec<&str> = full.split('\n').collect();
        if lines.len() <= 1 {
            return Vec::new();
        }
        lines.remove(0); // drop first line — already in ghost_text_display()
        // Strip trailing empty line produced by a trailing '\n'.
        if lines.last().map(|l| l.is_empty()).unwrap_or(false) {
            lines.pop();
        }
        lines.into_iter().map(str::to_string).collect()
    }

    /// Whether the current ghost suggestion spans multiple lines.
    pub fn has_multiline_ghost(&self) -> bool {
        self.ghost_full
            .as_deref()
            .map(|f| f.contains('\n'))
            .unwrap_or(false)
    }

    // ═══════════════════════════════════════════════════════════════════════
    // Cycling
    // ═══════════════════════════════════════════════════════════════════════

    pub fn cycle(&mut self, dir: i32) {
        if self.merged.is_empty() {
            return;
        }
        let len = self.merged.len();
        if dir > 0 {
            self.completion_idx = (self.completion_idx + 1) % len;
        } else if self.completion_idx > 0 {
            self.completion_idx -= 1;
        } else {
            self.completion_idx = len - 1;
        }

        if let Some(c) = self.merged.get(self.completion_idx) {
            let prefix = &self.current_prefix;
            if prefix.is_empty()
                || c.text.starts_with(prefix)
                || c.source.is_ai()
            {
                self.ghost_full = Some(c.text.clone());
                let first = c.text.split('\n').next().unwrap_or(&c.text);
                self.ghost_text = Some(first.to_string());
            } else {
                self.ghost_text = None;
                self.ghost_full = None;
            }
        }
    }

    // ═══════════════════════════════════════════════════════════════════════
    // Reset / mode transitions
    // ═══════════════════════════════════════════════════════════════════════

    pub fn reset_to_idle(&mut self) {
        self.prefix_version += 1;
        self.phase = Phase::Idle;
        self.ghost_text = None;
        self.ghost_full = None;
        self.merged.clear();
        self.source_results.clear();
        self.source_pending.clear();
        self.completion_idx = 0;
    }

    pub fn set_prefix(&mut self, prefix: String) {
        self.current_prefix = prefix;
    }

    pub fn prefix(&self) -> &str {
        &self.current_prefix
    }

    pub fn current_version(&self) -> u64 {
        self.prefix_version
    }

    pub fn get_pending_request_id(&self, source: CompletionSource) -> Option<usize> {
        self.source_pending.get(&source).map(|p| p.request_id)
    }

    /// Backwards-compat: set completions directly (e.g. command-line).
    pub fn set_active(&mut self, items: Vec<String>) {
        self.source_results.clear();
        self.source_pending.clear();
        self.merged = items
            .into_iter()
            .map(|text| CompletionCandidate {
                text,
                source: CompletionSource::Manual,
                score: 0,
            })
            .collect();
        self.phase = Phase::Active;
        self.completion_idx = 0;
        self.update_ghost_text();
    }

    pub fn on_cancel(&mut self, _id: usize) {}

    pub fn on_enter_insert(&mut self) {
        self.ghost_text = None;
        self.ghost_full = None;
        self.merged.clear();
        self.completion_idx = 0;
        self.phase = Phase::Throttling;
        self.last_edit_time = std::time::Instant::now();
        self.last_ai_trigger_time = self.last_edit_time;
    }

    pub fn on_leave_insert(&mut self) {
        self.reset_to_idle();
    }

    pub fn start_pending(&mut self, _row: usize, _col: usize) {
        self.phase = Phase::Throttling;
    }

    // ═══════════════════════════════════════════════════════════════════════
    // Codeium tick poll (unchanged API, updated logic)
    // ═══════════════════════════════════════════════════════════════════════

    /// Called on every tick. Returns `Some((id, text, offset, lang, version))`
    /// when the machine decides a Codeium request should fire.
    ///
    /// AI idle-debounce is applied here rather than inside `on_edit`, so
    /// that the caller can fire Codeium/Copilot from a single poll site.
    pub fn maybe_take_request(
        &mut self,
        rope_text: String,
        rope_len_chars: usize,
        line_text: String,
        cursor_char_offset: usize,
        filename: Option<&str>,
        mode: Mode,
        row: usize,
        col: usize,
    ) -> Option<(usize, String, usize, String, u64)> {
        if mode != Mode::Insert && mode != Mode::Brief {
            return None;
        }
        // Only fire if Codeium source is pending.
        if !self.source_pending.contains_key(&CompletionSource::Codeium) {
            return None;
        }
        // Enforce the AI idle debounce.
        if self.last_ai_trigger_time.elapsed()
            < std::time::Duration::from_millis(self.ai_idle_ms)
        {
            return None;
        }
        if !self.context_allows(&line_text, rope_len_chars, mode, row, col) {
            return None;
        }

        if let Some(pending) = self.source_pending.remove(&CompletionSource::Codeium) {
            self.last_ai_request_version = self.prefix_version;
            self.phase = Phase::Pending { row, col };
            Some((
                pending.request_id,
                rope_text,
                cursor_char_offset,
                detect_language(filename),
                pending.version,
            ))
        } else {
            None
        }
    }

    /// Called when a Codeium response arrives.
    pub fn on_response(&mut self, items: Vec<String>, version: u64) {
        self.merge_source(CompletionSource::Codeium, items, version);
    }

    fn context_allows(
        &self,
        _line_text: &str,
        rope_len_chars: usize,
        _mode: Mode,
        _row: usize,
        _col: usize,
    ) -> bool {
        rope_len_chars > 1
    }

    // ═══════════════════════════════════════════════════════════════════════
    // Accept
    // ═══════════════════════════════════════════════════════════════════════

    /// Accept the current ghost text.
    /// Returns `None` if nothing is active.
    pub fn accept(&mut self, buf: &Buffer, row: usize, col: usize) -> Option<AcceptResult> {
        // Prefer the full multi-line ghost for AI sources.
        let ghost = self.ghost_full.take().or_else(|| self.ghost_text.take())?;
        self.ghost_text = None;

        let line = buf.line_text(row);
        let before: String = line.chars().take(col).collect();
        let after: String = line.chars().skip(col).collect();

        let prefix_overlap = find_prefix_overlap(&before, &ghost);
        let ghost_suffix: String = ghost.chars().skip(prefix_overlap).collect();

        // For multi-line text, only try to overlap the first line's suffix.
        let first_ghost_line = ghost_suffix.split('\n').next().unwrap_or(&ghost_suffix);
        let overlap = common_prefix_len(&after, first_ghost_line);
        let to_insert: String = ghost_suffix.chars().skip(overlap).collect();

        self.phase = Phase::Idle;
        self.merged.clear();
        self.completion_idx = 0;

        Some(AcceptResult {
            to_insert,
            insert_offset: buf.rope.line_to_char(row) + col,
            advance_past: overlap,
            is_multiline: ghost_suffix.contains('\n'),
        })
    }
}

// ---------------------------------------------------------------------------
// AcceptResult
// ---------------------------------------------------------------------------

pub struct AcceptResult {
    pub to_insert: String,
    pub insert_offset: usize,
    pub advance_past: usize,
    /// True when `to_insert` spans more than one line (AI suggestion).
    pub is_multiline: bool,
}

// ---------------------------------------------------------------------------
// Public helpers
// ---------------------------------------------------------------------------

/// Find the length of the longest suffix of `prefix` that is also a prefix
/// of `completion`.
pub fn find_prefix_overlap(prefix: &str, completion: &str) -> usize {
    let pc: Vec<char> = prefix.chars().collect();
    let cc: Vec<char> = completion.chars().collect();
    for i in 0..pc.len() {
        if cc.starts_with(&pc[i..]) {
            return pc.len() - i;
        }
    }
    0
}

fn common_prefix_len(a: &str, b: &str) -> usize {
    a.chars().zip(b.chars()).take_while(|(x, y)| x == y).count()
}
