use crate::ed::editor::Editor;
use crate::ed::mode::{MessageKind, Mode};
use ratatui::{
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::Paragraph,
    Frame,
};

pub fn draw_command_line(f: &mut Frame, area: Rect, editor: &Editor) {
    let mut is_llm_prompt = false;

    let text = if editor.llm.prompt_active {
        // ── Single-round LLM prompt with '>' prefix ──────────────
        is_llm_prompt = true;
        let prompt = &editor.llm.prompt;
        let before_cursor: String = prompt.buffer.chars().take(prompt.cursor).collect();
        let after_cursor: String = prompt.buffer.chars().skip(prompt.cursor).collect();
        let mut spans = vec![Span::styled(
            "> ",
            Style::default()
                .fg(Color::Green)
                .add_modifier(Modifier::BOLD),
        )];
        spans.push(Span::styled(
            before_cursor,
            Style::default().fg(Color::White),
        ));
        spans.push(Span::styled(
            after_cursor,
            Style::default().fg(Color::White),
        ));
        Line::from(spans)
    } else {
        match editor.mode() {
            Mode::Command | Mode::Search => {
                let cmd = editor.command();
                let before_cursor: String = cmd.chars().take(editor.command_cursor).collect();
                let after_cursor: String = cmd.chars().skip(editor.command_cursor).collect();
                let prefix_str = match editor.mode() {
                    Mode::Search => "/",
                    _ => ":",
                };
                let mut spans = vec![Span::styled(
                    prefix_str,
                    Style::default()
                        .fg(Color::Yellow)
                        .add_modifier(Modifier::BOLD),
                )];
                spans.push(Span::styled(
                    before_cursor,
                    Style::default().fg(Color::White),
                ));
                if editor.pending_register {
                    spans.push(Span::styled(
                        "^",
                        Style::default()
                            .fg(Color::Yellow)
                            .add_modifier(Modifier::BOLD),
                    ));
                }
                spans.push(Span::styled(
                    after_cursor,
                    Style::default().fg(Color::White),
                ));
                Line::from(spans)
            }
            _ => {
                if !editor.completions().is_empty() {
                    let completions = editor.completions();
                    let total = completions.len();
                    let current_idx = editor.completion_idx();
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
                    if start_idx > 0 {
                        spans.push(Span::styled("... ", Style::default().fg(Color::DarkGray)));
                    }
                    for i in start_idx..end_idx {
                        let is_current = i == current_idx;
                        let word = trim_word(&completions[i]);
                        if is_current {
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
                    if end_idx < total {
                        spans.push(Span::styled(" ...", Style::default().fg(Color::DarkGray)));
                    }
                    Line::from(spans)
                } else {
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
        }
    };

    let widget = Paragraph::new(text);
    f.render_widget(widget, area);

    if is_llm_prompt {
        // ── Position cursor inside the '>' prompt ────────────────
        let cursor_char_pos = editor.llm.prompt.cursor;
        let cx = area
            .x
            .saturating_add(2) // "> " prefix is 2 columns
            .saturating_add(cursor_char_pos.min(u16::MAX as usize) as u16);
        let cx = cx.min(area.right().saturating_sub(1));
        let cy = area.y;
        f.set_cursor_position((cx, cy));
    } else if editor.mode() == Mode::Command || editor.mode() == Mode::Search {
        let cursor_char_pos = editor.command_cursor;
        let cx = area
            .x
            .saturating_add(1)
            .saturating_add(cursor_char_pos.min(u16::MAX as usize) as u16);
        let cx = cx.min(area.right().saturating_sub(1));
        let cy = area.y;
        f.set_cursor_position((cx, cy));
    }
}
