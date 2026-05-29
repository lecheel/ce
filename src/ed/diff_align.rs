use crate::ed::buffer::VirtualLine;

/// Parallel virtual line maps for a side-by-side diff.
#[derive(Debug, Clone)]
pub struct DiffAlignment {
    /// Virtual lines for the LEFT pane (HEAD / original).
    pub left: Vec<VirtualLine>,
    /// Virtual lines for the RIGHT pane (working copy).
    pub right: Vec<VirtualLine>,
}

impl DiffAlignment {
    /// Build alignment maps from a git2 patch.
    ///
    /// `old_line_count` — number of lines in the HEAD blob.
    /// `new_line_count` — number of lines in the working copy.
    pub fn from_patch(patch: &git2::Patch, old_line_count: usize, new_line_count: usize) -> Self {
        let mut left: Vec<VirtualLine> = Vec::new();
        let mut right: Vec<VirtualLine> = Vec::new();

        let mut old_cursor = 0usize;
        let mut new_cursor = 0usize;

        let num_hunks = patch.num_hunks();

        for hunk_idx in 0..num_hunks {
            let hunk = match patch.hunk(hunk_idx) {
                Ok(h) => h.0,
                Err(_) => continue,
            };

            // 1-based → 0-based
            let old_start = (hunk.old_start() as usize).saturating_sub(1);
            let new_start = (hunk.new_start() as usize).saturating_sub(1);

            // ── Flush unchanged context lines that precede this hunk ──
            // For well-formed patches with context_lines(0), the number of
            // context lines on both sides is always identical between hunks.
            // We use the minimum as a safety clamp for edge cases.
            let old_context = old_start.saturating_sub(old_cursor);
            let new_context = new_start.saturating_sub(new_cursor);
            let context_count = old_context.min(new_context);

            for i in 0..context_count {
                left.push(VirtualLine::Real(old_cursor + i));
                right.push(VirtualLine::Real(new_cursor + i));
            }
            old_cursor += context_count;
            new_cursor += context_count;

            // Handle any residual context lines on one side only.
            // This can happen with unusual patch formats or when git
            // reports slightly different start positions. We pad the
            // shorter side rather than dropping lines.
            if old_context > context_count {
                for i in context_count..old_context {
                    left.push(VirtualLine::Real(old_cursor + i - context_count));
                    right.push(VirtualLine::Padding);
                }
                old_cursor += old_context - context_count;
            }
            if new_context > context_count {
                for i in context_count..new_context {
                    left.push(VirtualLine::Padding);
                    right.push(VirtualLine::Real(new_cursor + i - context_count));
                }
                new_cursor += new_context - context_count;
            }

            // ── Process hunk lines ──
            let num_lines = patch.num_lines_in_hunk(hunk_idx).unwrap_or(0);

            // Collect change lines into blocks, flushing when we hit
            // context or the end of the hunk.
            let mut del_lines: Vec<usize> = Vec::new();
            let mut ins_lines: Vec<usize> = Vec::new();
            let mut hunk_old = old_cursor;
            let mut hunk_new = new_cursor;

            for line_idx in 0..num_lines {
                if let Ok(line) = patch.line_in_hunk(hunk_idx, line_idx) {
                    match line.origin() {
                        '-' => {
                            del_lines.push(hunk_old);
                            hunk_old += 1;
                        }
                        '+' => {
                            ins_lines.push(hunk_new);
                            hunk_new += 1;
                        }
                        ' ' => {
                            // Flush pending change block before the context line
                            flush_change_block(
                                &mut del_lines,
                                &mut ins_lines,
                                &mut left,
                                &mut right,
                            );
                            left.push(VirtualLine::Real(hunk_old));
                            right.push(VirtualLine::Real(hunk_new));
                            hunk_old += 1;
                            hunk_new += 1;
                        }
                        _ => {}
                    }
                }
            }
            // Flush any trailing change block at the end of the hunk
            flush_change_block(&mut del_lines, &mut ins_lines, &mut left, &mut right);

            old_cursor = hunk_old;
            new_cursor = hunk_new;
        }

        // ── Flush remaining unchanged lines after the last hunk ──
        let old_remaining = old_line_count.saturating_sub(old_cursor);
        let new_remaining = new_line_count.saturating_sub(new_cursor);

        for i in 0..old_remaining {
            left.push(VirtualLine::Real(old_cursor + i));
        }
        for i in 0..new_remaining {
            right.push(VirtualLine::Real(new_cursor + i));
        }

        // Pad the shorter side so both vecs have the same length.
        // Trailing lines after the last hunk don't need per-line pairing;
        // we just need equal-length vectors for the renderer.
        let max_len = left.len().max(right.len());
        while left.len() < max_len {
            left.push(VirtualLine::Padding);
        }
        while right.len() < max_len {
            right.push(VirtualLine::Padding);
        }

        debug_assert_eq!(
            left.len(),
            right.len(),
            "DiffAlignment: left/right length mismatch after build"
        );

        Self { left, right }
    }

    /// Total number of virtual rows (identical for both sides after alignment).
    pub fn len(&self) -> usize {
        self.left.len()
    }

    pub fn is_empty(&self) -> bool {
        self.left.is_empty()
    }

    // ═══════════════════════════════════════════════════════════════
    // Lookup methods for cursor synchronization
    // ═══════════════════════════════════════════════════════════════

    /// Find the virtual row index where a real rope row first appears
    /// in the given map.
    pub fn virtual_row_for_real(map: &[VirtualLine], real_row: usize) -> Option<usize> {
        map.iter()
            .position(|vl| matches!(vl, VirtualLine::Real(r) if *r == real_row))
    }

    /// Given a real row on one side, find the best corresponding real
    /// row on the other side for cursor synchronization in diff view.
    ///
    /// `active_map` — the virtual-line map for the side the cursor is on.
    /// `sibling_map` — the virtual-line map for the side we want to sync TO.
    /// `real_row` — the cursor's rope row on the active side.
    ///
    /// Returns the nearest real row on the sibling side, searching
    /// outward from the corresponding virtual row. Prefers searching
    /// upward (earlier in the file) when distances are equal.
    pub fn sibling_real_row(
        active_map: &[VirtualLine],
        sibling_map: &[VirtualLine],
        real_row: usize,
    ) -> usize {
        // Find the virtual row where the active side has this real row
        let vrow = Self::virtual_row_for_real(active_map, real_row);

        if let Some(vrow) = vrow {
            // Check the exact virtual row first (common case: both sides
            // have real lines here).
            if let Some(VirtualLine::Real(r)) = sibling_map.get(vrow) {
                return *r;
            }

            // Search outward from vrow for the nearest real line on the
            // sibling side. Prefer upward (vrow - delta) over downward
            // (vrow + delta) to keep the cursor near the start of change
            // blocks, matching traditional diff-viewer behavior.
            let max_delta = sibling_map.len().saturating_sub(1);
            for delta in 1..=max_delta {
                // Check above
                if vrow >= delta {
                    if let Some(VirtualLine::Real(r)) = sibling_map.get(vrow - delta) {
                        return *r;
                    }
                }
                // Check below
                if vrow + delta < sibling_map.len() {
                    if let Some(VirtualLine::Real(r)) = sibling_map.get(vrow + delta) {
                        return *r;
                    }
                }
            }
        }

        // Fallback: return the last real row on the sibling side
        sibling_map
            .iter()
            .rev()
            .find_map(|vl| match vl {
                VirtualLine::Real(r) => Some(*r),
                _ => None,
            })
            .unwrap_or(0)
    }
}

// ---------------------------------------------------------------------------
// Private helpers
// ---------------------------------------------------------------------------

/// Pair up accumulated deletion and insertion lines, padding the
/// shorter side so both columns have the same number of virtual rows.
fn flush_change_block(
    del: &mut Vec<usize>,
    ins: &mut Vec<usize>,
    left: &mut Vec<VirtualLine>,
    right: &mut Vec<VirtualLine>,
) {
    let max_len = del.len().max(ins.len());
    for i in 0..max_len {
        match (del.get(i), ins.get(i)) {
            (Some(&o), Some(&n)) => {
                // Modified line: both sides have content
                left.push(VirtualLine::Real(o));
                right.push(VirtualLine::Real(n));
            }
            (Some(&o), None) => {
                // Deleted line: left has content, right is padding
                left.push(VirtualLine::Real(o));
                right.push(VirtualLine::Padding);
            }
            (None, Some(&n)) => {
                // Inserted line: left is padding, right has content
                left.push(VirtualLine::Padding);
                right.push(VirtualLine::Real(n));
            }
            (None, None) => unreachable!(),
        }
    }
    del.clear();
    ins.clear();
}
