//! Text editing operations — grapheme-safe, char-offset implementation.
//!
//! Every function takes `(&mut Window, &mut Buffer)` so that cursor state
//! lives on the Window while text mutations live on the Buffer.
//! win.col is strictly a char index (number of chars from line start).

use crate::ed::buffer::Buffer;
use crate::ed::window::Window;
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

// ---------------------------------------------------------------------------
// Edit tracking for LSP incremental sync
// ---------------------------------------------------------------------------

/// Record of a single edit operation for LSP incremental sync.
#[derive(Debug, Clone, Default)]
pub struct Edit {
    pub start: usize,
    pub end: usize,
    pub inserted_text: String,
}

// ---------------------------------------------------------------------------
// Helper utilities
// ---------------------------------------------------------------------------
fn display_width(s: &str) -> usize {
    UnicodeWidthStr::width(s)
}

fn graphemes(text: &str) -> Vec<&str> {
    UnicodeSegmentation::graphemes(text, true).collect()
}

/// Calculate the visual column from a char index on a given line.
pub fn visual_col_from_char_idx(line: &str, char_idx: usize, tab_size: usize) -> usize {
    let mut vcol = 0;
    let mut n = 0;
    for g in UnicodeSegmentation::graphemes(line, true) {
        if n >= char_idx {
            break;
        }
        let w = if g == "\t" {
            tab_size - (vcol % tab_size)
        } else {
            display_width(g)
        };
        vcol += w;
        n += g.chars().count();
    }
    vcol
}

/// Find the nearest char index that corresponds to a visual column.
pub fn char_idx_from_visual_col(line: &str, visual_col: usize, tab_size: usize) -> usize {
    let mut vcol = 0;
    let mut char_idx = 0;
    for g in UnicodeSegmentation::graphemes(line, true) {
        if vcol >= visual_col {
            break;
        }
        let w = if g == "\t" {
            tab_size - (vcol % tab_size)
        } else {
            display_width(g)
        };
        if vcol + w > visual_col {
            break;
        }
        vcol += w;
        char_idx += g.chars().count();
    }
    char_idx
}

/// **Tab-aware** visual width of an entire line.
pub fn line_visual_width(line: &str, tab_size: usize) -> usize {
    visual_col_from_char_idx(line, line.chars().count(), tab_size)
}

// ---------------------------------------------------------------------------
// Insertion
// ---------------------------------------------------------------------------
fn cursor_in_bounds(win: &Window, buf: &Buffer) -> bool {
    win.row < buf.len_lines()
}

pub fn insert_char(win: &mut Window, buf: &mut Buffer, ch: char) {
    if !cursor_in_bounds(win, buf) {
        return;
    }
    let line_start = buf.rope.line_to_char(win.row);
    let insert_pos = line_start + win.col;

    // 1. Capture byte coordinates BEFORE insertion
    let start_byte = buf.rope.char_to_byte(insert_pos);
    let start_row = win.row;
    let start_col_byte = start_byte.saturating_sub(buf.rope.line_to_byte(start_row));
    let start_point = tree_sitter::Point::new(start_row, start_col_byte);

    let ch_str = ch.to_string();
    let added_bytes = ch_str.len();
    let char_count = 1;

    // 2. Perform the actual mutation
    buf.rope.insert(insert_pos, &ch_str);
    win.col += char_count;
    win.desired_col = visual_col_from_char_idx(&buf.line_text(win.row), win.col, buf.tab_size);

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
    if !cursor_in_bounds(win, buf) {
        return;
    }
    let row = win.row;
    let line_text = buf.line_text(row);
    let chars: Vec<char> = line_text.chars().collect();
    let col = win.col.min(chars.len());
    let before: String = chars[..col].iter().collect();
    let after: String = chars[col..].iter().collect();
    let before_trimmed = before.trim_end();
    let base_indent: String = line_text
        .chars()
        .take_while(|c| c.is_whitespace())
        .collect();

    let mut new_indent = base_indent;
    if before_trimmed.ends_with('{') {
        new_indent.push_str("    ");
    }

    let after_trimmed = after.trim_start();
    let after_clean = after_trimmed.trim_end_matches(|c| c == '\r' || c == '\n');

    let line1 = before_trimmed.to_string();
    let line2 = format!("{}{}", new_indent, after_clean);

    let start_char = buf.rope.line_to_char(row);
    let end_char = if row + 1 >= buf.len_lines() {
        buf.rope.len_chars()
    } else {
        buf.rope.line_to_char(row + 1)
    };

    buf.rope.remove(start_char..end_char);
    buf.rope
        .insert(start_char, &format!("{}\n{}\n", line1, line2));

    win.row = row + 1;
    win.col = new_indent.chars().count();
    win.desired_col = visual_col_from_char_idx(&buf.line_text(win.row), win.col, buf.tab_size);
    buf.mark_modified();
}

pub fn insert_newline_below(win: &mut Window, buf: &mut Buffer) {
    if !cursor_in_bounds(win, buf) {
        return;
    }
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
        let last = buf.rope.len_chars();
        if last > 0 && buf.rope.char(last - 1) != '\n' {
            buf.rope.insert(last, "\n");
        }
        buf.rope.len_chars()
    } else {
        buf.rope.line_to_char(row + 1)
    };

    buf.rope.insert(insert_pos, &new_line);
    win.row = row + 1;
    win.col = new_indent.chars().count();
    win.desired_col = visual_col_from_char_idx(&buf.line_text(win.row), win.col, buf.tab_size);
    buf.mark_modified();
}

pub fn insert_newline_above(win: &mut Window, buf: &mut Buffer) {
    if !cursor_in_bounds(win, buf) {
        return;
    }
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
    win.desired_col = visual_col_from_char_idx(&buf.line_text(win.row), win.col, buf.tab_size);
    buf.mark_modified();
}

/// Insert a tab (4 spaces).
pub fn insert_tab(win: &mut Window, buf: &mut Buffer) {
    if !cursor_in_bounds(win, buf) {
        return;
    }
    for _ in 0..4 {
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
    let line_len = buf.line_char_len(win.row);
    let col = win.col.min(line_len);
    if col > 0 {
        let line_start = buf.rope.line_to_char(win.row);
        let text = buf.line_text(win.row);
        let mut char_idx = 0;
        let mut prev_char_idx = 0;
        let mut prev_grapheme_len = 0;
        for grapheme in graphemes(&text) {
            if char_idx == col {
                break;
            }
            prev_char_idx = char_idx;
            prev_grapheme_len = grapheme.chars().count();
            char_idx += grapheme.chars().count();
        }
        let start_offset = line_start + prev_char_idx;
        let end_offset = line_start + col;
        let start_byte = buf.rope.char_to_byte(start_offset);
        let old_end_byte = buf.rope.char_to_byte(end_offset);
        let start_row = win.row;
        let start_col_byte = start_byte.saturating_sub(buf.rope.line_to_byte(start_row));
        let old_end_col_byte = old_end_byte.saturating_sub(buf.rope.line_to_byte(start_row));
        let start_point = tree_sitter::Point::new(start_row, start_col_byte);
        let old_end_point = tree_sitter::Point::new(start_row, old_end_col_byte);
        buf.rope.remove(start_offset..end_offset);
        win.col = prev_char_idx;
        win.desired_col = visual_col_from_char_idx(&buf.line_text(win.row), win.col, buf.tab_size);
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
    } else if win.row > 0 {
        if win.row - 1 >= buf.len_lines() {
            return;
        }
        let newline_pos = buf.rope.line_to_char(win.row) - 1;
        let prev_line_len = buf.line_char_len(win.row - 1);

        let start_byte = buf.rope.char_to_byte(newline_pos);
        let old_end_byte = start_byte + 1;
        let start_row = win.row - 1;
        let start_col_byte = start_byte.saturating_sub(buf.rope.line_to_byte(start_row));

        let start_point = tree_sitter::Point::new(start_row, start_col_byte);
        let old_end_point = tree_sitter::Point::new(win.row, 0);

        buf.rope.remove(newline_pos..newline_pos + 1);
        win.row -= 1;
        win.col = prev_line_len;
        win.desired_col = visual_col_from_char_idx(&buf.line_text(win.row), win.col, buf.tab_size);
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
    let line_len = buf.line_char_len(win.row);
    let col = win.col.min(line_len);
    if col < line_len {
        let line_start = buf.rope.line_to_char(win.row);
        let text = buf.line_text(win.row);
        let mut char_idx = 0;
        let mut grapheme_len = 1;
        for grapheme in graphemes(&text) {
            if char_idx == col {
                grapheme_len = grapheme.chars().count();
                break;
            }
            char_idx += grapheme.chars().count();
        }
        let start_offset = line_start + col;
        let end_offset = line_start + col + grapheme_len;
        let start_byte = buf.rope.char_to_byte(start_offset);
        let old_end_byte = buf.rope.char_to_byte(end_offset);
        let start_row = win.row;
        let start_col_byte = start_byte.saturating_sub(buf.rope.line_to_byte(start_row));
        let old_end_col_byte = old_end_byte.saturating_sub(buf.rope.line_to_byte(start_row));

        let start_point = tree_sitter::Point::new(start_row, start_col_byte);
        let old_end_point = tree_sitter::Point::new(start_row, old_end_col_byte);

        buf.rope.remove(start_offset..end_offset);
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
    win.desired_col = visual_col_from_char_idx(&buf.line_text(win.row), win.col, buf.tab_size);
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
        win.desired_col = 0;
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
    win.desired_col = visual_col_from_char_idx(&buf.line_text(win.row), win.col, buf.tab_size);
    buf.mark_modified();
    buf.parse_syntax();
}

/// Delete from the cursor to the start of the next word.
pub fn delete_word_forward(win: &mut Window, buf: &mut Buffer) {
    if !cursor_in_bounds(win, buf) {
        return;
    }
    let absolute_start = buf.rope.line_to_char(win.row) + win.col;

    crate::ed::movement::move_word_forward(win, buf);

    let absolute_end = buf.rope.line_to_char(win.row) + win.col;

    if absolute_start < absolute_end {
        buf.rope.remove(absolute_start..absolute_end);
        win.row = buf.rope.char_to_line(absolute_start);
        win.col = absolute_start - buf.rope.line_to_char(win.row);
        win.desired_col = visual_col_from_char_idx(&buf.line_text(win.row), win.col, buf.tab_size);
        buf.mark_modified();
    }
}

/// Delete from the cursor backward to the start of the previous word.
pub fn delete_word_backward(win: &mut Window, buf: &mut Buffer) {
    if !cursor_in_bounds(win, buf) {
        return;
    }
    let absolute_end = buf.rope.line_to_char(win.row) + win.col;

    crate::ed::movement::move_word_backward(win, buf);

    let absolute_start = buf.rope.line_to_char(win.row) + win.col;

    if absolute_start < absolute_end {
        buf.rope.remove(absolute_start..absolute_end);
        win.row = buf.rope.char_to_line(absolute_start);
        win.col = absolute_start - buf.rope.line_to_char(win.row);
        win.desired_col = visual_col_from_char_idx(&buf.line_text(win.row), win.col, buf.tab_size);
        buf.mark_modified();
    }
}

/// Delete from the cursor to the end of the current line (vim `D`/`d$`).
pub fn delete_to_end_of_line(win: &mut Window, buf: &mut Buffer) {
    if !cursor_in_bounds(win, buf) {
        return;
    }
    let line_start = buf.rope.line_to_char(win.row);
    let line_char_len = buf.line_char_len(win.row);
    let del_start = line_start + win.col;
    if win.col >= line_char_len {
        return;
    }
    buf.rope.remove(del_start..line_start + line_char_len);
    win.desired_col = visual_col_from_char_idx(&buf.line_text(win.row), win.col, buf.tab_size);
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
    win.col += 4;
    win.desired_col = visual_col_from_char_idx(&buf.line_text(win.row), win.col, buf.tab_size);
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
        win.desired_col = visual_col_from_char_idx(&buf.line_text(win.row), win.col, buf.tab_size);
        buf.mark_modified();
    }
}

/// Paste plain text inline at the cursor position.
pub fn paste_text(win: &mut Window, buf: &mut Buffer, text: &str) {
    if !cursor_in_bounds(win, buf) {
        return;
    }
    let line_len = buf.line_char_len(win.row);
    let col = win.col.min(line_len);
    let line_start = buf.rope.line_to_char(win.row);
    let insert_offset = line_start + col;
    let char_count = text.chars().count();
    buf.rope.insert(insert_offset, text);
    win.col = col + char_count;
    win.desired_col = visual_col_from_char_idx(&buf.line_text(win.row), win.col, buf.tab_size);
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
        if last > 0 && buf.rope.char(last - 1) != '\n' {
            buf.rope.insert(last, "\n");
        }
        buf.rope.len_chars()
    } else {
        buf.rope.line_to_char(next_line_row)
    };
    buf.rope.insert(insert_offset, text);
    win.row = next_line_row;
    win.col = 0;
    win.desired_col = 0;
    buf.mark_modified();
    buf.parse_syntax();
}

// ---------------------------------------------------------------------------
// Grapheme-safe utilities
// ---------------------------------------------------------------------------

/// **Tab-aware** visual width of a line in the buffer.
pub fn line_display_width(buf: &Buffer, row: usize) -> usize {
    if row >= buf.len_lines() {
        return 0;
    }
    line_visual_width(&buf.line_text(row), buf.tab_size)
}

pub fn move_to_display_column(win: &mut Window, buf: &mut Buffer, target_visual_col: usize) {
    let line_text = buf.line_text(win.row);
    win.col = char_idx_from_visual_col(&line_text, target_visual_col, buf.tab_size);
    win.desired_col = target_visual_col;
}

/// Returns the visual width of the grapheme at the given char index.
pub fn grapheme_width_at_char(buf: &Buffer, row: usize, char_idx: usize) -> usize {
    if row >= buf.len_lines() {
        return 1;
    }
    let line_text = buf.line_text(row);
    let mut n = 0;
    for g in UnicodeSegmentation::graphemes(line_text.as_str(), true) {
        if n == char_idx {
            let vcol = visual_col_from_char_idx(&line_text, n, buf.tab_size);
            return if g == "\t" {
                buf.tab_size - (vcol % buf.tab_size)
            } else {
                display_width(g).max(1)
            };
        }
        n += g.chars().count();
    }
    1
}
