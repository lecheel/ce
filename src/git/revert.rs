// src/git/revert.rs
use super::gutter::find_git_root;
use crate::ed::buffer::Buffer;
use crate::ed::window::Window;
use git2::{DiffOptions, Patch, Repository};

// Change the signature:
pub fn revert_hunk_at_cursor(
    win: &mut Window,
    buf: &mut Buffer,
    gutter_width: usize,
) -> Option<()> {
    let filename = buf.filename.as_ref()?;
    let path_buf = std::path::PathBuf::from(filename);
    let repo_root = find_git_root(&path_buf)?;

    log::debug!(
        "Revert: attempting hunk revert for {:?} at row={}",
        filename,
        win.row
    );

    let repo = Repository::open(&repo_root).ok()?;
    let relative_path = path_buf.strip_prefix(&repo_root).unwrap_or(&path_buf);

    // Resolve reference content from the Git index
    let head = repo.head().ok()?;
    let commit = head.peel_to_commit().ok()?;
    let tree = commit.tree().ok()?;
    let entry = tree.get_path(relative_path).ok()?;
    let obj = entry.to_object(&repo).ok()?;
    let blob = obj.into_blob().ok()?;
    let head_content = blob.content();

    log::debug!(
        "Revert: loaded HEAD content for {:?} ({} bytes)",
        relative_path,
        head_content.len()
    );

    let mut opts = DiffOptions::new();
    opts.context_lines(0);

    let rope_content = buf.rope.to_string();
    let patch = Patch::from_buffers(
        head_content,
        Some(relative_path),
        rope_content.as_bytes(),
        Some(relative_path),
        Some(&mut opts),
    )
    .ok()?;

    let cursor_row = win.row;
    let num_hunks = patch.num_hunks();

    log::debug!("Revert: computed patch with {} hunks", num_hunks);

    for h in 0..num_hunks {
        let (_, lines_count) = patch.hunk(h).ok()?;

        let mut min_new_line = usize::MAX;
        let mut max_new_line = 0usize;
        let mut original_lines = Vec::new();

        for l in 0..lines_count {
            if let Ok(line) = patch.line_in_hunk(h, l) {
                match line.origin() {
                    '+' => {
                        if let Some(new_ln) = line.new_lineno() {
                            let idx = new_ln as usize - 1;
                            min_new_line = min_new_line.min(idx);
                            max_new_line = max_new_line.max(idx);
                        }
                    }
                    '-' => {
                        if let Ok(content_str) = std::str::from_utf8(line.content()) {
                            original_lines.push(content_str.to_string());
                        }
                    }
                    ' ' => {
                        if let Some(new_ln) = line.new_lineno() {
                            let idx = new_ln as usize - 1;
                            min_new_line = min_new_line.min(idx);
                            max_new_line = max_new_line.max(idx);
                        }
                    }
                    _ => {}
                }
            }
        }

        log::debug!(
            "Revert: hunk {} spans lines {}-{}, cursor_row={}, original_lines={}",
            h,
            min_new_line,
            max_new_line,
            cursor_row,
            original_lines.len()
        );

        // Apply changes if the cursor falls within this hunk
        if cursor_row >= min_new_line && cursor_row <= max_new_line {
            log::debug!("Revert: cursor is within hunk {}, applying revert", h);

            // Push undo state to support reverting the hunk discard
            buf.push_undo(win.row, win.col);

            let start_char = buf.rope.line_to_char(min_new_line);
            let end_char = buf
                .rope
                .line_to_char((max_new_line + 1).min(buf.rope.len_lines()));

            // Update in-memory Rope structure
            buf.rope.remove(start_char..end_char);

            let mut insert_text = String::new();
            for orig in &original_lines {
                insert_text.push_str(orig);
            }
            buf.rope.insert(start_char, &insert_text);

            buf.mark_modified();
            buf.parse_syntax(); // Triggers incremental syntax tree update

            win.row = min_new_line;
            win.col = 0;
            win.scroll_to_cursor(win.position.height, win.position.width, gutter_width);

            log::debug!(
                "Revert: successfully reverted hunk {} (restored {} original lines)",
                h,
                original_lines.len()
            );

            return Some(());
        }
    }

    log::debug!("Revert: no hunk contains cursor_row={}", cursor_row);

    None
}
