//! Window — an independent viewport into a Buffer.
//!
//! …existing docs…
//!
//! # Layout tree
//!
//! [`LayoutNode`] is a binary tree that describes how windows are
//! arranged on screen.  Each leaf references a window ID; each inner
//! `Split` node divides its rectangle between two sub-trees.  The
//! render loop calls [`LayoutNode::compute_positions`] with the
//! available terminal area to obtain a `(window_id, WindowPosition)`
//! pair for every leaf.

use crate::ed::buffer::Buffer;
use crate::ed::Mode;

// ---------------------------------------------------------------------------
// WindowPosition — layout rectangle within the terminal
// ---------------------------------------------------------------------------

/// Layout rectangle for a window inside the terminal grid.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WindowPosition {
    pub x: usize,
    pub y: usize,
    pub width: usize,
    pub height: usize,
}

impl Default for WindowPosition {
    fn default() -> Self {
        Self {
            x: 0,
            y: 0,
            width: 0,
            height: 0,
        }
    }
}

impl WindowPosition {
    pub fn new(x: usize, y: usize, width: usize, height: usize) -> Self {
        Self {
            x,
            y,
            width,
            height,
        }
    }

    /// Whether the position has a non-zero area.
    pub fn is_visible(&self) -> bool {
        self.width > 0 && self.height > 0
    }

    /// True when `self` and `other` overlap along the vertical axis.
    pub fn overlaps_vertically(&self, other: &WindowPosition) -> bool {
        self.y < other.y + other.height && other.y < self.y + self.height
    }

    /// True when `self` and `other` overlap along the horizontal axis.
    pub fn overlaps_horizontally(&self, other: &WindowPosition) -> bool {
        self.x < other.x + other.width && other.x < self.x + other.width
    }
}

// ---------------------------------------------------------------------------
// Split direction
// ---------------------------------------------------------------------------

/// Direction of a split in the layout tree.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SplitDir {
    /// Stacked top-to-bottom (horizontal divider).
    Horizontal,
    /// Side-by-side left-to-right (vertical divider).
    Vertical,
}

// ---------------------------------------------------------------------------
// LayoutNode — binary tree describing the window arrangement
// ---------------------------------------------------------------------------

/// A node in the window layout tree.
///
/// ```text
/// LayoutNode::Split { Horizontal, first=A, second=B }
/// ┌───────────┐
/// │  leaf A   │
/// ├───────────┤
/// │  leaf B   │
/// └───────────┘
/// ```
#[derive(Debug, Clone)]
pub enum LayoutNode {
    /// A single window.
    Leaf(usize), // window_id

    /// Two sub-trees separated by a divider.
    Split {
        direction: SplitDir,
        first: Box<LayoutNode>,
        second: Box<LayoutNode>,
    },
}

impl LayoutNode {
    // ---- Constructors ----

    pub fn leaf(window_id: usize) -> Self {
        LayoutNode::Leaf(window_id)
    }

    pub fn split(direction: SplitDir, first: Self, second: Self) -> Self {
        LayoutNode::Split {
            direction,
            first: Box::new(first),
            second: Box::new(second),
        }
    }

    // ---- Queries ----

    /// Number of leaf nodes (i.e. windows).
    pub fn leaf_count(&self) -> usize {
        match self {
            LayoutNode::Leaf(_) => 1,
            LayoutNode::Split { first, second, .. } => first.leaf_count() + second.leaf_count(),
        }
    }

    /// Does the tree contain a leaf with the given window ID?
    pub fn contains(&self, window_id: usize) -> bool {
        match self {
            LayoutNode::Leaf(id) => *id == window_id,
            LayoutNode::Split { first, second, .. } => {
                first.contains(window_id) || second.contains(window_id)
            }
        }
    }

    /// Collect all window IDs in the tree, depth-first.
    pub fn window_ids(&self) -> Vec<usize> {
        match self {
            LayoutNode::Leaf(id) => vec![*id],
            LayoutNode::Split { first, second, .. } => {
                let mut ids = first.window_ids();
                ids.extend(second.window_ids());
                ids
            }
        }
    }

    // ---- Mutations ----

    /// Replace the leaf `target_window_id` with a `Split` that contains
    /// both the original leaf and a new leaf `new_window_id`.
    ///
    /// Returns `true` if the target was found and split, `false` otherwise.
    pub fn split_leaf(
        &mut self,
        target_window_id: usize,
        direction: SplitDir,
        new_window_id: usize,
    ) -> bool {
        match self {
            LayoutNode::Leaf(id) if *id == target_window_id => {
                *self = LayoutNode::split(
                    direction,
                    LayoutNode::Leaf(target_window_id),
                    LayoutNode::Leaf(new_window_id),
                );
                true
            }
            LayoutNode::Split { first, second, .. } => {
                first.split_leaf(target_window_id, direction, new_window_id)
                    || second.split_leaf(target_window_id, direction, new_window_id)
            }
            _ => false,
        }
    }

    /// Remove the leaf with `target_window_id`, replacing its parent
    /// `Split` node with the sibling sub-tree.
    ///
    /// Returns `true` if a leaf was removed, `false` otherwise.
    /// Does nothing when called on a single-leaf root.
    pub fn remove_leaf(&mut self, target_window_id: usize) -> bool {
        match self {
            LayoutNode::Leaf(_) => false,
            LayoutNode::Split { first, second, .. } => {
                // Check if `first` is the target leaf → replace self with second.
                if let LayoutNode::Leaf(id) = first.as_ref() {
                    if *id == target_window_id {
                        let sibling = std::mem::replace(second.as_mut(), LayoutNode::Leaf(0));
                        *self = sibling;
                        return true;
                    }
                }
                // Check if `second` is the target leaf → replace self with first.
                if let LayoutNode::Leaf(id) = second.as_ref() {
                    if *id == target_window_id {
                        let sibling = std::mem::replace(first.as_mut(), LayoutNode::Leaf(0));
                        *self = sibling;
                        return true;
                    }
                }
                // Recurse into children.
                first.remove_leaf(target_window_id) || second.remove_leaf(target_window_id)
            }
        }
    }

    // ---- Layout computation ----

    /// Walk the tree, assigning a [`WindowPosition`] to every leaf.
    ///
    /// `separator` is the number of rows/cols consumed by a divider
    /// (typically 1).
    pub fn compute_positions(
        &self,
        area: WindowPosition,
        separator: usize,
    ) -> Vec<(usize, WindowPosition)> {
        match self {
            LayoutNode::Leaf(window_id) => {
                vec![(*window_id, area)]
            }
            LayoutNode::Split {
                direction,
                first,
                second,
            } => match direction {
                SplitDir::Horizontal => {
                    let sep = if area.height > separator {
                        separator
                    } else {
                        0
                    };
                    let available = area.height.saturating_sub(sep);
                    let first_h = available / 2;
                    let second_h = available - first_h;
                    let first_area = WindowPosition::new(area.x, area.y, area.width, first_h);
                    let second_area =
                        WindowPosition::new(area.x, area.y + first_h + sep, area.width, second_h);
                    let mut out = first.compute_positions(first_area, separator);
                    out.extend(second.compute_positions(second_area, separator));
                    out
                }
                SplitDir::Vertical => {
                    let sep = if area.width > separator { separator } else { 0 };
                    let available = area.width.saturating_sub(sep);
                    let first_w = available / 2;
                    let second_w = available - first_w;
                    let first_area = WindowPosition::new(area.x, area.y, first_w, area.height);
                    let second_area =
                        WindowPosition::new(area.x + first_w + sep, area.y, second_w, area.height);
                    let mut out = first.compute_positions(first_area, separator);
                    out.extend(second.compute_positions(second_area, separator));
                    out
                }
            },
        }
    }
}

// ---------------------------------------------------------------------------
// Window
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct Window {
    /// Unique window identifier.
    pub id: usize,

    /// ID of the buffer this window is viewing.
    buffer_id: usize,

    // ── Cursor ────────────────────────────────────────────────────
    /// 0-based cursor row (line index within the buffer).
    pub row: usize,
    /// 0-based cursor column (character offset within the line,
    /// excluding the line-break).
    pub col: usize,

    // ── Scroll ────────────────────────────────────────────────────
    pub desired_col: usize,
    /// Vertical scroll: index of the first visible line.
    pub scroll_line: usize,
    /// Horizontal scroll: first visible display column.
    pub scroll_col: usize,
    /// Sub-line vertical pixel offset for smooth scrolling.
    pub scroll_offset_y: usize,

    /// Position before the last jump — used by `` (backtick ping-pong).
    pub last_jump: Option<(usize, usize)>,

    // Link to another window currently participating in side-by-side diff comparison
    pub diff_sibling: Option<usize>,

    // ── Layout ────────────────────────────────────────────────────
    /// Position and size of this window within the terminal grid.
    pub position: WindowPosition,
    pub visual_anchor: Option<(usize, usize)>,
}

impl Window {
    // -----------------------------------------------------------------------
    // Constructor
    // -----------------------------------------------------------------------

    /// Create a new window viewing `buffer_id`, with cursor at (0, 0)
    /// and scroll at the top of the file.
    pub fn new(id: usize, buffer_id: usize) -> Self {
        Self {
            id,
            buffer_id,
            row: 0,
            col: 0,
            desired_col: 0,
            scroll_line: 0,
            scroll_col: 0,
            scroll_offset_y: 0,
            position: WindowPosition::default(),
            visual_anchor: None,
            last_jump: None,
            diff_sibling: None,
        }
    }

    /// Save current cursor position as the jump-back anchor.
    pub fn save_jump_position(&mut self) {
        self.last_jump = Some((self.row, self.col));
    }

    // -----------------------------------------------------------------------
    // Buffer-id accessors + clamping
    // -----------------------------------------------------------------------

    /// The buffer this window is currently viewing.
    #[inline]
    pub fn buffer_id(&self) -> usize {
        self.buffer_id
    }

    /// Point this window at a different buffer.
    pub fn set_buffer_id(&mut self, id: usize) {
        self.buffer_id = id;
    }

    /// Clamp `buffer_id` so it always points to a valid entry.
    pub fn clamp_buffer_id(&mut self, buffer_ids: &[usize]) {
        if buffer_ids.is_empty() {
            self.buffer_id = usize::MAX;
            return;
        }
        if buffer_ids.contains(&self.buffer_id) {
            return;
        }
        let nearest = buffer_ids
            .iter()
            .min_by_key(|&&id| (id as isize - self.buffer_id as isize).abs())
            .expect("buffer_ids is non-empty"); // safe: checked above
        self.buffer_id = *nearest;
    }

    // -----------------------------------------------------------------------
    // Cursor helpers (require a Buffer reference)
    // -----------------------------------------------------------------------

    /// Compute the absolute character offset of the cursor within the
    /// rope.
    #[inline]
    pub fn cursor_char_offset(&self, buf: &Buffer) -> usize {
        if self.row >= buf.len_lines() {
            return buf.rope.len_chars(); // end-of-file sentinel
        }
        buf.rope.line_to_char(self.row) + self.col.min(buf.line_char_len(self.row))
    }

    pub fn clamp_cursor(&mut self, buf: &Buffer) {
        if buf.len_lines() == 0 {
            self.row = 0;
            self.col = 0;
            return;
        }
        let max_row = buf.len_lines().saturating_sub(1);
        self.row = self.row.min(max_row);
        let max_col = buf.line_char_len(self.row);
        self.col = self.col.min(max_col);
    }

    // -----------------------------------------------------------------------
    // Scroll helpers
    // -----------------------------------------------------------------------

    // -----------------------------------------------------------------------
    // Scroll helpers
    // -----------------------------------------------------------------------
    // -----------------------------------------------------------------------
    // Scroll helpers
    // -----------------------------------------------------------------------

    /// Adjust `scroll_line` and `scroll_col` so the cursor is visible
    /// inside a viewport of `viewport_height` lines and `viewport_width` columns,
    /// with `margin` lines/cols of context, and `gutter_width` columns reserved.
    pub fn ensure_cursor_visible(
        &mut self,
        viewport_height: usize,
        viewport_width: usize,
        margin: usize,
        gutter_width: usize,
    ) {
        if viewport_height == 0 || viewport_width == 0 {
            return;
        }

        // ── Vertical scrolling ──────────────────────────────────────
        if self.row < self.scroll_line + margin {
            self.scroll_line = self.row.saturating_sub(margin);
        }
        if self.row >= self.scroll_line + viewport_height - margin {
            self.scroll_line = self.row + margin + 1 - viewport_height;
        }

        // ── Horizontal scrolling (accounts for gutter) ─────────────
        let text_width = viewport_width.saturating_sub(gutter_width);
        if text_width == 0 {
            return;
        }

        if self.col < self.scroll_col {
            self.scroll_col = self.col.saturating_sub(margin);
        } else if self.col >= self.scroll_col + text_width {
            self.scroll_col = self.col - text_width + 1 + margin;
        }
    }

    /// Reset scroll to center the cursor in the viewport.
    pub fn scroll_to_cursor(
        &mut self,
        viewport_height: usize,
        viewport_width: usize,
        gutter_width: usize,
    ) {
        self.scroll_line = if self.row >= viewport_height {
            self.row - viewport_height / 2
        } else {
            0
        };

        let text_width = viewport_width.saturating_sub(gutter_width);
        self.scroll_col = if text_width > 0 && self.col >= text_width {
            self.col - text_width / 2
        } else {
            0
        };

        self.scroll_offset_y = 0;
    }

    // -----------------------------------------------------------------------
    // Position / layout
    // -----------------------------------------------------------------------

    /// Update the layout rectangle.
    pub fn set_position(&mut self, pos: WindowPosition) {
        self.position = pos;
    }

    // -----------------------------------------------------------------------
    // Convenience: delegate common Buffer queries through the Window
    // -----------------------------------------------------------------------

    #[inline]
    pub fn line_text(&self, buf: &Buffer, idx: usize) -> String {
        buf.line_text(idx)
    }

    #[inline]
    pub fn line_char_len(&self, buf: &Buffer, idx: usize) -> usize {
        buf.line_char_len(idx)
    }

    #[inline]
    pub fn current_line_text(&self, buf: &Buffer) -> String {
        buf.line_text(self.row)
    }

    #[inline]
    pub fn current_line_char_len(&self, buf: &Buffer) -> usize {
        buf.line_char_len(self.row)
    }

    /// Calculates the start and end character offsets of the active selection.
    pub fn get_selection_range(&self, buf: &Buffer, mode: Mode) -> Option<(usize, usize)> {
        let anchor = self.visual_anchor?;
        let cursor = (self.row, self.col);

        let (start, end) = if anchor.0 < cursor.0 || (anchor.0 == cursor.0 && anchor.1 <= cursor.1)
        {
            (anchor, cursor)
        } else {
            (cursor, anchor)
        };

        if mode == Mode::VisualLine {
            let start_char = buf.rope.line_to_char(start.0);
            let end_char = buf.rope.line_to_char(end.0) + buf.line_char_len(end.0) + 1; // Include full line
            Some((start_char, end_char.min(buf.rope.len_chars())))
        } else {
            let start_char =
                buf.rope.line_to_char(start.0) + start.1.min(buf.line_char_len(start.0));
            let end_char = buf.rope.line_to_char(end.0) + end.1.min(buf.line_char_len(end.0)) + 1; // Inclusive
            Some((start_char, end_char.min(buf.rope.len_chars())))
        }
    }
}
