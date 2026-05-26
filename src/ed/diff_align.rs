// ed/diff_align.rs

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

            // Flush unchanged context lines that precede this hunk.
            // Use old_start as ground truth; new_start may differ due to
            // prior insertions/deletions — context content is identical.
            let context_count = old_start.saturating_sub(old_cursor);
            for i in 0..context_count {
                let ol = old_cursor + i;
                let nl = new_cursor + i;
                left.push(VirtualLine::Real(ol));
                if nl < new_line_count {
                    right.push(VirtualLine::Real(nl));
                } else {
                    right.push(VirtualLine::Padding);
                }
            }
            old_cursor += context_count;
            new_cursor += context_count;

            // Sanity-clamp: if the hunk header says new_start is further
            // ahead than our new_cursor accounts for, fast-forward.
            // This can happen on the very first hunk when the file starts
            // with unchanged lines that git omits from the header counts.
            if new_start > new_cursor {
                let extra = new_start - new_cursor;
                // These were already emitted above via old_start; if there
                // is still a gap on the new side we need to fill it.
                for i in 0..extra {
                    let nl = new_cursor + i;
                    if nl < new_line_count {
                        // pair with a padding on the left because old_cursor
                        // has already moved past these
                        left.push(VirtualLine::Padding);
                        right.push(VirtualLine::Real(nl));
                    }
                }
                new_cursor += extra;
            }

            // Collect deleted and inserted lines for this hunk separately
            // so we can pair them up and pad the shorter side.
            let num_lines = patch.num_lines_in_hunk(hunk_idx).unwrap_or(0);

            let mut del_lines: Vec<usize> = Vec::new();
            let mut ins_lines: Vec<usize> = Vec::new();

            // Context lines inside the hunk are emitted immediately as
            // paired real rows; only +/- lines go into the staging vecs.
            let mut hunk_old = old_cursor;
            let mut hunk_new = new_cursor;

            // We need two passes: first collect context positions so we
            // can interleave them correctly with the change blocks.
            // Instead, build a per-line decision list.
            #[derive(Debug)]
            enum HunkLine {
                Context,
                Del,
                Ins,
            }
            let mut hunk_plan: Vec<HunkLine> = Vec::with_capacity(num_lines);
            for line_idx in 0..num_lines {
                if let Ok(line) = patch.line_in_hunk(hunk_idx, line_idx) {
                    match line.origin() {
                        '-' => hunk_plan.push(HunkLine::Del),
                        '+' => hunk_plan.push(HunkLine::Ins),
                        ' ' => hunk_plan.push(HunkLine::Context),
                        _ => {}
                    }
                }
            }

            // Walk the plan, flushing accumulated del/ins blocks whenever
            // we hit a context line or the end of the hunk.
            let flush_block = |del: &mut Vec<usize>,
                               ins: &mut Vec<usize>,
                               left: &mut Vec<VirtualLine>,
                               right: &mut Vec<VirtualLine>| {
                let max_len = del.len().max(ins.len());
                for i in 0..max_len {
                    match (del.get(i), ins.get(i)) {
                        (Some(&o), Some(&n)) => {
                            left.push(VirtualLine::Real(o));
                            right.push(VirtualLine::Real(n));
                        }
                        (Some(&o), None) => {
                            left.push(VirtualLine::Real(o));
                            right.push(VirtualLine::Padding);
                        }
                        (None, Some(&n)) => {
                            left.push(VirtualLine::Padding);
                            right.push(VirtualLine::Real(n));
                        }
                        (None, None) => {}
                    }
                }
                del.clear();
                ins.clear();
            };

            for action in &hunk_plan {
                match action {
                    HunkLine::Context => {
                        // Flush pending change block first
                        flush_block(&mut del_lines, &mut ins_lines, &mut left, &mut right);
                        // Emit the context line as a paired real row
                        left.push(VirtualLine::Real(hunk_old));
                        right.push(VirtualLine::Real(hunk_new));
                        hunk_old += 1;
                        hunk_new += 1;
                    }
                    HunkLine::Del => {
                        del_lines.push(hunk_old);
                        hunk_old += 1;
                    }
                    HunkLine::Ins => {
                        ins_lines.push(hunk_new);
                        hunk_new += 1;
                    }
                }
            }
            // Flush any trailing change block at the end of the hunk
            flush_block(&mut del_lines, &mut ins_lines, &mut left, &mut right);

            old_cursor = hunk_old;
            new_cursor = hunk_new;
        }

        // Flush remaining unchanged lines after the last hunk
        let old_remaining = old_line_count.saturating_sub(old_cursor);
        let new_remaining = new_line_count.saturating_sub(new_cursor);
        let trailing = old_remaining.max(new_remaining);

        for i in 0..trailing {
            let ol = old_cursor + i;
            let nl = new_cursor + i;
            left.push(if ol < old_line_count {
                VirtualLine::Real(ol)
            } else {
                VirtualLine::Padding
            });
            right.push(if nl < new_line_count {
                VirtualLine::Real(nl)
            } else {
                VirtualLine::Padding
            });
        }

        // Invariant: both vecs must always be the same length.
        // If they diverge due to a bug above, pad the shorter side so
        // the renderer never panics on an index mismatch.
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
}
