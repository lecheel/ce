//! Main buffer area rendering (text, line numbers, ghost text, cursor).
//!
//! Supports multiple split windows rendered from the Editor's layout tree.
//! Each pane shows its own buffer with independent scroll/cursor state.
//! Only the active window displays ghost text, completions, and the
//! terminal cursor.

use ratatui::{
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::Paragraph,
    Frame,
};

use crate::comp::state::find_prefix_overlap;
use crate::ed::buffer::Buffer;
use crate::ed::buffer::VirtualLine;
use crate::ed::diff_align::DiffAlignment;
use crate::ed::editor::Editor;
use crate::ed::mode::Mode;
use crate::ed::window::{Window, WindowPosition};
use crate::render::helpers::display_width;

/// Returns the rope line index to render for a given virtual row,
/// or `None` if this row is a padding filler.
fn resolve_virtual_row(
    alignment: Option<&DiffAlignment>,
    is_head_pane: bool,
    virtual_row: usize,
) -> Option<usize> {
    let Some(align) = alignment else {
        return Some(virtual_row); // no alignment active — 1:1 mapping
    };
    let map = if is_head_pane {
        &align.left
    } else {
        &align.right
    };
    match map.get(virtual_row) {
        Some(VirtualLine::Real(n)) => Some(*n),
        Some(VirtualLine::Padding) | None => None,
    }
}

/// Total number of virtual rows for a buffer (alignment-aware).
fn virtual_line_count(buf: &Buffer) -> usize {
    if let Some(ref a) = buf.diff_alignment {
        a.len()
    } else {
        buf.len_lines()
    }
}

fn common_prefix_len(a: &str, b: &str) -> usize {
    a.chars().zip(b.chars()).take_while(|(x, y)| x == y).count()
}

/// True when this buffer holds the HEAD (original) side of a diffthis split.
fn is_head_pane(buf: &Buffer) -> bool {
    buf.kind == crate::ed::buffer::BufferKind::GitDiffHead
}

// Helper to convert per-char highlights into grouped Spans
fn styled_spans_from_highlights(
    chars: &[char],
    default_style: Style,
    highlights: &[Option<Style>],
    selected_mask: &[bool],
    search_mask: &[bool],
) -> Vec<Span<'static>> {
    let mut spans = Vec::new();
    if chars.is_empty() {
        return spans;
    }

    let get_style = |idx: usize| -> Style {
        let mut base = highlights
            .get(idx)
            .copied()
            .flatten()
            .unwrap_or(default_style);
        if selected_mask.get(idx).copied().unwrap_or(false) {
            // Apply a nice dark slate blue selection background
            base = base.bg(Color::Rgb(40, 50, 80));
        } else if search_mask.get(idx).copied().unwrap_or(false) {
            base = base.fg(Color::Black).bg(Color::Yellow); // Vim-yellow search highlight
        }
        base
    };

    let mut current_style = get_style(0);
    let mut chunk_start = 0;

    for (i, _) in chars.iter().enumerate() {
        let target_style = get_style(i);
        if target_style != current_style {
            if chunk_start < i {
                let text: String = chars[chunk_start..i].iter().collect();
                spans.push(Span::styled(text, current_style));
            }
            current_style = target_style;
            chunk_start = i;
        }
    }

    if chunk_start < chars.len() {
        let text: String = chars[chunk_start..].iter().collect();
        spans.push(Span::styled(text, current_style));
    }

    spans
}

// ═══════════════════════════════════════════════════════════════════
// Public entry point
// ═══════════════════════════════════════════════════════════════════

/// Render all editor windows into `area`.
///
/// Computes layout positions via the editor's layout tree, renders
/// each window pane, draws dividers between them, and positions the
/// terminal cursor in the active pane.
///
/// **Caller change:** replace `draw_buffer(f, area, &editor)` with
/// `draw_windows(f, area, &mut editor)`.
pub fn draw_windows(f: &mut Frame, area: Rect, editor: &mut Editor) {
    if area.width == 0 || area.height == 0 {
        return;
    }

    // 1. Compute layout positions from the layout tree
    let wp = WindowPosition::new(
        area.x as usize,
        area.y as usize,
        area.width as usize,
        area.height as usize,
    );
    editor.layout_windows_default(wp);

    let active_idx = editor.active_window_index();
    let mode = editor.mode();

    // 2. Render each pane
    for (idx, win) in editor.all_windows().iter().enumerate() {
        let pos = win.position;
        if !pos.is_visible() {
            continue;
        }

        let rect = Rect::new(
            pos.x as u16,
            pos.y as u16,
            pos.width as u16,
            pos.height as u16,
        );

        // let buf = editor.buf_by_id(win.buffer_id());
        let is_active = idx == active_idx;

        // Only show ghost text in Insert / Brief modes for the active window
        let ghost = if is_active && (mode == Mode::Insert || mode == Mode::Brief) {
            editor.ghost_text()
        } else {
            None
        };

        if let Some(buf) = editor.buf_by_id(win.buffer_id()) {
            draw_pane(f, rect, win, buf, editor, mode, is_active, ghost);
        }
    }

    // 3. Draw dividers between panes
    draw_dividers(f, editor.all_windows());
}

// ═══════════════════════════════════════════════════════════════════
// Single pane renderer
// ═══════════════════════════════════════════════════════════════════
fn draw_pane(
    f: &mut Frame,
    area: Rect,
    win: &Window,
    buf: &Buffer,
    editor: &Editor,
    mode: Mode,
    is_active: bool,
    ghost_text: Option<&str>,
) {
    let viewport_height = area.height as usize;
    let scroll = win.scroll_line;
    let cursor_row = win.row;
    let cursor_col = win.col;

    let total_virtual = virtual_line_count(buf);
    let head_pane = is_head_pane(buf);

    let gutter_width = crate::ed::gutter::gutter_width(buf, win, &editor.config);
    let gutter_style = if is_active {
        Style::default().fg(Color::Rgb(90, 90, 100))
    } else {
        Style::default().fg(Color::Rgb(55, 55, 65))
    };
    let text_style = if is_active {
        Style::default().fg(Color::Rgb(150, 150, 150))
    } else {
        Style::default().fg(Color::Rgb(140, 140, 140))
    };

    // Padding line style — subtle, clearly not real content
    let pad_bg = Color::Rgb(28, 28, 34);
    let pad_fg = Color::Rgb(60, 60, 72);
    let pad_style = Style::default().fg(pad_fg).bg(pad_bg);

    let mut rendered: Vec<Line> = Vec::with_capacity(viewport_height);
    let end = scroll.saturating_add(viewport_height).min(total_virtual);

    let mut rendered_cursor_x: u16 = 0;
    let mut rendered_cursor_y: u16 = 0;

    let is_block_cursor = (mode == Mode::Normal
        || mode == Mode::Search
        || mode == Mode::Visual
        || mode == Mode::VisualLine)
        && is_active;

    for virtual_row in scroll..end {
        // ── Resolve real rope row (or padding) ───────────────────────
        let real_row_opt = resolve_virtual_row(buf.diff_alignment.as_ref(), head_pane, virtual_row);

        // ── Padding line ─────────────────────────────────────────────
        let Some(i) = real_row_opt else {
            let pad_gutter = " ".repeat(gutter_width);
            // Fill available width with a thin dash pattern
            let text_cols = (area.width as usize).saturating_sub(gutter_width);
            let filler: String = "─ "
                .chars()
                .cycle()
                .take(text_cols.saturating_sub(1))
                .collect();

            rendered.push(Line::from(vec![
                Span::styled(pad_gutter, gutter_style),
                Span::styled(filler, pad_style),
            ]));

            // The cursor should never land on a padding row, but
            // if it does, treat it as an off-cursor line so nothing
            // moves the terminal cursor here.
            continue;
        };

        // ── Normal line (real content) ────────────────────────────────
        let is_cursor_line = i == cursor_row;
        let hscroll = win.scroll_col;
        let line_text = buf.line_text(i);
        let chars: Vec<char> = line_text.chars().skip(hscroll).collect();
        let col = cursor_col.saturating_sub(hscroll);

        // Visual selection mask
        let selected_mask: Vec<bool> = (0..chars.len())
            .map(|c_idx| {
                if mode == Mode::Visual || mode == Mode::VisualLine {
                    is_char_selected(win, i, c_idx + hscroll, mode)
                } else {
                    false
                }
            })
            .collect();

        // Search highlight mask
        let mut search_mask = vec![false; chars.len()];
        if let Some(ref query) = editor.last_search_query {
            if !query.is_empty() {
                let line_str: String = chars.iter().collect();
                let mut start = 0;
                while let Some(pos) = line_str[start..].find(query.as_str()) {
                    let abs_pos = start + pos;
                    let char_len = query.chars().count();
                    for offset in 0..char_len {
                        if abs_pos + offset < search_mask.len() {
                            search_mask[abs_pos + offset] = true;
                        }
                    }
                    start = abs_pos + char_len.max(1);
                }
            }
        }

        let gutter_spans = crate::ed::gutter::render_gutter_line(buf, win, i, &editor.config);

        if is_cursor_line {
            let col = col.min(chars.len());
            let raw_line = buf.line_text(i);
            let mut highlights = buf.syntax.get_line_highlights(i, &raw_line);
            if hscroll < highlights.len() {
                highlights = highlights.split_off(hscroll);
            } else {
                highlights.clear();
            }
            while highlights.len() < chars.len() {
                highlights.push(None);
            }

            let visual_before_len = display_width(&chars[..col].iter().collect::<String>()) as u16;
            let mut spans = gutter_spans;

            // Text before cursor
            let before_col = col.min(highlights.len());
            spans.extend(styled_spans_from_highlights(
                &chars[..col],
                text_style,
                &highlights[..before_col],
                &selected_mask[..before_col],
                &search_mask[..before_col],
            ));

            // Ghost text
            let before_str: String = chars[..col].iter().collect();
            let after: String = chars[col..].iter().collect();
            let display_ghost = if let Some(ghost) = ghost_text {
                let overlap_len = find_prefix_overlap(&before_str, ghost);
                let ghost_chars: Vec<char> = ghost.chars().collect();
                let overlap_len = overlap_len.min(ghost_chars.len());
                let display_ghost_str: String = ghost_chars[overlap_len..].iter().collect();
                if !display_ghost_str.is_empty() {
                    let suffix_overlap = common_prefix_len(&after, &display_ghost_str);
                    let suffix_overlap = suffix_overlap.min(display_ghost_str.chars().count());
                    let final_ghost: String =
                        display_ghost_str.chars().skip(suffix_overlap).collect();
                    if !final_ghost.is_empty() {
                        Some(final_ghost)
                    } else {
                        None
                    }
                } else {
                    None
                }
            } else {
                None
            };

            if is_block_cursor {
                let cursor_style = Style::default().fg(Color::Rgb(20, 20, 20)).bg(Color::Cyan);
                if col < chars.len() {
                    spans.push(Span::styled(chars[col].to_string(), cursor_style));
                    let after_col = col.saturating_add(1);
                    spans.extend(styled_spans_from_highlights(
                        chars.get(after_col..).unwrap_or(&[]),
                        text_style,
                        highlights.get(after_col..).unwrap_or(&[]),
                        selected_mask.get(after_col..).unwrap_or(&[]),
                        search_mask.get(after_col..).unwrap_or(&[]),
                    ));
                } else {
                    spans.push(Span::styled(" ".to_string(), cursor_style));
                }
            } else {
                if let Some(ref ghost) = display_ghost {
                    spans.push(Span::styled(
                        ghost.clone(),
                        Style::default()
                            .fg(Color::Rgb(110, 110, 140))
                            .add_modifier(Modifier::ITALIC),
                    ));
                }
                spans.extend(styled_spans_from_highlights(
                    &chars[col..],
                    text_style,
                    highlights.get(col..).unwrap_or(&[]),
                    selected_mask.get(col..).unwrap_or(&[]),
                    search_mask.get(col..).unwrap_or(&[]),
                ));
            }

            rendered.push(Line::from(spans));

            if is_active {
                rendered_cursor_y = area
                    .y
                    .saturating_add((virtual_row.saturating_sub(scroll)) as u16);
                let offset_x = gutter_width
                    .saturating_add(visual_before_len as usize)
                    .min(u16::MAX as usize) as u16;
                rendered_cursor_x = area.x.saturating_add(offset_x);
            }
        } else {
            // Non-cursor line
            let raw_line = buf.line_text(i);
            let mut highlights = buf.syntax.get_line_highlights(i, &raw_line);
            while highlights.len() < chars.len() {
                highlights.push(None);
            }
            let mut spans = gutter_spans;
            spans.extend(styled_spans_from_highlights(
                &chars,
                text_style,
                &highlights,
                &selected_mask,
                &search_mask,
            ));
            rendered.push(Line::from(spans));
        }
    }

    // EOF padding (rows past end of virtual content)
    while rendered.len() < viewport_height {
        let pad_str = " ".repeat(gutter_width);
        rendered.push(Line::from(vec![
            Span::styled(pad_str, gutter_style),
            Span::styled(String::new(), Style::default()),
        ]));
    }

    let paragraph = Paragraph::new(rendered);
    f.render_widget(paragraph, area);

    // Terminal cursor (active window, insert/brief modes only)
    if is_active {
        match mode {
            Mode::Insert | Mode::Brief => {
                let cx = rendered_cursor_x.min(area.right().saturating_sub(1));
                let cy = rendered_cursor_y.min(area.bottom().saturating_sub(1));
                f.set_cursor_position((cx, cy));
            }
            _ => {}
        }
    }
}

// ═══════════════════════════════════════════════════════════════════
// Dividers between split panes
// ═══════════════════════════════════════════════════════════════════

fn draw_dividers(f: &mut Frame, windows: &[Window]) {
    if windows.len() <= 1 {
        return;
    }

    let divider_style = Style::default().fg(Color::Rgb(80, 80, 100));

    for i in 0..windows.len() {
        let a = windows[i].position;
        if !a.is_visible() {
            continue;
        }
        for j in (i.saturating_add(1))..windows.len() {
            let b = windows[j].position;
            if !b.is_visible() {
                continue;
            }

            // Horizontal divider: pane A is directly above pane B
            // (1-row gap left by the layout separator)
            if a.overlaps_horizontally(&b) && b.y == a.y.saturating_add(a.height).saturating_add(1)
            {
                let y = a.y.saturating_add(a.height).min(u16::MAX as usize) as u16;
                let x_start = a.x.max(b.x).min(u16::MAX as usize) as u16;
                let x_end = (a.x.saturating_add(a.width))
                    .min(b.x.saturating_add(b.width))
                    .min(u16::MAX as usize) as u16;
                let width = x_end.saturating_sub(x_start);
                if width > 0 {
                    let line = Line::from(Span::styled("─".repeat(width as usize), divider_style));
                    f.render_widget(Paragraph::new(line), Rect::new(x_start, y, width, 1));
                }
            }

            // Vertical divider: pane A is directly left of pane B
            // (1-col gap left by the layout separator)
            if a.overlaps_vertically(&b) && b.x == a.x.saturating_add(a.width).saturating_add(1) {
                let x = a.x.saturating_add(a.width).min(u16::MAX as usize) as u16;
                let y_start = a.y.max(b.y).min(u16::MAX as usize) as u16;
                let y_end = (a.y.saturating_add(a.height))
                    .min(b.y.saturating_add(b.height))
                    .min(u16::MAX as usize) as u16;
                let height = y_end.saturating_sub(y_start);
                if height > 0 {
                    let lines: Vec<Line> = (0..height)
                        .map(|_| Line::from(Span::styled("│", divider_style)))
                        .collect();
                    f.render_widget(Paragraph::new(lines), Rect::new(x, y_start, 1, height));
                }
            }
        }
    }
}

/// Checks if a given row/col coordinate is inside the active selection window.
pub fn is_char_selected(win: &Window, row: usize, col: usize, mode: Mode) -> bool {
    let Some(anchor) = win.visual_anchor else {
        return false;
    };
    let cursor = (win.row, win.col);

    let (start, end) = if anchor.0 < cursor.0 || (anchor.0 == cursor.0 && anchor.1 <= cursor.1) {
        (anchor, cursor)
    } else {
        (cursor, anchor)
    };

    if mode == Mode::VisualLine {
        row >= start.0 && row <= end.0
    } else {
        if row < start.0 || row > end.0 {
            false
        } else if row > start.0 && row < end.0 {
            true
        } else if start.0 == end.0 {
            col >= start.1 && col <= end.1
        } else if row == start.0 {
            col >= start.1
        } else if row == end.0 {
            col <= end.1
        } else {
            false
        }
    }
}
