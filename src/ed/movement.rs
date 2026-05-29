//! Cursor movement operations — grapheme-safe implementation.
//!
//! Every function takes `(&mut Window, &Buffer)` so that cursor state
//! lives on the Window while the Buffer provides text data.
//! All movements respect unicode display widths and grapheme clusters.

use crate::ed::buffer::Buffer;
use crate::ed::mode::Mode;
use crate::ed::window::Window;
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn valid_row(win: &Window, buf: &Buffer) -> bool {
    win.row < buf.len_lines()
}

fn display_width(s: &str) -> usize {
    UnicodeWidthStr::width(s)
}

fn graphemes(text: &str) -> Vec<&str> {
    UnicodeSegmentation::graphemes(text, true).collect()
}

fn line_display_width(buf: &Buffer, row: usize) -> usize {
    if row >= buf.len_lines() {
        return 0;
    }
    display_width(&buf.line_text(row))
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
        let mut current_col = 0;
        let mut prev_col = 0;

        for grapheme in graphemes(&text) {
            let width = display_width(grapheme);

            // If win.col is exactly at the start of this grapheme
            if current_col >= win.col {
                break;
            }

            // If win.col is inside this wide grapheme (e.g. right half of 🔴)
            // Snap to the START of this wide grapheme
            if current_col + width > win.col {
                prev_col = current_col;
                break;
            }

            prev_col = current_col;
            current_col += width;
        }

        win.col = prev_col;
    } else if win.row > 0 {
        win.row -= 1;
        win.col = line_display_width(buf, win.row);
    }
    win.desired_col = win.col;
}

pub fn move_right(win: &mut Window, buf: &Buffer) {
    if !valid_row(win, buf) {
        return;
    }
    let line_width = line_display_width(buf, win.row);

    if win.col < line_width {
        let text = buf.line_text(win.row);
        let mut current_col = 0;

        for grapheme in graphemes(&text) {
            let width = display_width(grapheme);

            // If win.col is exactly at the start of this grapheme
            if current_col == win.col {
                win.col = current_col + width;
                break;
            }

            // If win.col is inside this wide grapheme (e.g. right half of 🔴)
            if current_col < win.col && current_col + width > win.col {
                win.col = current_col + width; // Snap to the END of it
                break;
            }

            current_col += width;
        }
    } else if win.row + 1 < buf.len_lines() {
        win.row += 1;
        win.col = 0;
    }
    win.desired_col = win.col;
}

pub fn move_line_start(win: &mut Window, _buf: &Buffer) {
    win.col = 0;
    win.desired_col = 0;
}

pub fn move_line_end(win: &mut Window, buf: &Buffer, mode: Mode) {
    if !valid_row(win, buf) {
        return;
    }
    let width = line_display_width(buf, win.row);
    win.col = if mode == Mode::Normal {
        width.saturating_sub(1)
    } else {
        width
    };
    win.desired_col = win.col;
}

pub fn move_word_forward(win: &mut Window, buf: &Buffer) {
    if !valid_row(win, buf) {
        return;
    }
    let text = buf.line_text(win.row);
    let gr = graphemes(&text);
    let mut current_col = 0;
    let mut in_word = false;
    let mut past_word = false;

    for g in &gr {
        let width = display_width(g);

        if current_col > win.col {
            if !g.trim().is_empty() {
                if past_word {
                    win.col = current_col;
                    win.desired_col = win.col;
                    return;
                }
                in_word = true;
            } else {
                if in_word {
                    past_word = true;
                }
            }
        }

        current_col += width;
    }

    if win.row + 1 < buf.len_lines() {
        win.row += 1;
        win.col = 0;
    } else {
        win.col = line_display_width(buf, win.row);
    }
    win.desired_col = win.col;
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
            win.col = line_display_width(buf, win.row);
            win.desired_col = win.col;
        }
        return;
    }

    // Map graphemes to their start columns
    let mut nodes = Vec::new();
    let mut current_col = 0;
    for g in &gr {
        let width = display_width(g);
        nodes.push((current_col, *g));
        current_col += width;
    }

    // Find the grapheme at or after the cursor
    let mut idx = nodes.len();
    for (i, &(col, _)) in nodes.iter().enumerate() {
        if col >= win.col {
            idx = i;
            break;
        }
    }

    let mut in_word = false;
    let mut past_space = false;

    for i in (0..idx).rev() {
        let (col, g) = nodes[i];
        let is_ws = g.trim().is_empty();

        if !is_ws {
            if past_space {
                win.col = col;
                win.desired_col = win.col;
                return;
            }
            in_word = true;
        } else {
            if in_word {
                past_space = true;
            }
        }
    }

    if win.row > 0 {
        win.row -= 1;
        win.col = line_display_width(buf, win.row);
    } else {
        win.col = 0;
    }
    win.desired_col = win.col;
}

// ---------------------------------------------------------------------------
// Vertical movement
// ---------------------------------------------------------------------------

pub fn move_up(win: &mut Window, buf: &Buffer) {
    if !valid_row(win, buf) {
        return;
    }
    if win.row > 0 {
        win.desired_col = win.desired_col.max(win.col);
        win.row -= 1;
        let max_col = line_display_width(buf, win.row);
        win.col = win.desired_col.min(max_col);
    }
}

pub fn move_down(win: &mut Window, buf: &Buffer) {
    if !valid_row(win, buf) {
        return;
    }
    if win.row + 1 < buf.len_lines() {
        win.desired_col = win.desired_col.max(win.col);
        win.row += 1;
        let max_col = line_display_width(buf, win.row);
        win.col = win.desired_col.min(max_col);
    }
}

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
    win.desired_col = win.desired_col.max(win.col);
    win.row = win.row.saturating_sub(jump);
    if win.row >= buf.len_lines() {
        win.row = buf.len_lines().saturating_sub(1);
    }
    let max_col = line_display_width(buf, win.row);
    win.col = win.desired_col.min(max_col);
}

pub fn page_down(win: &mut Window, buf: &Buffer, jump: usize) {
    win.desired_col = win.desired_col.max(win.col);
    win.row = (win.row + jump).min(buf.len_lines().saturating_sub(1));
    if win.row >= buf.len_lines() {
        win.row = buf.len_lines().saturating_sub(1);
    }
    let max_col = line_display_width(buf, win.row);
    win.col = win.desired_col.min(max_col);
}
