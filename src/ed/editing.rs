//! Text editing operations — grapheme-safe implementation.
//!
//! Every function takes `(&mut Window, &mut Buffer)` so that cursor state
//! lives on the Window while text mutations live on the Buffer.

use crate::ed::buffer::Buffer;
use crate::ed::window::Window;
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

// ---------------------------------------------------------------------------
// Helper utilities
// ---------------------------------------------------------------------------

fn display_width(s: &str) -> usize {
    UnicodeWidthStr::width(s)
}

fn graphemes(text: &str) -> Vec<&str> {
    UnicodeSegmentation::graphemes(text, true).collect()
}

/// Returns (char_offset, visual_col).
/// `char_offset` is in Unicode scalar values (compatible with Ropey).
fn find_grapheme_boundary(line: &str, target_col: usize) -> (usize, usize) {
    let mut char_offset = 0;
    let mut current_col = 0;
    for grapheme in graphemes(line) {
        let width = display_width(grapheme);
        if current_col >= target_col {
            break;
        }
        // If target_col is inside this wide grapheme, snap to its END
        if current_col + width > target_col {
            if width > 1 && target_col > current_col {
                char_offset += grapheme.chars().count();
                current_col += width;
            }
            break;
        }
        char_offset += grapheme.chars().count();
        current_col += width;
    }
    (char_offset, current_col)
}

/// Returns (start_char, end_char, visual_width) of the grapheme before `col`.
fn prev_grapheme_at_col(line: &str, col: usize) -> Option<(usize, usize, usize)> {
    let mut char_offset = 0;
    let mut current_col = 0;
    let mut prev_start = 0;
    let mut prev_end = 0;
    let mut prev_width = 0;
    for grapheme in graphemes(line) {
        let width = display_width(grapheme);
        if current_col >= col {
            return Some((prev_start, prev_end, prev_width));
        }
        // If col is inside this wide grapheme, backspace should delete THIS grapheme
        if current_col + width > col {
            return Some((char_offset, char_offset + grapheme.chars().count(), width));
        }
        prev_start = prev_end;
        prev_end = char_offset + grapheme.chars().count();
        prev_width = width;
        char_offset += grapheme.chars().count();
        current_col += width;
    }
    if prev_end > 0 {
        Some((prev_start, prev_end, prev_width))
    } else {
        None
    }
}

/// Returns (start_char, end_char, visual_width) of the grapheme at `col`.
fn grapheme_at_col(line: &str, col: usize) -> Option<(usize, usize, usize)> {
    let mut char_offset = 0;
    let mut current_col = 0;
    for grapheme in graphemes(line) {
        let width = display_width(grapheme);
        if current_col == col {
            return Some((char_offset, char_offset + grapheme.chars().count(), width));
        }
        // If col is inside this wide grapheme, the character at col is THIS grapheme.
        if current_col + width > col {
            return Some((char_offset, char_offset + grapheme.chars().count(), width));
        }
        char_offset += grapheme.chars().count();
        current_col += width;
    }
    None
}

/// Returns the visual width of the grapheme at the given visual column.
pub fn grapheme_width_at_col(buf: &Buffer, row: usize, col: usize) -> usize {
    if row >= buf.len_lines() {
        return 1;
    }
    let line_text = buf.line_text(row);
    if let Some((_, _, width)) = grapheme_at_col(&line_text, col) {
        width
    } else {
        1
    }
}

pub fn col_from_char_offset(line: &str, target_offset: usize) -> usize {
    let mut char_offset = 0;
    let mut col = 0;
    for grapheme in graphemes(line) {
        if char_offset >= target_offset {
            break;
        }
        if char_offset + grapheme.chars().count() > target_offset {
            break;
        }
        col += display_width(grapheme);
        char_offset += grapheme.chars().count();
    }
    col
}

// ---------------------------------------------------------------------------
// Insertion
// ---------------------------------------------------------------------------
fn cursor_in_bounds(win: &Window, buf: &Buffer) -> bool {
    win.row < buf.len_lines()
}

/// Insert a single character at the cursor position and advance the cursor.
pub fn insert_char(win: &mut Window, buf: &mut Buffer, ch: char) {
    if !cursor_in_bounds(win, buf) {
        return;
    }
    let line_start = buf.rope.line_to_char(win.row);
    let current_line = buf.line_text(win.row);
    let (insert_char_offset, insert_col) = find_grapheme_boundary(&current_line, win.col);
    let insert_pos = line_start + insert_char_offset;

    // 1. Capture byte coordinates BEFORE insertion
    let start_byte = buf.rope.char_to_byte(insert_pos);
    let start_row = win.row;
    let start_col_byte = start_byte.saturating_sub(buf.rope.line_to_byte(start_row));
    let start_point = tree_sitter::Point::new(start_row, start_col_byte);

    let ch_str = ch.to_string();
    let char_width = display_width(&ch_str);
    let added_bytes = ch_str.len();

    // 2. Perform the actual mutation
    buf.rope.insert(insert_pos, &ch_str);
    win.col = insert_col + char_width;
    win.desired_col = win.col;
    buf.mark_modified();

    // 3. Perform incremental update
    let edit = tree_sitter::InputEdit {
        start_byte,
        old_end_byte: start_byte,
        new_end_byte: start_byte + added_bytes,
        start_position: start_point,
        old_end_position: start_point,
        new_end_position: tree_sitter::Point::new(start_row, start_col_byte + added_bytes),
    };
    buf.parse_syntax_incremental(edit);
}

/// Insert a newline at the cursor position, auto-copying indentation.
pub fn insert_newline(win: &mut Window, buf: &mut Buffer) {
    let row = win.row;

    let line_text = buf.line_text(row);

    // Resolve the visual column to a character offset
    let (char_offset, _) = find_grapheme_boundary(&line_text, win.col);

    let chars: Vec<char> = line_text.chars().collect();

    // Split the line at the cursor using character indices (safe for multi-byte/emoji)
    let before: String = chars[..char_offset].iter().collect();
    let after: String = chars[char_offset..].iter().collect();

    // Trim trailing whitespace from the line we are leaving (standard Vim behavior)
    let before_trimmed = before.trim_end();

    // Calculate base indentation from the ORIGINAL full line.
    // This ensures that if the cursor is at col 0 on an indented line,
    // we preserve the block indentation.
    let base_indent: String = line_text
        .chars()
        .take_while(|c| c.is_whitespace())
        .collect();

    // Determine the new indentation for the second line
    let mut new_indent = base_indent;

    // Heuristic: if the first half ends with '{', increase indentation level
    if before_trimmed.ends_with('{') {
        new_indent.push_str("    "); // Assuming 4-space indent
    }

    // Strip original leading whitespace to prevent double indentation
    let after_trimmed = after.trim_start();
    // Strip trailing line breaks so we can control formatting cleanly
    let after_clean = after_trimmed.trim_end_matches(|c| c == '\r' || c == '\n');

    // Reconstruct the two lines
    let line1 = before_trimmed.to_string();
    let line2 = format!("{}{}", new_indent, after_clean);

    // Replace the current line in the Rope
    let start_char = buf.rope.line_to_char(row);
    let end_char = if row + 1 >= buf.len_lines() {
        buf.rope.len_chars()
    } else {
        buf.rope.line_to_char(row + 1)
    };

    buf.rope.remove(start_char..end_char);
    // Explicitly add a newline after both lines to keep the next line split
    buf.rope
        .insert(start_char, &format!("{}\n{}\n", line1, line2));

    // Update cursor to the beginning of the text on the new line
    win.row = row + 1;
    win.col = new_indent.chars().count();

    buf.mark_modified();
}

pub fn insert_newline_below(win: &mut Window, buf: &mut Buffer) {
    let row = win.row;
    let raw_line = buf.line_text(row);
    let line_text = raw_line.trim_end_matches('\n');

    let indent: String = line_text
        .chars()
        .take_while(|c| c.is_whitespace())
        .collect();

    let mut new_indent = indent;
    let trimmed = line_text.trim_end();
    if trimmed.ends_with('{') {
        new_indent.push_str("    ");
    }

    let new_line = format!("{}\n", new_indent);

    let insert_pos = if row + 1 >= buf.len_lines() {
        buf.rope.len_chars()
    } else {
        buf.rope.line_to_char(row + 1)
    };

    buf.rope.insert(insert_pos, &new_line);

    win.row = row + 1;
    win.col = new_indent.chars().count();

    buf.mark_modified();
}

pub fn insert_newline_above(win: &mut Window, buf: &mut Buffer) {
    let row = win.row;
    let raw_line = buf.line_text(row);
    let line_text = raw_line.trim_end_matches('\n');

    let indent: String = line_text
        .chars()
        .take_while(|c| c.is_whitespace())
        .collect();
    let new_line = format!("{}\n", indent);

    let insert_pos = buf.rope.line_to_char(row);
    buf.rope.insert(insert_pos, &new_line);

    win.row = row;
    win.col = indent.chars().count();

    buf.mark_modified();
}

/// Insert a tab (4 spaces).
pub fn insert_tab(win: &mut Window, buf: &mut Buffer) {
    if !cursor_in_bounds(win, buf) {
        return;
    }
    let tab_width = 4;
    let spaces_to_next_tab = tab_width - (win.col % tab_width);
    for _ in 0..spaces_to_next_tab {
        insert_char(win, buf, ' ');
    }
}

// ---------------------------------------------------------------------------
// Deletion & Indentation
// ---------------------------------------------------------------------------

pub fn backspace(win: &mut Window, buf: &mut Buffer) {
    if !cursor_in_bounds(win, buf) {
        return;
    }
    if win.col > 0 {
        let line_start = buf.rope.line_to_char(win.row);
        let current_line = buf.line_text(win.row);

        if let Some((start, end, width)) = prev_grapheme_at_col(&current_line, win.col) {
            // 1. Capture byte coordinates BEFORE removal
            let start_byte = buf.rope.char_to_byte(line_start + start);
            let old_end_byte = buf.rope.char_to_byte(line_start + end);
            let start_row = win.row;
            let start_col_byte = start_byte.saturating_sub(buf.rope.line_to_byte(start_row));
            let old_end_col_byte = old_end_byte.saturating_sub(buf.rope.line_to_byte(start_row));

            let start_point = tree_sitter::Point::new(start_row, start_col_byte);
            let old_end_point = tree_sitter::Point::new(start_row, old_end_col_byte);

            // 2. Perform mutation
            buf.rope.remove(line_start + start..line_start + end);
            win.col -= width;
            win.desired_col = win.col;
            buf.mark_modified();

            // 3. Perform incremental update
            let edit = tree_sitter::InputEdit {
                start_byte,
                old_end_byte,
                new_end_byte: start_byte,
                start_position: start_point,
                old_end_position: old_end_point,
                new_end_position: start_point,
            };
            buf.parse_syntax_incremental(edit);
        }
    } else if win.row > 0 {
        if win.row - 1 >= buf.len_lines() {
            return;
        }
        let newline_pos = buf.rope.line_to_char(win.row) - 1;
        let prev_line = buf.line_text(win.row - 1);
        let prev_line_width = display_width(&prev_line);

        let start_byte = buf.rope.char_to_byte(newline_pos);
        let old_end_byte = start_byte + 1;
        let start_row = win.row - 1;
        let start_col_byte = start_byte.saturating_sub(buf.rope.line_to_byte(start_row));

        let start_point = tree_sitter::Point::new(start_row, start_col_byte);
        let old_end_point = tree_sitter::Point::new(win.row, 0);

        buf.rope.remove(newline_pos..newline_pos + 1);
        win.row -= 1;
        win.col = prev_line_width;
        win.desired_col = win.col;
        buf.mark_modified();

        let edit = tree_sitter::InputEdit {
            start_byte,
            old_end_byte,
            new_end_byte: start_byte,
            start_position: start_point,
            old_end_position: old_end_point,
            new_end_position: start_point,
        };
        buf.parse_syntax_incremental(edit);
    }
}

/// Delete the grapheme at the cursor position. Joins lines at line end.
pub fn delete_char_forward(win: &mut Window, buf: &mut Buffer) {
    if !cursor_in_bounds(win, buf) {
        return;
    }
    let line_start = buf.rope.line_to_char(win.row);
    let current_line = buf.line_text(win.row);
    let line_width = display_width(&current_line);

    if win.col < line_width {
        if let Some((start, end, _)) = grapheme_at_col(&current_line, win.col) {
            let start_byte = buf.rope.char_to_byte(line_start + start);
            let old_end_byte = buf.rope.char_to_byte(line_start + end);
            let start_row = win.row;
            let start_col_byte = start_byte.saturating_sub(buf.rope.line_to_byte(start_row));
            let old_end_col_byte = old_end_byte.saturating_sub(buf.rope.line_to_byte(start_row));

            let start_point = tree_sitter::Point::new(start_row, start_col_byte);
            let old_end_point = tree_sitter::Point::new(start_row, old_end_col_byte);

            buf.rope.remove(line_start + start..line_start + end);
            buf.mark_modified();

            let edit = tree_sitter::InputEdit {
                start_byte,
                old_end_byte,
                new_end_byte: start_byte,
                start_position: start_point,
                old_end_position: old_end_point,
                new_end_position: start_point,
            };
            buf.parse_syntax_incremental(edit);
        }
    } else if win.row + 1 < buf.len_lines() {
        let newline_pos = buf.rope.line_to_char(win.row + 1) - 1;

        let start_byte = buf.rope.char_to_byte(newline_pos);
        let old_end_byte = start_byte + 1;
        let start_row = win.row;
        let start_col_byte = start_byte.saturating_sub(buf.rope.line_to_byte(start_row));

        let start_point = tree_sitter::Point::new(start_row, start_col_byte);
        let old_end_point = tree_sitter::Point::new(win.row + 1, 0);

        buf.rope.remove(newline_pos..newline_pos + 1);
        buf.mark_modified();

        let edit = tree_sitter::InputEdit {
            start_byte,
            old_end_byte,
            new_end_byte: start_byte,
            start_position: start_point,
            old_end_position: old_end_point,
            new_end_position: start_point,
        };
        buf.parse_syntax_incremental(edit);
    }
    win.desired_col = win.col;
}

/// Delete the entire current line.
pub fn delete_current_line(win: &mut Window, buf: &mut Buffer) {
    if !cursor_in_bounds(win, buf) {
        return;
    }
    if buf.len_lines() <= 1 {
        buf.rope.remove(..buf.rope.len_chars());
        buf.rope.insert(0, "\n");
        win.col = 0;
        win.desired_col = win.col;
        buf.mark_modified();
        return;
    }

    let line_start = buf.rope.line_to_char(win.row);
    let next_line_start = buf.rope.line_to_char(win.row + 1);
    buf.rope.remove(line_start..next_line_start);

    if win.row >= buf.len_lines() {
        win.row = buf.len_lines() - 1;
    }
    win.col = win.col.min(buf.line_char_len(win.row));
    buf.mark_modified();
    buf.parse_syntax();
}

/// Delete from the cursor to the start of the next word.
pub fn delete_word_forward(win: &mut Window, buf: &mut Buffer) {
    if !cursor_in_bounds(win, buf) {
        return;
    }
    let start_line = buf.line_text(win.row);
    let (start_char_offset, _) = find_grapheme_boundary(&start_line, win.col);
    let absolute_start = buf.rope.line_to_char(win.row) + start_char_offset;

    crate::ed::movement::move_word_forward(win, buf);

    let end_line = buf.line_text(win.row);
    let (end_char_offset, _) = find_grapheme_boundary(&end_line, win.col);
    let absolute_end = buf.rope.line_to_char(win.row) + end_char_offset;

    if absolute_start < absolute_end {
        buf.rope.remove(absolute_start..absolute_end);
        let new_line = buf.line_text(win.row);
        win.row = buf.rope.char_to_line(absolute_start);
        let line_start = buf.rope.line_to_char(win.row);
        win.col = col_from_char_offset(&new_line, absolute_start - line_start);
        win.desired_col = win.col;
        buf.mark_modified();
    }
}

/// Delete from the cursor backward to the start of the previous word.
pub fn delete_word_backward(win: &mut Window, buf: &mut Buffer) {
    if !cursor_in_bounds(win, buf) {
        return;
    }
    let end_line = buf.line_text(win.row);
    let (end_char_offset, _) = find_grapheme_boundary(&end_line, win.col);
    let absolute_end = buf.rope.line_to_char(win.row) + end_char_offset;

    crate::ed::movement::move_word_backward(win, buf);

    let start_line = buf.line_text(win.row);
    let (start_char_offset, _) = find_grapheme_boundary(&start_line, win.col);
    let absolute_start = buf.rope.line_to_char(win.row) + start_char_offset;

    if absolute_start < absolute_end {
        buf.rope.remove(absolute_start..absolute_end);
        let final_line = buf.line_text(win.row);
        win.row = buf.rope.char_to_line(absolute_start);
        let line_start = buf.rope.line_to_char(win.row);
        win.col = col_from_char_offset(&final_line, absolute_start - line_start);
        win.desired_col = win.col;
        buf.mark_modified();
    }
}

/// Delete from the cursor to the end of the current line (vim `D`/`d$`).
pub fn delete_to_end_of_line(win: &mut Window, buf: &mut Buffer) {
    if !cursor_in_bounds(win, buf) {
        return;
    }
    let line_start = buf.rope.line_to_char(win.row);
    let line_text = buf.line_text(win.row);
    let line_char_len = buf.line_char_len(win.row);
    let (start_char_offset, _) = find_grapheme_boundary(&line_text, win.col);
    let del_start = line_start + start_char_offset;
    if start_char_offset >= line_char_len {
        return;
    }
    buf.rope.remove(del_start..line_start + line_char_len);
    win.desired_col = win.col;
    buf.mark_modified();
}

/// Indent the current line by one level (4 spaces).
pub fn indent_line(win: &mut Window, buf: &mut Buffer) {
    if !cursor_in_bounds(win, buf) {
        return;
    }
    let line_start = buf.rope.line_to_char(win.row);
    let indent = "    ";
    buf.rope.insert(line_start, indent);
    win.col += display_width(indent);
    win.desired_col = win.col;
    buf.mark_modified();
}

/// Outdent the current line by up to one level.
pub fn outdent_line(win: &mut Window, buf: &mut Buffer) {
    if !cursor_in_bounds(win, buf) {
        return;
    }
    let line_text = buf.line_text(win.row);
    let leading_spaces = line_text.chars().take_while(|c| *c == ' ').count();
    if leading_spaces > 0 {
        let to_remove = leading_spaces.min(4);
        let line_start = buf.rope.line_to_char(win.row);
        buf.rope.remove(line_start..line_start + to_remove);
        win.col = win.col.saturating_sub(to_remove);
        win.desired_col = win.col;
        buf.mark_modified();
    }
}

/// Paste plain text inline at the cursor position.
pub fn paste_text(win: &mut Window, buf: &mut Buffer, text: &str) {
    if !cursor_in_bounds(win, buf) {
        return;
    }
    let line_start = buf.rope.line_to_char(win.row);
    let current_line = buf.line_text(win.row);
    let (insert_char_offset, insert_col) = find_grapheme_boundary(&current_line, win.col);
    let insert_offset = line_start + insert_char_offset;

    buf.rope.insert(insert_offset, text);
    win.col = insert_col + display_width(text);
    win.desired_col = win.col;
    buf.mark_modified();
    buf.parse_syntax();
}

/// Paste a line-yanked sequence below the current line.
pub fn paste_line_below(win: &mut Window, buf: &mut Buffer, text: &str) {
    if !cursor_in_bounds(win, buf) {
        return;
    }
    let next_line_row = win.row + 1;
    let insert_offset = if next_line_row >= buf.len_lines() {
        let last = buf.rope.len_chars();
        if last > 0 {
            let last_char = buf.rope.char(last - 1);
            if last_char != '\n' {
                buf.rope.insert(last, "\n");
            }
        }
        buf.rope.len_chars()
    } else {
        buf.rope.line_to_char(next_line_row)
    };
    buf.rope.insert(insert_offset, text);
    win.row = next_line_row;
    win.col = 0;
    win.desired_col = win.col;
    buf.mark_modified();
    buf.parse_syntax();
}

// ---------------------------------------------------------------------------
// Grapheme-safe utilities
// ---------------------------------------------------------------------------

pub fn line_display_width(buf: &Buffer, row: usize) -> usize {
    if row >= buf.len_lines() {
        return 0;
    }
    display_width(&buf.line_text(row))
}

pub fn move_to_display_column(win: &mut Window, buf: &Buffer, target_col: usize) {
    let line_text = buf.line_text(win.row);
    let (_, actual_col) = find_grapheme_boundary(&line_text, target_col);
    win.col = actual_col;
}
