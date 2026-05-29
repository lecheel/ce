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
use unicode_segmentation::UnicodeSegmentation;

/// Describes what character occupies a given visual column in a line.
#[derive(Debug)]
enum VisualColContent {
    /// A space character, carrying its char index in the line.
    Space(usize),
    /// The column falls inside a tab character.
    Tab,
    /// A non-whitespace character occupies the column.
    NonSpace,
    /// The column is past the end of the line.
    PastEnd,
}

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
    line_bg: Option<Color>, // <-- NEW parameter
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
            base = base.bg(Color::Rgb(40, 50, 80));
        } else if search_mask.get(idx).copied().unwrap_or(false) {
            base = base.fg(Color::Black).bg(Color::Yellow);
        } else if base.bg.is_none() {
            if let Some(bg) = line_bg {
                base = base.bg(bg);
            }
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

    // ── Pre-compute indent guide depths for all visible lines ──────
    let tab_size = editor.config.tab_size.max(1);
    let indent_guide_style = if is_active {
        Style::default().fg(Color::Rgb(55, 55, 75))
    } else {
        Style::default().fg(Color::Rgb(40, 40, 55))
    };

    let mut line_info: Vec<(usize, bool)> = Vec::with_capacity(end.saturating_sub(scroll));
    for vr in scroll..end {
        let real_row_opt = resolve_virtual_row(buf.diff_alignment.as_ref(), head_pane, vr);
        if let Some(i) = real_row_opt {
            let text = buf.line_text(i);
            let level = indent_level(&text, tab_size);
            let blank = is_blank_line(&text);
            line_info.push((level, blank));
        } else {
            line_info.push((0, true)); // padding row → treat as blank
        }
    }
    let guide_depths: Vec<usize> = if editor.config.show_indent_guides {
        compute_guide_depths(&line_info)
    } else {
        vec![0; line_info.len()]
    };

    let is_block_cursor = (mode == Mode::Normal
        || mode == Mode::Search
        || mode == Mode::Visual
        || mode == Mode::VisualLine
        || mode == Mode::VisualBlock)
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

        // OPTIMIZATION: Only collect and process characters that can fit in the viewport
        let max_visible_chars = (area.width as usize).saturating_mul(2).max(100);
        let mut chars: Vec<char> = line_text
            .chars()
            .skip(hscroll)
            .take(max_visible_chars)
            .collect();
        let col = cursor_col.saturating_sub(hscroll);

        // Visual selection mask
        let is_selecting = mode == Mode::Visual
            || mode == Mode::VisualLine
            || mode == Mode::VisualBlock
            || editor.visual_block_insert_state.is_some()
            || (mode == Mode::Brief && win.visual_anchor.is_some())
            || (mode == Mode::Command && win.visual_anchor.is_some()); // NEW: Keep highlight in command mode

        let block_insert_col = editor.visual_block_insert_state.as_ref().map(|s| s.col);

        let mut selected_mask: Vec<bool> = (0..chars.len())
            .map(|c_idx| {
                if is_selecting {
                    // If we are in Command mode with a visual anchor, use prev_mode
                    // to correctly render Visual vs VisualLine vs VisualBlock selections
                    let eval_mode = if mode == Mode::Command && win.visual_anchor.is_some() {
                        editor.prev_mode
                    } else if block_insert_col.is_some() {
                        Mode::VisualBlock
                    } else {
                        mode
                    };
                    is_char_selected_ex(win, i, c_idx + hscroll, eval_mode, block_insert_col)
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

        // ── Cursor Line Background Highlight ──────────────────────────
        let line_bg = if is_cursor_line && editor.config.cursor_line_highlight {
            Some(
                editor
                    .config
                    .resolve_color(&editor.config.cursor_line_highlight_color),
            )
        } else {
            None
        };

        if is_cursor_line {
            // let col = col.min(chars.len());
            let raw_line = buf.line_text(i);
            let mut highlights = buf.syntax.get_line_highlights(i, &raw_line);

            // Skip and clamp syntax highlights relative to viewport scroll
            if hscroll < highlights.len() {
                highlights = highlights.split_off(hscroll);
            } else {
                highlights.clear();
            }
            highlights.truncate(chars.len());
            while highlights.len() < chars.len() {
                highlights.push(None);
            }

            // ── Apply indent guides (cursor line) ──────────────────
            apply_indent_guides(
                &mut chars,
                &mut highlights,
                &mut selected_mask,
                &mut search_mask,
                &raw_line,
                hscroll,
                tab_size,
                guide_depths[virtual_row - scroll],
                indent_guide_style,
            );

            // ── Grapheme-aware cursor positioning ──────────────────────
            let mut vis_col = 0;
            let mut char_offset = 0;
            let mut cursor_grapheme: Option<&str> = None;
            let mut cursor_width = 1;

            for g in raw_line.graphemes(true) {
                let width = display_width(g);
                if vis_col == col {
                    cursor_grapheme = Some(g);
                    cursor_width = width;
                    break;
                }
                if vis_col < col && vis_col + width > col {
                    // Cursor is on the right half of a wide grapheme (e.g. 🔴)
                    cursor_grapheme = Some(g);
                    cursor_width = width;
                    break;
                }
                vis_col += width;
                char_offset += g.chars().count();
            }

            let safe_offset = char_offset.min(chars.len());
            let visual_before_len =
                display_width(&chars[..safe_offset].iter().collect::<String>()) as u16;
            let mut spans = gutter_spans;

            // Text before cursor
            let before_len = safe_offset.min(highlights.len());
            spans.extend(styled_spans_from_highlights(
                &chars[..safe_offset],
                text_style,
                &highlights[..before_len],
                &selected_mask[..before_len],
                &search_mask[..before_len],
                line_bg,
            ));

            // Ghost text
            let before_str: String = chars[..safe_offset].iter().collect();
            let after: String = chars[safe_offset..].iter().collect();
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
                let fg_color = editor
                    .config
                    .resolve_color(&editor.config.cursor_text_color);
                let bg_color = editor
                    .config
                    .resolve_color(&editor.config.cursor_highlight_color);
                let cursor_style = Style::default().fg(fg_color).bg(bg_color);

                if let Some(g) = cursor_grapheme {
                    spans.push(Span::styled(g.to_string(), cursor_style));
                    let after_offset = safe_offset + g.chars().count();
                    spans.extend(styled_spans_from_highlights(
                        chars.get(after_offset..).unwrap_or(&[]),
                        text_style,
                        highlights.get(after_offset..).unwrap_or(&[]),
                        selected_mask.get(after_offset..).unwrap_or(&[]),
                        search_mask.get(after_offset..).unwrap_or(&[]),
                        line_bg,
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
                    chars.get(safe_offset..).unwrap_or(&[]),
                    text_style,
                    highlights.get(safe_offset..).unwrap_or(&[]),
                    selected_mask.get(safe_offset..).unwrap_or(&[]),
                    search_mask.get(safe_offset..).unwrap_or(&[]),
                    line_bg,
                ));
            }

            // ── Pad line highlight to full viewport width ──────────
            if let Some(bg) = line_bg {
                let width_used: usize = spans.iter().map(|s| display_width(&s.content)).sum();
                let padding_needed = (area.width as usize).saturating_sub(width_used);
                if padding_needed > 0 {
                    spans.push(Span::styled(
                        " ".repeat(padding_needed),
                        Style::default().bg(bg),
                    ));
                }
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

            // Skip and clamp syntax highlights relative to viewport scroll (fixes off-by-hscroll bug)
            if hscroll < highlights.len() {
                highlights = highlights.split_off(hscroll);
            } else {
                highlights.clear();
            }
            highlights.truncate(chars.len());
            while highlights.len() < chars.len() {
                highlights.push(None);
            }

            // ── Apply indent guides (non-cursor line) ──────────────
            apply_indent_guides(
                &mut chars,
                &mut highlights,
                &mut selected_mask,
                &mut search_mask,
                &raw_line,
                hscroll,
                tab_size,
                guide_depths[virtual_row - scroll],
                indent_guide_style,
            );

            let mut spans = gutter_spans;
            spans.extend(styled_spans_from_highlights(
                &chars,
                text_style,
                &highlights,
                &selected_mask,
                &search_mask,
                line_bg, // Will be None for non-cursor lines
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

/// Like `is_char_selected` but accepts an optional frozen cursor column
/// used during visual-block insert so the highlight rectangle doesn't
/// drift as the user types.
pub fn is_char_selected_ex(
    win: &Window,
    row: usize,
    col: usize,
    mode: Mode,
    frozen_cursor_col: Option<usize>,
) -> bool {
    let Some(anchor) = win.visual_anchor else {
        return false;
    };

    // Use the frozen col for VisualBlock so typing doesn't shift the rect.
    let effective_cursor_col = if mode == Mode::VisualBlock {
        frozen_cursor_col.unwrap_or(win.col)
    } else {
        win.col
    };

    let cursor = (win.row, effective_cursor_col);

    let (start, end) = if anchor.0 < cursor.0 || (anchor.0 == cursor.0 && anchor.1 <= cursor.1) {
        (anchor, cursor)
    } else {
        (cursor, anchor)
    };

    if mode == Mode::VisualLine {
        row >= start.0 && row <= end.0
    } else if mode == Mode::VisualBlock {
        let r1 = anchor.0.min(win.row);
        let r2 = anchor.0.max(win.row);
        let c1 = anchor.1.min(effective_cursor_col);
        let c2 = anchor.1.max(effective_cursor_col);
        row >= r1 && row <= r2 && col >= c1 && col <= c2
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

pub fn is_char_selected(win: &Window, row: usize, col: usize, mode: Mode) -> bool {
    is_char_selected_ex(win, row, col, mode, None)
}

// ═══════════════════════════════════════════════════════════════════
// Indent Guide Helpers
// ═══════════════════════════════════════════════════════════════════

/// Compute the indent level of a line in units of `tab_size`.
/// Leading spaces count 1 column each; tabs snap to the next
/// `tab_size` boundary.
fn indent_level(line_text: &str, tab_size: usize) -> usize {
    let tab_size = tab_size.max(1);
    let mut col = 0;
    for ch in line_text.chars() {
        match ch {
            ' ' => col += 1,
            '\t' => col = (col / tab_size + 1) * tab_size,
            _ => break,
        }
    }
    col / tab_size
}

/// True when the line contains only whitespace (or is empty).
fn is_blank_line(line_text: &str) -> bool {
    line_text.trim().is_empty()
}

/// Compute the effective guide depth for each visible line.
///
/// Non-blank lines use their own indent level. Blank lines inherit
/// the **minimum** indent of the nearest non-blank neighbours above
/// and below so that guides continue through empty lines inside an
/// indented block.
fn compute_guide_depths(levels: &[(usize, bool)]) -> Vec<usize> {
    let n = levels.len();
    let mut depths: Vec<usize> = levels.iter().map(|(l, _)| *l).collect();

    for i in 0..n {
        let (level, blank) = levels[i];
        if blank && level == 0 {
            let above = (0..i)
                .rev()
                .find_map(|j| {
                    let (l, b) = levels[j];
                    if !b && l > 0 {
                        Some(l)
                    } else {
                        None
                    }
                })
                .unwrap_or(0);
            let below = (i + 1..n)
                .find_map(|j| {
                    let (l, b) = levels[j];
                    if !b && l > 0 {
                        Some(l)
                    } else {
                        None
                    }
                })
                .unwrap_or(0);

            if above > 0 && below > 0 {
                depths[i] = above.min(below);
            } else if above > 0 {
                // Still inside a block that ends below the viewport
                depths[i] = above;
            }
            // below-only: block hasn't started yet — no guides
        }
    }

    depths
}

/// Inspects a line of text and determines what occupies `visual_col`.
fn visual_col_content(line_text: &str, visual_col: usize, tab_size: usize) -> VisualColContent {
    let mut current_col = 0;
    // Use chars().enumerate() instead of char_indices() to get character offsets rather than byte offsets
    for (char_idx, ch) in line_text.chars().enumerate() {
        let width = if ch == '\t' {
            tab_size - (current_col % tab_size)
        } else {
            display_width(&ch.to_string())
        };

        if current_col == visual_col {
            log::debug!(
                "VIS_COL: visual_col={} exactly matched char_idx={} ch='{}'",
                visual_col,
                char_idx,
                ch
            );
            return if ch == ' ' {
                VisualColContent::Space(char_idx)
            } else if ch == '\t' {
                VisualColContent::Tab
            } else {
                VisualColContent::NonSpace
            };
        }

        // The visual column falls in the middle of a wide char or tab
        if current_col < visual_col && current_col + width > visual_col {
            log::debug!(
                "VIS_COL: visual_col={} falls inside char_idx={} ch='{}' (cols {}..{})",
                visual_col,
                char_idx,
                ch,
                current_col,
                current_col + width
            );
            return if ch == '\t' {
                VisualColContent::Tab
            } else {
                VisualColContent::NonSpace // Wide CJK char
            };
        }
        current_col += width;
    }
    log::debug!(
        "VIS_COL: visual_col={} is PastEnd (line visual width={})",
        visual_col,
        current_col
    );
    VisualColContent::PastEnd
}

/// Apply indent guide characters to a line's character and style arrays.
///
/// For each indent level 1..=guide_depth the space at the visual column
/// `level * tab_size - 1` is replaced with `│` and styled.
///
/// When the line is shorter than a guide position (truly empty line),
/// the arrays are extended with blank padding so the guide can still
/// be rendered — this makes guides continuous through blank lines.
///
/// If a guide position falls inside a tab or on a non-space character,
/// the guide hides itself to avoid distorting alignment.
/// Apply indent guide characters to a line's character and style arrays.
///
/// For each indent level 1..=guide_depth the space at the visual column
/// `(level - 1) * tab_size` (the FIRST space of the indent block) is
/// replaced with `│` and styled.
///
/// When the line is shorter than a guide position (truly empty line),
/// the arrays are extended with blank padding so the guide can still
/// be rendered — this makes guides continuous through blank lines.
///
/// If a guide position falls inside a tab or on a non-space character,
/// the guide hides itself to avoid distorting alignment.
fn apply_indent_guides(
    chars: &mut Vec<char>,
    highlights: &mut Vec<Option<Style>>,
    selected_mask: &mut Vec<bool>,
    search_mask: &mut Vec<bool>,
    line_text: &str,
    hscroll: usize,
    tab_size: usize,
    guide_depth: usize,
    style: Style,
) {
    if guide_depth == 0 {
        return;
    }

    for level in 1..=guide_depth {
        // FIX: Draw the guide at the START of the indent level.
        // Level 1 -> column 0, Level 2 -> column 4, Level 3 -> column 8, etc.
        let abs_pos = (level - 1) * tab_size;

        if abs_pos < hscroll {
            continue;
        }

        let content = visual_col_content(line_text, abs_pos, tab_size);

        match content {
            VisualColContent::Space(char_idx) => {
                let vp_pos = char_idx.saturating_sub(hscroll);
                if vp_pos < chars.len() {
                    chars[vp_pos] = '|';
                    if vp_pos < highlights.len() {
                        highlights[vp_pos] = Some(style);
                    }
                }
            }
            VisualColContent::PastEnd => {
                let vp_pos = abs_pos - hscroll;
                while chars.len() <= vp_pos {
                    chars.push(' ');
                    highlights.push(None);
                    selected_mask.push(false);
                    search_mask.push(false);
                }
                chars[vp_pos] = '|';
                highlights[vp_pos] = Some(style);
            }
            VisualColContent::Tab | VisualColContent::NonSpace => {
                // Hide guide: do nothing, don't disrupt tabs or non-space characters
            }
        }
    }
}
