// keybind/block_ops.rs
//! Visual-block yank / delete / paste.

use crate::Editor;

pub fn yank_block(editor: &mut Editor) -> Option<String> {
    let win = editor.active_window();
    let buf = editor.buf();
    let anchor = win.visual_anchor?;

    let r1 = anchor.0.min(win.row);
    let r2 = anchor.0.max(win.row);
    let c1 = anchor.1.min(win.col);
    let c2 = anchor.1.max(win.col);

    let mut lines = Vec::new();
    for r in r1..=r2 {
        if r >= buf.len_lines() {
            continue;
        }
        let line_text = buf.line_text(r);
        let chars: Vec<char> = line_text.chars().collect();

        let start = c1.min(chars.len());
        let end = (c2 + 1).min(chars.len());

        if start < end {
            let chunk: String = chars[start..end].iter().collect();
            lines.push(chunk);
        } else {
            lines.push(String::new());
        }
    }

    Some(lines.join("\n"))
}

pub fn delete_block(editor: &mut Editor) {
    let anchor = match editor.active_window().visual_anchor {
        Some(a) => a,
        None => return,
    };

    let (win_row, win_col) = {
        let win = editor.active_window();
        (win.row, win.col)
    };

    let r1 = anchor.0.min(win_row);
    let r2 = anchor.0.max(win_row);
    let c1 = anchor.1.min(win_col);
    let c2 = anchor.1.max(win_col);

    let buf = editor.buf_mut();
    for r in r1..=r2 {
        if r >= buf.len_lines() {
            continue;
        }

        let line_char_offset = buf.rope.line_to_char(r);
        let line_len = buf.line_char_len(r);

        let start_col = c1.min(line_len);
        let end_col = (c2 + 1).min(line_len);

        if start_col < end_col {
            let remove_start = line_char_offset + start_col;
            let remove_end = line_char_offset + end_col;
            buf.rope.remove(remove_start..remove_end);
        }
    }

    buf.mark_modified();
    buf.parse_syntax();

    // disjoint mutable borrow to safely position and clamp cursor
    let (win, buf) = editor.active_window_and_buf_mut();
    win.row = r1;
    win.col = c1.min(buf.line_char_len(r1));
    win.clamp_cursor(buf);
}

pub fn paste_block(editor: &mut Editor, text: &str) {
    let (win, buf) = editor.active_window_and_buf_mut();
    let (row, col) = (win.row, win.col);

    let lines: Vec<&str> = if text.contains("\r\n") {
        text.split("\r\n").collect()
    } else {
        text.split('\n').collect()
    };

    for (i, line_text) in lines.iter().enumerate() {
        let target_row = row + i;

        while target_row >= buf.len_lines() {
            buf.rope.insert(buf.rope.len_chars(), "\n");
        }

        let line_len = buf.line_char_len(target_row);

        if col > line_len {
            let pad_spaces = " ".repeat(col - line_len);
            let insert_offset = buf.rope.line_to_char(target_row) + line_len;
            buf.rope.insert(insert_offset, &pad_spaces);
        }

        let insert_offset = buf.rope.line_to_char(target_row) + col;
        buf.rope.insert(insert_offset, line_text);
    }

    buf.mark_modified();
    buf.parse_syntax();
}
