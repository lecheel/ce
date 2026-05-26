//! Command line / status message rendering.

use ratatui::{
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::Paragraph,
    Frame,
};

use crate::ed::editor::Editor;
use crate::ed::mode::{MessageKind, Mode};

/// Render the command line or status message.
pub fn draw_command_line(f: &mut Frame, area: Rect, editor: &Editor) {
    let text = match editor.mode() {
        Mode::Command => {
            let cmd = editor.command();
            if cmd.is_empty() {
                Line::from(vec![Span::styled(
                    ":",
                    Style::default()
                        .fg(Color::Yellow)
                        .add_modifier(Modifier::BOLD),
                )])
            } else {
                Line::from(vec![
                    Span::styled(
                        ":",
                        Style::default()
                            .fg(Color::Yellow)
                            .add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(cmd.to_string(), Style::default().fg(Color::White)),
                ])
            }
        }
        Mode::Search => {
            let cmd = editor.command();
            if cmd.is_empty() {
                Line::from(vec![Span::styled(
                    "/",
                    Style::default()
                        .fg(Color::Yellow)
                        .add_modifier(Modifier::BOLD),
                )])
            } else {
                Line::from(vec![
                    Span::styled(
                        "/",
                        Style::default()
                            .fg(Color::Yellow)
                            .add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(cmd.to_string(), Style::default().fg(Color::White)),
                ])
            }
        }
        _ => {
            // If active suggestions are present, render them here on the very bottom line
            if !editor.completions().is_empty() {
                let completions = editor.completions();
                let total = completions.len();
                let current_idx = editor.completion_idx();

                // Safe truncation (handles wide CJK characters without panic)
                let trim_word = |word: &str| -> String {
                    let chars: Vec<char> = word.chars().collect();
                    if chars.len() > 15 {
                        let mut s: String = chars.iter().take(12).collect();
                        s.push_str("...");
                        s
                    } else {
                        word.to_string()
                    }
                };

                // Sliding window calculations to keep the current candidate centered/visible
                let window_size = 5;
                let start_idx = if total <= window_size {
                    0
                } else {
                    let half = window_size / 2;
                    if current_idx < half {
                        0
                    } else if current_idx + half >= total {
                        total - window_size
                    } else {
                        current_idx - half
                    }
                };
                let end_idx = (start_idx + window_size).min(total);

                let mut spans = vec![Span::styled(
                    format!("[{}/{}] ", current_idx.saturating_add(1), total),
                    Style::default()
                        .fg(Color::Yellow)
                        .add_modifier(Modifier::BOLD),
                )];

                // Render ellipsis prefix if we sliced past the start
                if start_idx > 0 {
                    spans.push(Span::styled("... ", Style::default().fg(Color::DarkGray)));
                }

                for i in start_idx..end_idx {
                    let is_current = i == current_idx;
                    let word = trim_word(&completions[i]);

                    if is_current {
                        // Cyan highlight with black text for the selected candidate
                        spans.push(Span::styled(
                            word,
                            Style::default()
                                .fg(Color::Black)
                                .bg(Color::Cyan)
                                .add_modifier(Modifier::BOLD),
                        ));
                    } else {
                        spans.push(Span::styled(word, Style::default().fg(Color::Gray)));
                    }

                    if i + 1 < end_idx {
                        spans.push(Span::styled(", ", Style::default().fg(Color::DarkGray)));
                    }
                }

                // Render ellipsis suffix if there are remaining candidates
                if end_idx < total {
                    spans.push(Span::styled(" ...", Style::default().fg(Color::DarkGray)));
                }

                Line::from(spans)
            } else {
                // Otherwise, display standard welcome, info, success, or error status messages
                let msg = editor.status_msg();
                let kind = editor.status_kind();
                let style = match kind {
                    MessageKind::Info => Style::default().fg(Color::Gray),
                    MessageKind::Error => {
                        Style::default().fg(Color::Red).add_modifier(Modifier::BOLD)
                    }
                    MessageKind::Success => Style::default()
                        .fg(Color::Green)
                        .add_modifier(Modifier::BOLD),
                };
                Line::from(vec![Span::styled(msg.to_string(), style)])
            }
        }
    };

    let widget = Paragraph::new(text);
    f.render_widget(widget, area);

    if editor.mode() == Mode::Command || editor.mode() == Mode::Search {
        let cursor_char_pos = editor.command_cursor; // chars before cursor
        let cx = area
            .x
            .saturating_add(1) // +1 for the ":" or "/" prefix
            .saturating_add(cursor_char_pos.min(u16::MAX as usize) as u16);
        let cx = cx.min(area.right().saturating_sub(1));
        let cy = area.y;
        f.set_cursor_position((cx, cy));
    }
}
