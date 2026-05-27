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
    phase: Phase,

    /// Currently displayed ghost text (first or cycled candidate).
    pub ghost_text: Option<String>,
    /// All candidates from the last provider response.
    pub(crate) completions: Vec<String>,
    /// Which candidate is currently displayed.
    pub(crate) completion_idx: usize,

    /// Monotone request counter — responses with a mismatched ID are dropped.
    pub request_id: usize,

    pub(crate) throttle_ms: u64,
    pub(crate) last_edit_time: std::time::Instant,
}

impl Default for CompletionMachine {
    fn default() -> Self {
        Self::new()
    }
}

impl CompletionMachine {
    pub fn new() -> Self {
        Self {
            phase: Phase::Throttling,
            ghost_text: None,
            completions: Vec::new(),
            completion_idx: 0,
            request_id: 0,
            throttle_ms: 400,
            last_edit_time: std::time::Instant::now() - std::time::Duration::from_secs(10),
        }
    }

    /// Check if completion machine is in throttling phase
    pub fn is_throttling(&self) -> bool {
        matches!(self.phase, Phase::Throttling)
    }

    /// Transition to pending phase with location data
    pub fn start_pending(&mut self, row: usize, col: usize) {
        self.request_id += 1;
        self.phase = Phase::Pending { row, col };
    }

    /// Transition to idle phase and clear completions
    pub fn reset_to_idle(&mut self) {
        self.phase = Phase::Idle;
        self.completions.clear();
        self.completion_idx = 0;
    }

    // -----------------------------------------------------------------------
    // Editor → machine events
    // -----------------------------------------------------------------------

    /// Call on every buffer-mutating keystroke while in Insert mode.
    pub fn on_edit(&mut self) {
        self.last_edit_time = std::time::Instant::now();
        self.ghost_text = None;
        self.completions.clear();
        self.completion_idx = 0;
        if !matches!(self.phase, Phase::Pending { .. }) {
            self.phase = Phase::Throttling;
        }
    }

    /// Call whenever the editor enters Insert mode.
    pub fn on_enter_insert(&mut self) {
        self.ghost_text = None;
        self.completions.clear();
        self.completion_idx = 0;
        self.phase = Phase::Throttling;
        self.last_edit_time = std::time::Instant::now();
    }

    /// Call whenever the editor leaves Insert mode.
    pub fn on_leave_insert(&mut self) {
        self.ghost_text = None;
        self.completions.clear();
        self.completion_idx = 0;
        self.phase = Phase::Idle;
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
        self.completions = items;
        self.completion_idx = 0;
        self.ghost_text = self.completions.first().cloned();
        self.phase = Phase::Active;
    }

    /// Call when a provider task fails before it could send a response.
    pub fn on_cancel(&mut self, id: usize) {
        if id == self.request_id {
            self.phase = Phase::Idle;
        }
    }

    // -----------------------------------------------------------------------
    // Ghost-text interaction
    // -----------------------------------------------------------------------

    /// Cycle to the next (+1) or previous (-1) completion candidate.
    pub fn cycle(&mut self, direction: i32) {
        if self.completions.is_empty() {
            return;
        }
        let len = self.completions.len();
        self.completion_idx = if direction > 0 {
            (self.completion_idx + 1) % len
        } else if self.completion_idx == 0 {
            len - 1
        } else {
            self.completion_idx - 1
        };
        self.ghost_text = Some(self.completions[self.completion_idx].clone());
        self.last_edit_time = std::time::Instant::now();
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

        // Dynamically compute prefix overlap using the full candidate string
        let prefix_overlap = find_prefix_overlap(&before, &ghost);
        let ghost_suffix: String = ghost.chars().skip(prefix_overlap).collect();

        let overlap = common_prefix_len(&after, &ghost_suffix);
        let to_insert: String = ghost_suffix.chars().skip(overlap).collect();

        self.phase = Phase::Idle;
        self.completions.clear();
        self.completion_idx = 0;

        Some(AcceptResult {
            to_insert,
            insert_offset: buf.rope.line_to_char(row) + col,
            advance_past: overlap,
        })
    }

    // -----------------------------------------------------------------------
    // Read accessors
    // -----------------------------------------------------------------------

    pub fn ghost_text(&self) -> Option<&str> {
        self.ghost_text.as_deref()
    }

    pub fn completions(&self) -> &[String] {
        &self.completions
    }

    pub fn completion_idx(&self) -> usize {
        self.completion_idx
    }

    pub fn has_ghost(&self) -> bool {
        self.ghost_text.is_some()
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
    pub fn set_active(&mut self, items: Vec<String>) {
        self.completions = items;
        self.completion_idx = 0;
        self.ghost_text = self.completions.first().cloned();
        self.phase = Phase::Active;
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
