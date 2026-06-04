//! Cursor movement operations — grapheme-safe, char-offset implementation.
//!
//! win.col is strictly a char index. desired_col is a visual column
//! used to remember the horizontal position during vertical movements.

use crate::ed::buffer::Buffer;
use crate::ed::editing::{char_idx_from_visual_col, visual_col_from_char_idx};
use crate::ed::mode::Mode;
use crate::ed::window::Window;
use unicode_segmentation::UnicodeSegmentation;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn valid_row(win: &Window, buf: &Buffer) -> bool {
    win.row < buf.len_lines()
}

fn graphemes(text: &str) -> Vec<&str> {
    UnicodeSegmentation::graphemes(text, true).collect()
}

// ---------------------------------------------------------------------------
// Horizontal movement
// ---------------------------------------------------------------------------
pub fn move_left(win: &mut Window, buf: &Buffer) {
    if !valid_row(win, buf) {
        return;
    }

    if win.col > 0 {
        let text = buf.line_text(win.row);
        let mut char_idx = 0;
        let mut prev_char_idx = 0;
        for grapheme in graphemes(&text) {
            if char_idx == win.col {
                break;
            }
            prev_char_idx = char_idx;
            char_idx += grapheme.chars().count();
        }
        if char_idx < win.col {
            prev_char_idx = char_idx;
        }
        win.col = prev_char_idx;
    } else if win.row > 0 {
        win.row -= 1;
        win.col = buf.line_char_len(win.row);
    }
    win.desired_col = visual_col_from_char_idx(&buf.line_text(win.row), win.col, buf.tab_size);
}

pub fn move_right(win: &mut Window, buf: &Buffer) {
    if !valid_row(win, buf) {
        return;
    }
    let line_len = buf.line_char_len(win.row);
    if win.col < line_len {
        let text = buf.line_text(win.row);
        let mut char_idx = 0;
        for grapheme in graphemes(&text) {
            if char_idx == win.col {
                win.col += grapheme.chars().count();
                break;
            }
            char_idx += grapheme.chars().count();
        }
    } else if win.row + 1 < buf.len_lines() {
        win.row += 1;
        win.col = 0;
    }
    win.desired_col = visual_col_from_char_idx(&buf.line_text(win.row), win.col, buf.tab_size);
}

pub fn move_line_end(win: &mut Window, buf: &Buffer, mode: Mode) {
    if !valid_row(win, buf) {
        return;
    }
    let len = buf.line_char_len(win.row);
    win.col = if mode == Mode::Normal {
        len.saturating_sub(1)
    } else {
        len
    };
    win.desired_col = visual_col_from_char_idx(&buf.line_text(win.row), win.col, buf.tab_size);
}

pub fn move_up(win: &mut Window, buf: &Buffer) {
    if !valid_row(win, buf) {
        return;
    }
    if win.row > 0 {
        let vcol = visual_col_from_char_idx(&buf.line_text(win.row), win.col, buf.tab_size);
        win.desired_col = win.desired_col.max(vcol);

        let old_row = win.row;
        let old_col = win.col;
        win.row -= 1;
        let new_line = buf.line_text(win.row);
        win.col = char_idx_from_visual_col(&new_line, win.desired_col, buf.tab_size);
    }
}

pub fn move_down(win: &mut Window, buf: &Buffer) {
    if !valid_row(win, buf) {
        return;
    }
    if win.row + 1 < buf.len_lines() {
        let vcol = visual_col_from_char_idx(&buf.line_text(win.row), win.col, buf.tab_size);
        win.desired_col = win.desired_col.max(vcol);

        let old_row = win.row;
        let old_col = win.col;
        win.row += 1;
        let new_line = buf.line_text(win.row);
        win.col = char_idx_from_visual_col(&new_line, win.desired_col, buf.tab_size);
    }
}

pub fn move_line_start(win: &mut Window, _buf: &Buffer) {
    win.col = 0;
    win.desired_col = 0;
}

pub fn move_word_forward(win: &mut Window, buf: &Buffer) {
    if !valid_row(win, buf) {
        return;
    }
    let text = buf.line_text(win.row);
    let gr = graphemes(&text);
    let mut char_idx = 0;
    let mut in_word = false;
    let mut past_word = false;

    for g in &gr {
        let g_len = g.chars().count();
        if char_idx > win.col {
            if !g.trim().is_empty() {
                if past_word {
                    win.col = char_idx;
                    win.desired_col = visual_col_from_char_idx(&text, win.col, buf.tab_size);
                    return;
                }
                in_word = true;
            } else if in_word {
                past_word = true;
            }
        }
        char_idx += g_len;
    }

    if win.row + 1 < buf.len_lines() {
        win.row += 1;
        win.col = 0;
    } else {
        win.col = buf.line_char_len(win.row);
    }
    win.desired_col = visual_col_from_char_idx(&buf.line_text(win.row), win.col, buf.tab_size);
}

pub fn move_word_backward(win: &mut Window, buf: &Buffer) {
    if !valid_row(win, buf) {
        return;
    }
    let text = buf.line_text(win.row);
    let gr = graphemes(&text);

    if gr.is_empty() {
        if win.row > 0 {
            win.row -= 1;
            win.col = buf.line_char_len(win.row);
            win.desired_col =
                visual_col_from_char_idx(&buf.line_text(win.row), win.col, buf.tab_size);
        }
        return;
    }

    let mut nodes = Vec::new();
    let mut char_idx = 0;
    for g in &gr {
        nodes.push((char_idx, *g, g.chars().count()));
        char_idx += g.chars().count();
    }

    let mut idx = nodes.len();
    for (i, &(col, _, _)) in nodes.iter().enumerate() {
        if col >= win.col {
            idx = i;
            break;
        }
    }

    let mut in_word = false;
    let mut past_space = false;

    for i in (0..idx).rev() {
        let (col, g, _) = nodes[i];
        let is_ws = g.trim().is_empty();

        if !is_ws {
            if past_space {
                win.col = col;
                win.desired_col = visual_col_from_char_idx(&text, win.col, buf.tab_size);
                return;
            }
            in_word = true;
        } else if in_word {
            past_space = true;
        }
    }

    if win.row > 0 {
        win.row -= 1;
        win.col = buf.line_char_len(win.row);
    } else {
        win.col = 0;
    }
    win.desired_col = visual_col_from_char_idx(&buf.line_text(win.row), win.col, buf.tab_size);
}

// ---------------------------------------------------------------------------
// Vertical movement
// ---------------------------------------------------------------------------

pub fn move_to_first_line(win: &mut Window, _buf: &Buffer) {
    win.row = 0;
    win.col = 0;
    win.desired_col = 0;
}

pub fn move_to_last_line(win: &mut Window, buf: &Buffer) {
    win.row = buf.len_lines().saturating_sub(1);
    win.col = 0;
    win.desired_col = 0;
}

pub fn page_up(win: &mut Window, buf: &Buffer, jump: usize) {
    let vcol = visual_col_from_char_idx(&buf.line_text(win.row), win.col, buf.tab_size);
    win.desired_col = win.desired_col.max(vcol);

    win.row = win.row.saturating_sub(jump);
    if win.row >= buf.len_lines() {
        win.row = buf.len_lines().saturating_sub(1);
    }
    let new_line = buf.line_text(win.row);
    win.col = char_idx_from_visual_col(&new_line, win.desired_col, buf.tab_size);
}

pub fn page_down(win: &mut Window, buf: &Buffer, jump: usize) {
    let vcol = visual_col_from_char_idx(&buf.line_text(win.row), win.col, buf.tab_size);
    win.desired_col = win.desired_col.max(vcol);

    win.row = (win.row + jump).min(buf.len_lines().saturating_sub(1));
    if win.row >= buf.len_lines() {
        win.row = buf.len_lines().saturating_sub(1);
    }
    let new_line = buf.line_text(win.row);
    win.col = char_idx_from_visual_col(&new_line, win.desired_col, buf.tab_size);
}
