//! Buffer tab bar rendering.

use ratatui::{
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::Paragraph,
    Frame,
};

use crate::ed::editor::Editor;

/// Render the multi-buffer tab bar.
pub fn draw_tab_bar(f: &mut Frame, area: Rect, editor: &Editor) {
    let tabs = editor.buffer_tabs();
    if tabs.len() <= 1 {
        let blank = Paragraph::new(Line::from(""));
        f.render_widget(blank, area);
        return;
    }

    let mut spans: Vec<Span> = Vec::new();

    for (name, modified, is_active) in &tabs {
        let indicator = if *modified { " [+]" } else { "" };
        let label = format!(" {}{} ", name, indicator);

        let style = if *is_active {
            Style::default()
                .fg(Color::Black)
                .bg(Color::Cyan)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(Color::Gray).bg(Color::DarkGray)
        };

        spans.push(Span::styled(label, style));
    }

    let line = Line::from(spans);
    let widget = Paragraph::new(line);
    f.render_widget(widget, area);
}
