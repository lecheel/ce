//! Completion state machine (ghost-text).
//!
//! `CompletionMachine` is the single owner of all completion lifecycle.
//! The editor calls one method per event; the machine decides what to do.
//!
//! NOTE: Because cursor state now lives on `Window` rather than `Buffer`,
//! methods that need cursor position (`maybe_take_request`, `accept`,
//! `context_allows`) receive `row`/`col` as explicit parameters.

use crate::ed::buffer::{detect_language, Buffer};
use crate::ed::mode::Mode;
use ratatui::style::Color;
use std::collections::HashMap;

/// Identifies which source provided a completion candidate.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CompletionSource {
    /// External Language Server Protocol response.
    Lsp,
    /// Words scanned from the current buffer's rope.
    BufferWords,
    /// User-configured vocabulary wordlist.
    VocabWords,
    /// Manual Alt+/ trigger (merges buffer + vocab).
    Manual,
    FilePaths,
}

impl CompletionSource {
    /// Short label for the popup badge.
    pub fn badge(&self) -> &'static str {
        match self {
            Self::Lsp => "LSP",
            Self::BufferWords => "BUF",
            Self::VocabWords => "VOC",
            Self::Manual => "M",
            CompletionSource::FilePaths => "Path",
        }
    }

    /// Display color for the badge.
    pub fn badge_color(&self) -> Color {
        match self {
            Self::Lsp => Color::Cyan,
            Self::BufferWords => Color::Green,
            Self::VocabWords => Color::Magenta,
            Self::Manual => Color::Yellow,
            CompletionSource::FilePaths => Color::Cyan,
        }
    }
}

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
    /// The prefix_version this result was computed for.
    /// If it doesn't match the current version, it's stale.
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
// Phase — drives all guard logic; illegal states are unrepresentable
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Phase {
    Idle,
    Throttling,
    Pending { row: usize, col: usize },
    Active,
}

// ---------------------------------------------------------------------------
// CompletionMachine
// ---------------------------------------------------------------------------

#[derive(Debug)]
pub struct CompletionMachine {
    // ── Existing fields (unchanged) ──────────────────────────────
    pub phase: Phase,
    pub last_edit_time: std::time::Instant,
    pub throttle_ms: u64,
    pub request_id: usize,
    pub ghost_text: Option<String>,
    pub completion_idx: usize,

    // ── NEW: Multi-source aggregation ───────────────────────────
    /// Monotonically increasing version, bumped on every edit.
    /// Any completion result with a stale version is discarded.
    prefix_version: u64,

    /// Per-source result buckets.
    source_results: HashMap<CompletionSource, SourceBucket>,

    /// Per-source pending request state.
    source_pending: HashMap<CompletionSource, SourcePending>,

    /// The merged, deduplicated, ranked candidate list.
    /// Rebuilt from source_results after any merge.
    merged: Vec<CompletionCandidate>,

    /// The current word prefix being completed.
    current_prefix: String,
    // ── Backwards-compat accessors ──────────────────────────────
    // These are derived from `merged` so existing code keeps working.
    // completions() → merged.iter().map(|c| &c.text).collect()
    // ghost_text    → derived from merged[0] if non-empty
}

impl Default for CompletionMachine {
    fn default() -> Self {
        Self::new()
    }
}

impl CompletionMachine {
    pub fn new() -> Self {
        Self {
            phase: Phase::Idle,
            last_edit_time: std::time::Instant::now(),
            throttle_ms: 400,
            request_id: 0,
            ghost_text: None,
            completion_idx: 0,
            prefix_version: 0,
            source_results: HashMap::new(),
            source_pending: HashMap::new(),
            merged: Vec::new(),
            current_prefix: String::new(),
        }
    }

    /// Called on every edit. Bumps version, clears stale state.
    pub fn on_edit(&mut self) {
        self.prefix_version += 1;
        self.phase = Phase::Throttling;
        self.ghost_text = None;
        self.completion_idx = 0;
        self.source_results.clear();
        self.source_pending.clear();
        self.merged.clear();
        self.current_prefix.clear();
    }

    /// Register that a source has started a request.
    /// Returns the (request_id, version) pair the source must
    /// include in its response for validation.
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

    /// Merge results from a specific source.
    ///
    /// **Version-gated**: if `version` doesn't match the current
    /// `prefix_version`, the results are stale and silently discarded.
    ///
    /// **Non-destructive**: only the slot for `source` is updated;
    /// other sources' results remain intact.
    pub fn merge_source(&mut self, source: CompletionSource, items: Vec<String>, version: u64) {
        // ── Version gate: discard stale results ──────────────────
        if version != self.prefix_version {
            log::debug!(
                "[comp:merge] DROPPED {} results from {:?}: \
                 stale version {} != current {}",
                items.len(),
                source,
                version,
                self.prefix_version,
            );
            return;
        }

        log::debug!(
            "[comp:merge] MERGED {} results from {:?} (version={})",
            items.len(),
            source,
            version,
        );

        // Store in the source's bucket
        self.source_results
            .insert(source, SourceBucket { version, items });

        // Remove from pending
        self.source_pending.remove(&source);

        // Rebuild the merged list
        self.rebuild_merged();

        // Update phase
        if !self.merged.is_empty() {
            self.phase = Phase::Active;
            self.update_ghost_text();
        } else if self.source_pending.is_empty() {
            // All sources returned, nothing found
            self.phase = Phase::Idle;
            self.ghost_text = None;
        }
    }

    /// Rebuild the merged candidate list from all source buckets.
    ///
    /// Deduplicates by text (keeping the highest-priority source),
    /// sorts by score (shorter = better), and updates ghost text.
    fn rebuild_merged(&mut self) {
        let prefix = &self.current_prefix;
        let version = self.prefix_version;

        let mut seen: HashMap<String, CompletionSource> = HashMap::new();
        let mut candidates: Vec<CompletionCandidate> = Vec::new();

        // Source priority: Lsp > Manual > BufferWords > VocabWords
        let source_priority = |s: CompletionSource| -> usize {
            match s {
                CompletionSource::Lsp => 0,
                CompletionSource::Manual => 1,
                CompletionSource::FilePaths => 2,
                CompletionSource::BufferWords => 3,
                CompletionSource::VocabWords => 4,
            }
        };

        for (source, bucket) in &self.source_results {
            if bucket.version != version {
                continue; // stale
            }

            for text in &bucket.items {
                // Skip exact prefix match (no point completing "apple" with "apple")
                if text == prefix {
                    continue;
                }

                let score = text.len(); // shorter = better

                if let Some(existing_source) = seen.get(text) {
                    // Keep the higher-priority source
                    if source_priority(*source) < source_priority(*existing_source) {
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

        // Sort: shortest first (best match), then alphabetically
        candidates.sort_by(|a, b| a.score.cmp(&b.score).then_with(|| a.text.cmp(&b.text)));

        self.merged = candidates;

        // Clamp selection
        if self.completion_idx >= self.merged.len() {
            self.completion_idx = 0;
        }

        log::debug!(
            "[comp:rebuild] merged {} candidates from {} sources",
            self.merged.len(),
            self.source_results.len(),
        );
    }

    /// Update ghost text from the first (best) merged candidate.
    fn update_ghost_text(&mut self) {
        if self.merged.is_empty() {
            self.ghost_text = None;
            return;
        }

        let prefix = &self.current_prefix;
        let best = &self.merged[0].text;

        if best.starts_with(prefix) && !prefix.is_empty() {
            self.ghost_text = Some(best.clone());
        } else {
            self.ghost_text = None;
        }
    }

    // ── Backwards-compatible accessors ───────────────────────────

    /// All merged candidate texts (for the popup).
    pub fn completions(&self) -> Vec<String> {
        self.merged.iter().map(|c| c.text.clone()).collect()
    }

    /// Merged candidates with source info (for the enhanced popup).
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

        // Sync inline ghost text with the popup selection
        if let Some(c) = self.merged.get(self.completion_idx) {
            if c.text.starts_with(&self.current_prefix) && !self.current_prefix.is_empty() {
                self.ghost_text = Some(c.text.clone());
            } else {
                // Non-prefix match (e.g., fuzzy). We cannot render this inline
                // without replacing the existing word, which ratatui doesn't
                // visually support. Hide inline ghost; popup keeps selection.
                self.ghost_text = None;
            }
        }
    }

    pub fn reset_to_idle(&mut self) {
        self.phase = Phase::Idle;
        self.ghost_text = None;
        self.merged.clear();
        self.source_results.clear();
        self.source_pending.clear();
        self.completion_idx = 0;
    }

    /// Sets the current prefix being completed.
    pub fn set_prefix(&mut self, prefix: String) {
        self.current_prefix = prefix;
    }

    /// Gets the current prefix version.
    pub fn current_version(&self) -> u64 {
        self.prefix_version
    }

    /// Gets the pending request ID for a specific source.
    pub fn get_pending_request_id(&self, source: CompletionSource) -> Option<usize> {
        self.source_pending.get(&source).map(|p| p.request_id)
    }

    // ── Kept for backwards compat (command completion) ──────────
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

    pub fn on_cancel(&mut self, _id: usize) {
        // No-op in v2; version gating handles stale requests
    }

    /// Call whenever the editor enters Insert mode.
    pub fn on_enter_insert(&mut self) {
        self.ghost_text = None;
        self.merged.clear();
        self.completion_idx = 0;
        self.phase = Phase::Throttling;
        self.last_edit_time = std::time::Instant::now();
    }

    pub fn on_leave_insert(&mut self) {
        self.reset_to_idle();
    }

    /// Legacy method — use start_source_request instead.
    pub fn start_pending(&mut self, _row: usize, _col: usize) {
        self.phase = Phase::Throttling;
    }

    // -----------------------------------------------------------------------
    // Tick → machine
    // -----------------------------------------------------------------------

    /// Called on every tick. Returns `Some((id, full_text, offset, lang))` when
    /// the machine decides a request should be fired; `None` otherwise.
    ///
    /// **row** and **col** are the cursor position from the active `Window`.
    pub fn maybe_take_request(
        &mut self,
        buf: &Buffer,
        mode: Mode,
        row: usize,
        col: usize,
    ) -> Option<(usize, String, usize, String)> {
        if mode != Mode::Insert && mode != Mode::Brief {
            return None;
        }
        if !matches!(self.phase, Phase::Throttling) {
            return None;
        }
        if self.last_edit_time.elapsed() < std::time::Duration::from_millis(self.throttle_ms) {
            return None;
        }
        if !self.context_allows(buf, mode, row, col) {
            return None;
        }

        // Transition: Throttling → Pending
        self.request_id += 1;
        self.phase = Phase::Pending { row, col };

        let filename = buf.filename.clone();
        Some((
            self.request_id,
            buf.rope.to_string(),
            buf.rope.line_to_char(row) + col, // cursor_char_offset
            detect_language(filename.as_deref()),
        ))
    }

    /// Call when a completion response arrives from the async provider task.
    pub fn on_response(
        &mut self,
        id: usize,
        items: Vec<String>,
        buf: &Buffer,
        mode: Mode,
        row: usize,
        col: usize,
    ) {
        if id != self.request_id {
            log::debug!(
                "dropped stale response (got={id}, want={})",
                self.request_id
            );
            return;
        }
        // Re-check context at current cursor position
        if !self.context_allows(buf, mode, row, col) {
            self.phase = Phase::Idle;
            return;
        }
        if items.is_empty() {
            self.phase = Phase::Idle;
            return;
        }

        self.merged = items
            .into_iter()
            .map(|text| {
                let score = text.len();
                CompletionCandidate {
                    text,
                    source: CompletionSource::Lsp,
                    score,
                }
            })
            .collect();

        self.completion_idx = 0;
        self.update_ghost_text();
        self.phase = Phase::Active;
    }

    /// Accept the current ghost text.
    ///
    /// **row** and **col** come from the active `Window`.
    /// Accept the current ghost text.
    ///
    /// **row** and **col** come from the active `Window`.
    pub fn accept(&mut self, buf: &Buffer, row: usize, col: usize) -> Option<AcceptResult> {
        let ghost = self.ghost_text.take()?;

        let line = buf.line_text(row);
        let before: String = line.chars().take(col).collect();
        let after: String = line.chars().skip(col).collect();

        let prefix_overlap = find_prefix_overlap(&before, &ghost);
        let ghost_suffix: String = ghost.chars().skip(prefix_overlap).collect();

        let overlap = common_prefix_len(&after, &ghost_suffix);
        let to_insert: String = ghost_suffix.chars().skip(overlap).collect();

        self.phase = Phase::Idle;
        self.merged.clear(); // Changed from self.completions.clear()
        self.completion_idx = 0;

        Some(AcceptResult {
            to_insert,
            insert_offset: buf.rope.line_to_char(row) + col,
            advance_past: overlap,
        })
    }

    // -----------------------------------------------------------------------
    // Private helpers
    // -----------------------------------------------------------------------
    /// Returns false when contextual clues suggest a request would be useless.
    fn context_allows(&self, buf: &Buffer, mode: Mode, row: usize, col: usize) -> bool {
        if buf.rope.len_chars() <= 1 {
            return false;
        }
        let line = buf.line_text(row);
        let chars: Vec<char> = line.chars().collect();
        let c = col.min(chars.len());

        if c < chars.len() {
            let next = chars[c];
            if next.is_alphanumeric() || next == '_' || next == ')' {
                return false;
            }
        }
        if c > 0 && chars[c - 1] == ')' {
            return false;
        }

        // ── MINIMUM PREFIX LENGTH GUARD ──────────────────────────────
        // Prevent ghost text from aggressively popping up on the very
        // first keystroke.
        // - Brief mode requires 3 chars (commands are single keys).
        // - Vim Insert mode requires 2 chars (standard editor behavior).
        let min_prefix = match mode {
            Mode::Brief => 4,
            Mode::Insert => 4,
            _ => return true, // Other modes don't trigger completion typing
        };

        let mut prefix_len = 0;
        let mut i = c;
        while i > 0 {
            let ch = chars[i - 1];
            if ch.is_alphanumeric() || ch == '_' {
                prefix_len += 1;
                i -= 1;
            } else {
                break;
            }
        }

        if prefix_len < min_prefix {
            return false;
        }

        true
    }
}

// ---------------------------------------------------------------------------
// AcceptResult
// ---------------------------------------------------------------------------

pub struct AcceptResult {
    /// Text to insert at `insert_offset`.
    pub to_insert: String,
    /// Absolute rope char offset where insertion should happen.
    pub insert_offset: usize,
    /// Number of chars after the cursor that the ghost already matched.
    pub advance_past: usize,
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
