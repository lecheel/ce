// ── Rounded Box Helpers ────────────────────────────────────────────────

use crossterm::cursor::MoveTo;
use crossterm::execute;
use crossterm::style::{Color, Print, ResetColor, SetBackgroundColor, SetForegroundColor};
use ratatui::symbols::border;
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

/// Catppuccin Mocha color palette.
#[rustfmt::skip]
pub mod catppuccin {
    use crossterm::style::Color;

    pub const MANTLE: Color = Color::Rgb { r: 24, g: 24, b: 37, };
    pub const CRUST: Color = Color::Rgb { r: 17, g: 17, b: 27, };
    pub const SURFACE0: Color = Color::Rgb { r: 49, g: 50, b: 68, };
    pub const SURFACE1: Color = Color::Rgb { r: 69, g: 71, b: 90, };
    pub const SURFACE2: Color = Color::Rgb { r: 88, g: 91, b: 112, };
    pub const OVERLAY0: Color = Color::Rgb { r: 166, g: 173, b: 200, };
    pub const OVERLAY1: Color = Color::Rgb { r: 186, g: 194, b: 222, };
    pub const TEXT: Color = Color::Rgb { r: 205, g: 214, b: 244, };
    pub const SUBTEXT: Color = Color::Rgb { r: 166, g: 173, b: 200, };
    pub const BLUE: Color = Color::Rgb { r: 137, g: 180, b: 250, };
    pub const LAVENDER: Color = Color::Rgb { r: 180, g: 190, b: 254, };
    pub const MAUVE: Color = Color::Rgb { r: 203, g: 166, b: 247, };
    pub const GREEN: Color = Color::Rgb { r: 166, g: 227, b: 161, };
    pub const RED: Color = Color::Rgb { r: 243, g: 139, b: 168, };
    pub const YELLOW: Color = Color::Rgb { r: 249, g: 226, b: 175, };
    pub const PEACH: Color = Color::Rgb { r: 250, g: 179, b: 135, };
    pub const TEAL: Color = Color::Rgb { r: 148, g: 226, b: 213, };
    pub const FLAMINGO: Color = Color::Rgb { r: 242, g: 205, b: 205, };
    pub const PINK: Color = Color::Rgb { r: 245, g: 194, b: 231, };
    pub const MAROON: Color = Color::Rgb { r: 235, g: 160, b: 172, };
    pub const SKY: Color = Color::Rgb { r: 137, g: 220, b: 235, };
    pub const SAPPHIRE: Color = Color::Rgb { r: 116, g: 199, b: 236, };
}

// ── Style types ────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct BoxStyle {
    pub border: Color,
    pub bg: Color,
    pub title: Option<String>,
    pub title_color: Color,
    pub footer: Option<String>,
    pub footer_color: Color,
}

impl Default for BoxStyle {
    fn default() -> Self {
        Self {
            border: catppuccin::SURFACE2,
            bg: catppuccin::MANTLE,
            title: None,
            title_color: catppuccin::MAUVE,
            footer: None,
            footer_color: catppuccin::OVERLAY0,
        }
    }
}

impl BoxStyle {
    pub fn with_title(mut self, title: impl Into<String>) -> Self {
        self.title = Some(title.into());
        self
    }

    pub fn with_footer(mut self, footer: impl Into<String>) -> Self {
        self.footer = Some(footer.into());
        self
    }

    pub fn with_border(mut self, color: Color) -> Self {
        self.border = color;
        self
    }

    pub fn with_bg(mut self, color: Color) -> Self {
        self.bg = color;
        self
    }
}

#[derive(Debug, Clone)]
pub struct RowStyle {
    pub border: Color,
    pub bg: Color,
    pub text: Color,
    pub pad_left: usize,
    pub pad_right: usize,
}

impl Default for RowStyle {
    fn default() -> Self {
        Self {
            border: catppuccin::SURFACE2,
            bg: catppuccin::MANTLE,
            text: catppuccin::TEXT,
            pad_left: 1,
            pad_right: 1,
        }
    }
}

impl RowStyle {
    pub fn selected() -> Self {
        Self {
            border: catppuccin::SURFACE2,
            bg: catppuccin::SURFACE0,
            text: catppuccin::TEXT,
            pad_left: 1,
            pad_right: 1,
        }
    }

    pub fn normal() -> Self {
        Self::default()
    }

    pub fn with_bg(mut self, color: Color) -> Self {
        self.bg = color;
        self
    }

    pub fn with_text(mut self, color: Color) -> Self {
        self.text = color;
        self
    }

    pub fn with_border(mut self, color: Color) -> Self {
        self.border = color;
        self
    }

    pub fn no_padding(mut self) -> Self {
        self.pad_left = 0;
        self.pad_right = 0;
        self
    }

    pub fn with_padding(mut self, left: usize, right: usize) -> Self {
        self.pad_left = left;
        self.pad_right = right;
        self
    }
}

#[derive(Debug, Clone)]
pub struct Segment<'a> {
    pub text: &'a str,
    pub color: Color,
}

impl<'a> Segment<'a> {
    pub fn new(text: &'a str, color: Color) -> Self {
        Self { text, color }
    }
}

// ── Core drawing functions ─────────────────────────────────────────────

#[inline]
pub fn inner_width(width: u16) -> usize {
    width.saturating_sub(2) as usize
}

#[inline]
pub fn content_width(width: u16, style: &RowStyle) -> usize {
    inner_width(width).saturating_sub(style.pad_left + style.pad_right)
}

#[inline]
pub fn str_width(s: &str) -> usize {
    UnicodeWidthStr::width(s)
}

pub fn truncate_to_width(s: &str, max_width: usize) -> &str {
    if max_width == 0 {
        return "";
    }
    let mut width = 0;
    let mut end = s.len();
    for (i, ch) in s.char_indices() {
        let ch_w = UnicodeWidthChar::width(ch).unwrap_or(0);
        if width + ch_w > max_width {
            end = i;
            break;
        }
        width += ch_w;
    }
    &s[..end]
}

pub fn draw_border(
    stdout: &mut std::io::Stdout,
    x: u16,
    y: u16,
    width: u16,
    height: u16,
    style: &BoxStyle,
) -> Result<(), Box<dyn std::error::Error>> {
    draw_top_border(stdout, x, y, width, style)?;

    let iw = inner_width(width);
    for row in 1..height.saturating_sub(1) {
        draw_side_row(stdout, x, y + row, iw, style)?;
    }

    draw_bottom_border(stdout, x, y + height.saturating_sub(1), width, style)?;
    Ok(())
}

pub fn draw_top_border(
    stdout: &mut std::io::Stdout,
    x: u16,
    y: u16,
    width: u16,
    style: &BoxStyle,
) -> Result<(), Box<dyn std::error::Error>> {
    let iw = inner_width(width);

    execute!(stdout, MoveTo(x, y))?;
    execute!(stdout, SetForegroundColor(style.border))?;
    execute!(stdout, SetBackgroundColor(style.bg))?;
    // execute!(stdout, Print("┌"))?;
    execute!(stdout, Print(border::ROUNDED.top_left))?;

    if let Some(ref title) = style.title {
        execute!(stdout, Print(" "))?;

        let max_title_w = iw.saturating_sub(4);
        let display = truncate_to_width(title, max_title_w);
        let display_w = str_width(display);

        execute!(stdout, SetForegroundColor(style.title_color))?;
        // bg is still set from above — no need to re-set
        execute!(stdout, Print(display))?;

        execute!(stdout, SetForegroundColor(style.border))?;
        execute!(stdout, Print(" "))?;

        let used = 2 + display_w;
        let remaining = iw.saturating_sub(used);
        if remaining > 0 {
            execute!(stdout, Print(&"─".repeat(remaining)))?;
        }
    } else if iw > 0 {
        execute!(stdout, Print(&"─".repeat(iw)))?;
    }

    // execute!(stdout, Print("┐"))?;
    execute!(stdout, Print(border::ROUNDED.top_right))?;
    Ok(())
}
pub fn draw_bottom_border(
    stdout: &mut std::io::Stdout,
    x: u16,
    y: u16,
    width: u16,
    style: &BoxStyle,
) -> Result<(), Box<dyn std::error::Error>> {
    let iw = inner_width(width);

    execute!(stdout, MoveTo(x, y))?;
    execute!(stdout, SetForegroundColor(style.border))?;
    execute!(stdout, SetBackgroundColor(style.bg))?;
    // execute!(stdout, Print("└"))?;
    execute!(stdout, Print(border::ROUNDED.bottom_left))?;

    if let Some(ref footer) = style.footer {
        execute!(stdout, Print(" "))?;

        let max_footer_w = iw.saturating_sub(4);
        let display = truncate_to_width(footer, max_footer_w);
        let display_w = str_width(display);

        execute!(stdout, SetForegroundColor(style.footer_color))?;
        execute!(stdout, Print(display))?;

        execute!(stdout, SetForegroundColor(style.border))?;
        execute!(stdout, Print(" "))?;

        let used = 2 + display_w;
        let remaining = iw.saturating_sub(used);
        if remaining > 0 {
            execute!(stdout, Print(&"─".repeat(remaining)))?;
        }
    } else if iw > 0 {
        execute!(stdout, Print(&"─".repeat(iw)))?;
    }

    // execute!(stdout, Print("┘"))?;
    execute!(stdout, Print(border::ROUNDED.bottom_right))?;
    Ok(())
}

fn draw_side_row(
    stdout: &mut std::io::Stdout,
    x: u16,
    y: u16,
    inner_w: usize,
    style: &BoxStyle,
) -> Result<(), Box<dyn std::error::Error>> {
    execute!(stdout, MoveTo(x, y))?;
    execute!(stdout, SetForegroundColor(style.border))?;
    execute!(stdout, SetBackgroundColor(style.bg))?;
    execute!(stdout, Print("│"))?;
    if inner_w > 0 {
        execute!(stdout, SetBackgroundColor(style.bg))?; // already set, but explicit
        execute!(stdout, Print(&" ".repeat(inner_w)))?;
    }
    execute!(stdout, SetForegroundColor(style.border))?;
    // bg is still active — no ResetColor between last inner-space and this │
    execute!(stdout, Print("│"))?;
    Ok(())
}

// ── Row drawing functions ──────────────────────────────────────────────

pub fn draw_row(
    stdout: &mut std::io::Stdout,
    x: u16,
    y: u16,
    width: u16,
    segments: &[Segment<'_>],
    style: &RowStyle,
) -> Result<(), Box<dyn std::error::Error>> {
    let iw = inner_width(width);
    let cw = iw.saturating_sub(style.pad_left + style.pad_right);

    execute!(stdout, MoveTo(x, y))?;
    execute!(stdout, ResetColor)?;
    execute!(stdout, SetForegroundColor(style.border))?;
    // FIX: border │ always on MANTLE, never inherits row highlight bg
    execute!(stdout, SetBackgroundColor(catppuccin::MANTLE))?;
    execute!(stdout, Print("│"))?;

    if style.pad_left > 0 {
        execute!(stdout, SetBackgroundColor(style.bg))?;
        execute!(stdout, Print(&" ".repeat(style.pad_left)))?;
    }

    let mut used_w = 0;
    for seg in segments {
        if used_w >= cw {
            break;
        }
        let available = cw.saturating_sub(used_w);
        let display = truncate_to_width(seg.text, available);
        let display_w = str_width(display);

        execute!(stdout, SetBackgroundColor(style.bg))?;
        execute!(stdout, SetForegroundColor(seg.color))?;
        execute!(stdout, Print(display))?;
        used_w += display_w;
    }

    let remaining = cw.saturating_sub(used_w);
    if remaining > 0 {
        execute!(stdout, SetBackgroundColor(style.bg))?;
        execute!(stdout, Print(&" ".repeat(remaining)))?;
    }
    if style.pad_right > 0 {
        execute!(stdout, SetBackgroundColor(style.bg))?;
        execute!(stdout, Print(&" ".repeat(style.pad_right)))?;
    }

    // FIX: border │ always on MANTLE, never inherits row highlight bg
    execute!(stdout, ResetColor)?;
    execute!(stdout, SetForegroundColor(style.border))?;
    execute!(stdout, SetBackgroundColor(catppuccin::MANTLE))?;
    execute!(stdout, Print("│"))?;

    Ok(())
}

pub fn draw_row_text(
    stdout: &mut std::io::Stdout,
    x: u16,
    y: u16,
    width: u16,
    text: &str,
    style: &RowStyle,
) -> Result<(), Box<dyn std::error::Error>> {
    draw_row(
        stdout,
        x,
        y,
        width,
        &[Segment::new(text, style.text)],
        style,
    )
}

pub fn draw_empty_row(
    stdout: &mut std::io::Stdout,
    x: u16,
    y: u16,
    width: u16,
    style: &RowStyle,
) -> Result<(), Box<dyn std::error::Error>> {
    draw_row(stdout, x, y, width, &[], style)
}

pub fn draw_row_with_trailing(
    stdout: &mut std::io::Stdout,
    x: u16,
    y: u16,
    width: u16,
    segments: &[Segment<'_>],
    trailing: &Segment<'_>,
    style: &RowStyle,
) -> Result<(), Box<dyn std::error::Error>> {
    let iw = inner_width(width);
    let cw = iw.saturating_sub(style.pad_left + style.pad_right);

    let trailing_display = truncate_to_width(trailing.text, cw);
    let trailing_w = str_width(trailing_display);

    let main_w = cw.saturating_sub(trailing_w);

    execute!(stdout, MoveTo(x, y))?;
    execute!(stdout, SetForegroundColor(style.border))?;
    execute!(stdout, SetBackgroundColor(style.bg))?;
    execute!(stdout, Print("│"))?;

    if style.pad_left > 0 {
        execute!(stdout, SetBackgroundColor(style.bg))?;
        execute!(stdout, Print(&" ".repeat(style.pad_left)))?;
    }

    let mut used_w = 0;
    for seg in segments {
        if used_w >= main_w {
            break;
        }
        let available = main_w.saturating_sub(used_w);
        let display = truncate_to_width(seg.text, available);
        let display_w = str_width(display);

        execute!(stdout, SetBackgroundColor(style.bg))?;
        execute!(stdout, SetForegroundColor(seg.color))?;
        execute!(stdout, Print(display))?;
        used_w += display_w;
    }

    let gap = main_w.saturating_sub(used_w);
    if gap > 0 {
        execute!(stdout, SetBackgroundColor(style.bg))?;
        execute!(stdout, Print(&" ".repeat(gap)))?;
    }

    execute!(stdout, SetBackgroundColor(style.bg))?;
    execute!(stdout, SetForegroundColor(trailing.color))?;
    execute!(stdout, Print(trailing_display))?;

    if style.pad_right > 0 {
        execute!(stdout, SetBackgroundColor(style.bg))?;
        execute!(stdout, Print(&" ".repeat(style.pad_right)))?;
    }

    execute!(stdout, SetForegroundColor(style.border))?;
    execute!(stdout, Print("│"))?;

    Ok(())
}

// ── Layout helpers ─────────────────────────────────────────────────────

pub fn centered_pos(popup_w: u16, popup_h: u16, term_w: u16, term_h: u16) -> (u16, u16) {
    let x = term_w.saturating_sub(popup_w) / 2;
    let y = term_h.saturating_sub(popup_h) / 2;
    (x, y)
}

pub fn centered_in_edit(
    popup_w: u16,
    popup_h: u16,
    term_w: u16,
    term_h: u16,
    status_h: u16,
) -> (u16, u16) {
    let edit_h = term_h.saturating_sub(status_h);
    let x = term_w.saturating_sub(popup_w) / 2;
    let y = edit_h.saturating_sub(popup_h) / 2;
    (x, y)
}

pub fn clamp_width(desired: u16, term_w: u16, margin: u16) -> u16 {
    desired.min(term_w.saturating_sub(margin)).max(20)
}

pub fn clamp_height(desired: u16, available: u16, min: u16) -> u16 {
    desired.min(available).max(min)
}

/// Paint a solid rectangle of spaces with the given background color.
/// Call this *before* drawing a popup so no stale editor pixels show through.
pub fn clear_rect(
    stdout: &mut std::io::Stdout,
    x: u16,
    y: u16,
    width: u16,
    height: u16,
    bg: Color,
) -> Result<(), Box<dyn std::error::Error>> {
    if width == 0 || height == 0 {
        return Ok(());
    }
    let blank = " ".repeat(width as usize);
    for row in 0..height {
        execute!(
            stdout,
            MoveTo(x, y + row),
            SetBackgroundColor(bg),
            Print(&blank)
        )?;
    }
    Ok(())
}
