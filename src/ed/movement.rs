//! Cursor movement operations.
//!
//! Every function takes `(&mut Window, &Buffer)` so that cursor state
//! lives on the Window while the Buffer provides text data.

use crate::ed::buffer::Buffer;
use crate::ed::mode::Mode;
use crate::ed::window::Window;

// ---------------------------------------------------------------------------
// Horizontal movement
// ---------------------------------------------------------------------------
fn valid_row(win: &Window, buf: &Buffer) -> bool {
    win.row < buf.len_lines()
}

pub fn move_left(win: &mut Window, buf: &Buffer) {
    if !valid_row(win, buf) {
        return;
    }
    if win.col > 0 {
        win.col -= 1;
    } else if win.row > 0 {
        win.row -= 1;
        win.col = buf.line_char_len(win.row);
    }
    win.desired_col = win.col;
}

pub fn move_right(win: &mut Window, buf: &Buffer) {
    if !valid_row(win, buf) {
        return;
    }
    let line_len = buf.line_char_len(win.row);
    if win.col < line_len {
        win.col += 1;
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
    win.col = if mode == Mode::Normal {
        buf.line_char_len(win.row).saturating_sub(1)
    } else {
        buf.line_char_len(win.row)
    };
    win.desired_col = win.col;
}

pub fn move_word_forward(win: &mut Window, buf: &Buffer) {
    if !valid_row(win, buf) {
        return;
    }
    let text = buf.line_text(win.row);
    let chars: Vec<char> = text.chars().collect();
    let mut pos = win.col;

    while pos < chars.len() && !chars[pos].is_whitespace() {
        pos += 1;
    }
    while pos < chars.len() && chars[pos].is_whitespace() {
        pos += 1;
    }

    if pos >= chars.len() && win.row + 1 < buf.len_lines() {
        win.row += 1;
        win.col = 0;
    } else {
        win.col = pos;
    }
    win.desired_col = win.col;
}

pub fn move_word_backward(win: &mut Window, buf: &Buffer) {
    if !valid_row(win, buf) {
        return;
    }
    let text = buf.line_text(win.row);
    let chars: Vec<char> = text.chars().collect();

    if chars.is_empty() {
        if win.row > 0 {
            win.row -= 1;
            win.col = buf.line_char_len(win.row);
        }
        win.desired_col = win.col;
        return;
    }
    let mut pos = win.col.saturating_sub(1);

    while pos > 0 && chars.get(pos).map_or(false, |c| c.is_whitespace()) {
        pos -= 1;
    }
    while pos > 0 && chars.get(pos).map_or(false, |c| !c.is_whitespace()) {
        pos -= 1;
    }

    if pos == 0 && win.row > 0 {
        win.row -= 1;
        win.col = buf.line_char_len(win.row);
    } else {
        win.col = pos;
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
        let max = buf.line_char_len(win.row);
        win.col = win.desired_col.min(max);
    }
}

pub fn move_down(win: &mut Window, buf: &Buffer) {
    if !valid_row(win, buf) {
        return;
    }
    if win.row + 1 < buf.len_lines() {
        win.desired_col = win.desired_col.max(win.col);
        win.row += 1;
        let max = buf.line_char_len(win.row);
        win.col = win.desired_col.min(max);
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
    let max = buf.line_char_len(win.row);
    win.col = win.desired_col.min(max);
}

pub fn page_down(win: &mut Window, buf: &Buffer, jump: usize) {
    win.desired_col = win.desired_col.max(win.col);
    win.row = (win.row + jump).min(buf.len_lines().saturating_sub(1));
    if win.row >= buf.len_lines() {
        win.row = buf.len_lines().saturating_sub(1);
    }
    let max = buf.line_char_len(win.row);
    win.col = win.desired_col.min(max);
}
