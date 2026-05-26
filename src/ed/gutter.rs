//! Gutter rendering logic (line numbers, git signs, bookmarks).
//!
//! Uses Vim-style sign alignment where Git and Bookmark signs overlay
//! the leading spaces of right-aligned line numbers to save space.

use crate::config::app_config::Config;
use crate::ed::buffer::{Buffer, GitSign};
use crate::ed::window::Window;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::Span;

/// Calculates the total display width of the gutter column.
pub fn gutter_width(buf: &Buffer, _win: &Window, config: &Config) -> usize {
    let mut width = 0;

    let num_lines = buf.len_lines();
    let line_num_width = if num_lines > 0 {
        num_lines.to_string().len() // Safe integer length
    } else {
        1
    };

    if config.line_numbers_enabled {
        width += line_num_width;
        // If signs are enabled, they borrow the leading space of relative numbers.
        // For the current line (which has no leading space), we add +1 extra
        // column so the sign doesn't overwrite the digits.
        if config.git_gutter_enabled || config.bookmarks_enabled {
            width += 1;
        }
    } else {
        // No line numbers, signs need their own dedicated columns
        if config.git_gutter_enabled {
            width += 1;
        }
        if config.bookmarks_enabled {
            width += 1;
        }
    }

    // Trailing space separator between gutter and text
    width += 1;

    // Ensure minimum width of 1 if everything is disabled
    width.max(1)
}

/// Renders the gutter content for a single line.
pub fn render_gutter_line(
    buf: &Buffer,
    win: &Window,
    row: usize,
    config: &Config,
) -> Vec<Span<'static>> {
    let mut spans = Vec::new();
    let is_cursor_line = win.row == row;

    let dim_fg = Color::Rgb(90, 90, 100);
    let bright_fg = Color::Rgb(200, 200, 200);

    let num_lines = buf.len_lines();
    let line_num_width = if num_lines > 0 {
        num_lines.to_string().len()
    } else {
        1
    };

    // 1. Determine the sign character (Git takes priority over Bookmarks)
    let mut sign_char = " ".to_string();
    let mut sign_style = Style::default();

    if config.git_gutter_enabled {
        if let Some(sign) = buf.git_diffs.get(&row) {
            match sign {
                GitSign::Added => {
                    sign_char = "┃".to_string();
                    sign_style = Style::default().fg(Color::Green);
                }
                GitSign::Modified => {
                    sign_char = "┃".to_string();
                    sign_style = Style::default().fg(Color::Yellow);
                }
                GitSign::Removed => {
                    sign_char = "-".to_string();
                    sign_style = Style::default().fg(Color::Red);
                }
            }
        }
    }

    if sign_char == " " && config.bookmarks_enabled {
        let letter = buf
            .named_bookmarks
            .iter()
            .find(|&(_, &(r, _))| r == row)
            .map(|(&c, _)| c);

        if let Some(ch) = letter {
            sign_char = ch.to_string();
            sign_style = Style::default().fg(Color::Magenta);
        } else if buf.bookmarks.contains(&row) {
            sign_char = "►".to_string();
            sign_style = Style::default().fg(Color::Magenta);
        }
    }

    let has_signs = config.git_gutter_enabled || config.bookmarks_enabled;

    // Calculate the format width for right-alignment
    let format_width = if config.line_numbers_enabled && has_signs {
        line_num_width + 1 // +1 so the current line's number isn't destroyed by the sign
    } else {
        line_num_width
    };

    // 2. Render Line Numbers
    if config.line_numbers_enabled {
        let line_num_str = if config.relative_line_numbers {
            if is_cursor_line {
                // Show absolute line number on the cursor line
                format!("{:>width$}", row + 1, width = format_width)
            } else {
                // Show distance from cursor for other lines
                let dist = (win.row as isize - row as isize).abs() as usize;
                format!("{:>width$}", dist, width = format_width)
            }
        } else {
            // Standard absolute line numbers
            format!("{:>width$}", row + 1, width = format_width)
        };

        let style = if is_cursor_line {
            Style::default().fg(bright_fg).add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(dim_fg)
        };

        // Overlay the sign into the leading space of the right-aligned number
        if has_signs {
            let chars: Vec<char> = line_num_str.chars().collect();
            if !chars.is_empty() {
                // Render the sign (replaces the first leading space or dedicated column)
                spans.push(Span::styled(sign_char.to_string(), sign_style));
                // Render the rest of the line number
                if chars.len() > 1 {
                    spans.push(Span::styled(chars[1..].iter().collect::<String>(), style));
                }
            }
        } else {
            // No signs enabled, just render the plain line number
            spans.push(Span::styled(line_num_str, style));
        }

        spans.push(Span::raw(" ")); // Trailing separator between gutter and text
    } else {
        // No line numbers, just render the signs directly
        if config.git_gutter_enabled {
            spans.push(Span::styled(sign_char.to_string(), sign_style));
        }
        if config.bookmarks_enabled && sign_char == "►" {
            spans.push(Span::styled(
                "►".to_string(),
                Style::default().fg(Color::Magenta),
            ));
        }
        spans.push(Span::raw(" ")); // Trailing separator
    }

    spans
}
