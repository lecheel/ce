//--+ render/popup.rs
use crate::ed::editor::Editor;
use crate::popup::fuzzy;
use crate::popup::{PopupContent, PopupItem};
use ratatui::{
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, List, ListItem, Paragraph},
    Frame,
};

// ── layout helpers ─────────────────────────────────────────────────

fn clamp(screen: Rect, area: Rect) -> Rect {
    let x = area.x.min(screen.width.saturating_sub(1));
    let y = area.y.min(screen.height.saturating_sub(1));
    Rect {
        x,
        y,
        width: area.width.min(screen.width.saturating_sub(x)),
        height: area.height.min(screen.height.saturating_sub(y)),
    }
}

fn centered_rect(screen: Rect, width: u16, height: u16) -> Rect {
    clamp(
        screen,
        Rect {
            x: screen.width.saturating_sub(width) / 2,
            y: screen.height.saturating_sub(height) / 2,
            width,
            height,
        },
    )
}

fn compact_mods(s: &str) -> String {
    s.replace("shift+", "s+")
        .replace("ctrl+", "c+")
        .replace("alt+", "a+")
}

/// Unified popup width: 90 % of the screen, guaranteed to fit.
fn popup_width(screen: Rect) -> u16 {
    (screen.width as u32 * 90 / 100) as u16
}

// ── Tier 1: explicit popups ────────────────────────────────────────

/// Draw the explicitly-opened popup (Config, Scankey, FilePicker, FunctionList, MRU), if any.
/// Called once from `render::draw`.
pub fn draw_popup(f: &mut Frame, editor: &Editor) {
    let screen = f.area();

    // ── Error popup: always on top, dismiss with Esc ──────────────
    if editor.popup.error.is_some() {
        draw_error(f, editor, screen);
        return;
    }

    // 0. Intercept BufferList
    if editor.popup.buffer_list.is_some() {
        draw_buffer_list(f, editor, screen);
        return;
    }

    // 1. Intercept FilePicker
    if editor.popup.file_picker.is_some() {
        draw_file_picker(f, editor, screen);
        return;
    }

    // 2. Intercept Function Navigation List
    if editor.popup.function_list.is_some() {
        draw_function_list(f, editor, screen);
        return;
    }

    // 3. Intercept MRU List (Recent Files)
    if editor.popup.mru.is_some() {
        draw_mru(f, editor, screen);
        return;
    }

    // 4. Intercept GitHunkPopup
    if editor.popup.git_hunk.is_some() {
        draw_git_hunk_popup(f, editor, screen);
        return;
    }

    // 5. Intercept Marks Popup
    if editor.popup.marks.is_some() {
        draw_marks(f, editor, screen);
        return;
    }

    // 6. Intercept Command Palette
    if editor.popup.command_palette.is_some() {
        draw_command_palette(f, editor, screen);
        return;
    }

    // 7. Intercept Guide Popup
    if editor.popup.guide.is_some() {
        draw_guide(f, editor, screen);
        return;
    }

    // 8. Intercept tag Popup
    if editor.popup.tag_candidates.is_some() {
        draw_tag_candidates(f, editor, screen);
        return;
    }

    let Some(content) = &editor.popup.content else {
        return;
    };

    match content {
        PopupContent::Config { items, selected } => draw_config(f, items, *selected, screen),
        PopupContent::Scankey {
            key_label,
            action_label,
            raw_label,
        } => draw_scankey(f, key_label, action_label, raw_label, screen),
    }
}

fn draw_guide(f: &mut Frame, editor: &Editor, screen: Rect) {
    let popup = match &editor.popup.guide {
        Some(p) => p,
        None => return,
    };
    let list = &popup.list;

    let width = popup_width(screen);
    const VISIBLE_ROWS: usize = 20;
    const TOTAL_HEIGHT: u16 = VISIBLE_ROWS as u16 + 3; // +3 borders+filter
    let area = centered_rect(screen, width, TOTAL_HEIGHT);

    let title = format!(" Guide ({}) ", list.filtered.len());
    let footer = format!(
        "[Esc] close  [Enter] jump  {}/{}",
        if list.filtered.is_empty() {
            0
        } else {
            list.selected + 1
        },
        list.filtered.len()
    );

    let outer_block = Block::default()
        .title(Span::styled(
            title,
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ))
        .title_bottom(Span::styled(footer, Style::default().fg(Color::DarkGray)))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::DarkGray))
        .style(Style::default().bg(Color::Black));

    let inner = outer_block.inner(area);
    let filter_area = Rect {
        x: inner.x,
        y: inner.y,
        width: inner.width,
        height: 1,
    };
    let list_area = Rect {
        x: inner.x,
        y: inner.y + 1,
        width: inner.width,
        height: inner.height - 1,
    };

    let filter_line = if list.filter.is_empty() {
        Paragraph::new(Line::from(vec![
            Span::styled("Filter: ", Style::default().fg(Color::Gray)),
            Span::styled(
                "█",
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::ITALIC),
            ),
        ]))
    } else {
        Paragraph::new(Line::from(vec![
            Span::styled("Filter: ", Style::default().fg(Color::Gray)),
            Span::styled(
                list.filter.clone(),
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled("█", Style::default().fg(Color::Yellow)),
        ]))
    };

    let mut list_items = Vec::with_capacity(VISIBLE_ROWS);
    let scroll_offset = if list.selected >= VISIBLE_ROWS {
        list.selected - VISIBLE_ROWS + 1
    } else {
        0
    };

    for i in 0..VISIBLE_ROWS {
        let entry_idx = scroll_offset + i;
        if entry_idx < list.filtered.len() {
            let real_idx = list.filtered[entry_idx];
            if let Some(entry) = list.entries.get(real_idx) {
                let is_selected = entry_idx == list.selected;

                let kind_display = format!("{:>12} ", entry.kind);

                let file_name = std::path::Path::new(&entry.file)
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or(&entry.file);

                let label_display = format!("{} ({})", entry.label, file_name);

                let kind_style = if is_selected {
                    Style::default()
                        .fg(Color::Black)
                        .bg(Color::DarkGray)
                        .add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(Color::Magenta)
                };

                let label_style = if is_selected {
                    Style::default()
                        .fg(Color::Yellow)
                        .bg(Color::DarkGray)
                        .add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(Color::Gray)
                };

                let spans = vec![
                    Span::styled(kind_display, kind_style),
                    Span::styled(label_display, label_style),
                ];

                list_items.push(ListItem::new(Line::from(spans)));
                continue;
            }
        }
        list_items.push(ListItem::new(Line::from("")));
    }

    let list_widget = List::new(list_items);
    f.render_widget(Clear, area);
    f.render_widget(outer_block, area);
    f.render_widget(filter_line, filter_area);
    f.render_widget(list_widget, list_area);
}

fn draw_config(f: &mut Frame, items: &[PopupItem], selected: usize, screen: Rect) {
    let height = items.len().saturating_add(4).min(u16::MAX as usize) as u16;
    let area = centered_rect(screen, 120, height);

    let block = Block::default()
        .title(Span::styled(
            " Editor config (Space to toggle, Esc to exit) ",
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::DarkGray))
        .style(Style::default().bg(Color::Black));

    let lines: Vec<Line> = items
        .iter()
        .enumerate()
        .map(|(idx, item)| {
            let is_sel = idx == selected;
            let style = if is_sel {
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(Color::Gray)
            };
            let mut spans = vec![
                Span::styled(if is_sel { " > " } else { "   " }, style),
                Span::styled(&item.label, style),
            ];
            if let Some(detail) = &item.detail {
                spans.push(Span::styled(
                    format!(" — {detail}"),
                    Style::default().fg(Color::DarkGray),
                ));
            }
            Line::from(spans)
        })
        .collect();

    f.render_widget(Clear, area);
    f.render_widget(Paragraph::new(lines).block(block), area);
}

fn draw_scankey(f: &mut Frame, key_label: &str, action_label: &str, raw_label: &str, screen: Rect) {
    let content_w = key_label
        .len()
        .max(action_label.len())
        .max(raw_label.len())
        .saturating_add(8);
    let width = (content_w.min(u16::MAX as usize) as u16)
        .max(120)
        .min(screen.width);
    let height = 7u16.min(screen.height);
    let area = centered_rect(screen, width, height);

    let action_style = if action_label == "NONE" {
        Style::default().fg(Color::Red).add_modifier(Modifier::BOLD)
    } else {
        Style::default()
            .fg(Color::Green)
            .add_modifier(Modifier::BOLD)
    };

    let lines = vec![
        Line::from(""),
        Line::from(vec![
            Span::styled("  Key:    ", Style::default().fg(Color::Gray)),
            Span::styled(
                key_label,
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            ),
        ]),
        Line::from(vec![
            Span::styled("  Action: ", Style::default().fg(Color::Gray)),
            Span::styled(action_label, action_style),
        ]),
        Line::from(vec![
            Span::styled("  Raw:    ", Style::default().fg(Color::Gray)),
            Span::styled(raw_label, Style::default().fg(Color::DarkGray)),
        ]),
    ];

    let block = Block::default()
        .title(Span::styled(
            " Scankey (q to quit) ",
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::DarkGray))
        .style(Style::default().bg(Color::Black));

    f.render_widget(Clear, area);
    f.render_widget(Paragraph::new(lines).block(block), area);
}

fn draw_buffer_list(f: &mut Frame, editor: &Editor, screen: Rect) {
    let popup = match &editor.popup.buffer_list {
        Some(p) => p,
        None => return,
    };
    let list = &popup.list;

    let width = popup_width(screen);
    let max_h = screen.height.saturating_sub(4) as usize;
    let list_height = list.visible_height.min(20).min(max_h);

    let total_height = (list_height as u16).saturating_add(3);
    let area = centered_rect(screen, width, total_height);

    let title = format!(
        " Active Buffers ({}/{}) ",
        list.filtered.len(),
        list.entries.len()
    );

    let footer = format!(
        " [Enter] open  [d]/[Del] close  [Esc] close  {}/{}",
        if list.filtered.is_empty() {
            0
        } else {
            list.selected + 1
        },
        list.filtered.len()
    );

    let outer_block = Block::default()
        .title(Span::styled(
            title,
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ))
        .title_bottom(Span::styled(footer, Style::default().fg(Color::DarkGray)))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::DarkGray))
        .style(Style::default().bg(Color::Black));

    let inner = outer_block.inner(area);
    let filter_area = Rect {
        x: inner.x,
        y: inner.y,
        width: inner.width,
        height: 1,
    };
    let list_area = Rect {
        x: inner.x,
        y: inner.y.saturating_add(1),
        width: inner.width,
        height: inner.height.saturating_sub(1),
    };

    let filter_line = if list.filter.is_empty() {
        Paragraph::new(Line::from(vec![
            Span::styled("Filter: ", Style::default().fg(Color::Gray)),
            Span::styled(
                "█",
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::ITALIC),
            ),
        ]))
    } else {
        Paragraph::new(Line::from(vec![
            Span::styled("Filter: ", Style::default().fg(Color::Gray)),
            Span::styled(
                list.filter.clone(),
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled("█", Style::default().fg(Color::Yellow)),
        ]))
    };

    let mut list_items = Vec::with_capacity(list_height);
    for i in 0..list_height {
        let entry_idx = list.scroll + i;
        if entry_idx < list.filtered.len() {
            let real_idx = list.filtered[entry_idx];
            let entry = &list.entries[real_idx];
            let is_selected = entry_idx == list.selected;

            let row_style = if is_selected {
                Style::default().bg(Color::DarkGray).fg(Color::White)
            } else {
                Style::default()
            };

            let idx_str = format!("{:>2}", entry_idx + 1);
            let idx_style = if is_selected {
                Style::default().fg(Color::White)
            } else {
                Style::default().fg(Color::DarkGray)
            };

            let name_color = if is_selected {
                Color::White
            } else if entry.is_modified {
                Color::Yellow
            } else {
                Color::Blue
            };

            let mut name_spans = vec![];
            let match_idx = list
                .match_indices
                .get(entry_idx)
                .cloned()
                .unwrap_or_default();

            for (ci, ch) in entry.name.chars().enumerate() {
                let matched = match_idx.binary_search(&ci).is_ok();
                let style = if is_selected {
                    if matched {
                        Style::default()
                            .fg(Color::Yellow)
                            .add_modifier(Modifier::BOLD)
                    } else {
                        Style::default().fg(Color::White)
                    }
                } else if matched {
                    Style::default()
                        .fg(Color::Yellow)
                        .add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(name_color)
                };
                name_spans.push(Span::styled(ch.to_string(), style));
            }

            let mod_span = if entry.is_modified {
                Span::styled(" * [modified]", Style::default().fg(Color::Red))
            } else {
                Span::raw("")
            };

            let info_str = format!("  ({} lines)", entry.line_count);
            let info_span = Span::styled(info_str, Style::default().fg(Color::DarkGray));

            let mut line_spans = vec![
                Span::styled(idx_str, idx_style),
                Span::styled(" ", Style::default().fg(Color::DarkGray)),
            ];
            line_spans.extend(name_spans);
            line_spans.push(mod_span);
            line_spans.push(info_span);

            list_items.push(ListItem::new(Line::from(line_spans).style(row_style)));
        } else {
            list_items.push(ListItem::new(Line::from("")));
        }
    }

    let list_widget = List::new(list_items);

    f.render_widget(Clear, area);
    f.render_widget(outer_block, area);
    f.render_widget(filter_line, filter_area);
    f.render_widget(list_widget, list_area);
}

fn draw_file_picker(f: &mut Frame, editor: &Editor, screen: Rect) {
    let picker = match &editor.popup.file_picker {
        Some(p) => p,
        None => return,
    };
    let list = &picker.list;

    let width = popup_width(screen);

    let max_h = screen.height.saturating_sub(4) as usize;
    let list_height = list.visible_height.min(20).min(max_h);

    // +2 for top/bottom borders, +1 for the filter line
    let total_height = (list_height as u16).saturating_add(3);
    let area = centered_rect(screen, width, total_height);

    // ── Build list items with fuzzy-match highlighting ──────────────
    let items: Vec<ListItem> = list
        .filtered
        .iter()
        .enumerate()
        .skip(list.scroll)
        .take(list_height)
        .map(|(vis_i, &entry_idx)| {
            let Some(entry) = list.entries.get(entry_idx) else {
                return ListItem::new(Line::from(""));
            };
            let is_sel = vis_i == list.selected;
            let match_idx = list.match_indices.get(vis_i).cloned().unwrap_or_default();

            let prefix = if is_sel { "   " } else { "   " };
            let prefix_style = if is_sel {
                Style::default()
                    .fg(Color::Black)
                    .bg(Color::Green)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(Color::DarkGray)
            };

            let name_chars: Vec<char> = entry.name.chars().collect();
            let mut spans = vec![Span::styled(prefix, prefix_style)];

            for (ci, ch) in name_chars.iter().enumerate() {
                let matched = match_idx.binary_search(&ci).is_ok();
                let style = if is_sel {
                    if matched {
                        Style::default()
                            .fg(Color::Black)
                            .bg(Color::Green)
                            .add_modifier(Modifier::BOLD)
                    } else {
                        Style::default().fg(Color::Black).bg(Color::Green)
                    }
                } else if entry.is_dir {
                    if matched {
                        Style::default()
                            .fg(Color::Cyan)
                            .add_modifier(Modifier::BOLD)
                    } else {
                        Style::default().fg(Color::Cyan)
                    }
                } else if matched {
                    Style::default()
                        .fg(Color::Green)
                        .add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(Color::White)
                };
                spans.push(Span::styled(ch.to_string(), style));
            }

            ListItem::new(Line::from(spans))
        })
        .collect();

    // ── Title & Hints ───────────────────────────────────────────────
    let title = format!(" File Picker {} ", picker.cwd_display());

    let hints = Line::from(vec![
        Span::styled(" <Esc>", Style::default().fg(Color::Yellow)),
        Span::styled(" Close ", Style::default().fg(Color::DarkGray)),
        Span::styled("<Tab>", Style::default().fg(Color::Yellow)),
        Span::styled(" flat ", Style::default().fg(Color::DarkGray)),
        Span::styled("<BS>", Style::default().fg(Color::Yellow)),
        Span::styled(" Up/Filter ", Style::default().fg(Color::DarkGray)),
        Span::styled("<Home>", Style::default().fg(Color::Yellow)),
    ]);

    let outer_block = Block::default()
        .title(Span::styled(
            title,
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ))
        .title_bottom(hints)
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::DarkGray))
        .style(Style::default().bg(Color::Black));

    // Split the inner area: 1 row for filter, rest for list
    let inner = outer_block.inner(area);
    let filter_area = Rect {
        x: inner.x,
        y: inner.y,
        width: inner.width,
        height: 1,
    };
    let list_area = Rect {
        x: inner.x,
        y: inner.y.saturating_add(1),
        width: inner.width,
        height: inner.height.saturating_sub(1),
    };

    // ── Filter / Error Line ─────────────────────────────────────────
    let filter_line = if let Some(err) = &picker.last_error {
        Paragraph::new(Line::from(vec![
            Span::styled("⚠ ", Style::default().fg(Color::Red)),
            Span::styled(
                err.clone(),
                Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
            ),
        ]))
    } else if list.filter.is_empty() {
        Paragraph::new(Line::from(vec![
            Span::styled("> ", Style::default().fg(Color::Cyan)),
            Span::styled(
                "█",
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::ITALIC),
            ),
        ]))
    } else {
        Paragraph::new(Line::from(vec![
            Span::styled("> ", Style::default().fg(Color::Cyan)),
            Span::styled(
                list.filter.clone(),
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled("█", Style::default().fg(Color::Yellow)),
            Span::styled(
                format!(" ({} matches)", list.filtered.len()),
                Style::default().fg(Color::DarkGray),
            ),
        ]))
    };

    let list_widget = List::new(items);

    f.render_widget(Clear, area);
    f.render_widget(outer_block, area);
    f.render_widget(filter_line, filter_area);
    f.render_widget(list_widget, list_area);
}

fn draw_command_palette(f: &mut Frame, editor: &Editor, screen: Rect) {
    let popup = match &editor.popup.command_palette {
        Some(p) => p,
        None => return,
    };
    let list = &popup.list;

    let width = popup_width(screen);
    let max_h = screen.height.saturating_sub(4) as usize;
    let list_height = list.visible_height.min(20).min(max_h);
    let total_height = (list_height as u16).saturating_add(3);
    let area = centered_rect(screen, width, total_height);

    let title = format!(" Command Palette ({}) ", list.filtered.len());
    let footer = format!(
        "[Enter] execute  [Esc] close  {}/{}",
        if list.filtered.is_empty() {
            0
        } else {
            list.selected + 1
        },
        list.filtered.len()
    );

    let outer_block = Block::default()
        .title(Span::styled(
            title,
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ))
        .title_bottom(Span::styled(footer, Style::default().fg(Color::DarkGray)))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::DarkGray))
        .style(Style::default().bg(Color::Black));

    let inner = outer_block.inner(area);
    let filter_area = Rect {
        x: inner.x,
        y: inner.y,
        width: inner.width,
        height: 1,
    };
    let list_area = Rect {
        x: inner.x,
        y: inner.y.saturating_add(1),
        width: inner.width,
        height: inner.height.saturating_sub(1),
    };

    // Filter line with prompt
    let filter_line = if list.filter.is_empty() {
        Paragraph::new(Line::from(vec![
            Span::styled("> ", Style::default().fg(Color::Cyan)),
            Span::styled(
                "█",
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::ITALIC),
            ),
        ]))
    } else {
        Paragraph::new(Line::from(vec![
            Span::styled("> ", Style::default().fg(Color::Cyan)),
            Span::styled(
                list.filter.clone(),
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled("█", Style::default().fg(Color::Yellow)),
        ]))
    };

    // Build list items with fuzzy highlighting
    let mut list_items = Vec::with_capacity(list_height);
    for i in 0..list_height {
        let entry_idx = list.scroll + i;
        if entry_idx < list.filtered.len() {
            let real_idx = list.filtered[entry_idx];
            let entry = &list.entries[real_idx];
            let is_selected = entry_idx == list.selected;
            let match_indices = list
                .match_indices
                .get(entry_idx)
                .cloned()
                .unwrap_or_default();

            let row_style = if is_selected {
                Style::default().bg(Color::DarkGray).fg(Color::White)
            } else {
                Style::default()
            };

            // Format: name (description) [keyhint]
            let name_text = &entry.name;
            let desc_text = &entry.description;
            let hint_text = &entry.key_hint;

            let mut spans = Vec::new();

            // Highlight name portion if matched
            let mut last_idx = 0;
            for &idx in &match_indices {
                if idx < name_text.len() {
                    if last_idx < idx {
                        spans.push(Span::styled(
                            &name_text[last_idx..idx],
                            if is_selected {
                                Style::default().fg(Color::White)
                            } else {
                                Style::default().fg(Color::Blue)
                            },
                        ));
                    }
                    spans.push(Span::styled(
                        &name_text[idx..idx + 1],
                        if is_selected {
                            Style::default()
                                .fg(Color::Yellow)
                                .bg(Color::DarkGray)
                                .add_modifier(Modifier::BOLD)
                        } else {
                            Style::default()
                                .fg(Color::Yellow)
                                .add_modifier(Modifier::BOLD)
                        },
                    ));
                    last_idx = idx + 1;
                }
            }
            if last_idx < name_text.len() {
                spans.push(Span::styled(
                    &name_text[last_idx..],
                    if is_selected {
                        Style::default().fg(Color::White)
                    } else {
                        Style::default().fg(Color::Blue)
                    },
                ));
            }

            // Description (gray)
            spans.push(Span::styled(
                format!("  {}  ", desc_text),
                if is_selected {
                    Style::default().fg(Color::White)
                } else {
                    Style::default().fg(Color::DarkGray)
                },
            ));
            // Key hint (dim yellow)
            spans.push(Span::styled(
                hint_text,
                if is_selected {
                    Style::default().fg(Color::Yellow)
                } else {
                    Style::default().fg(Color::Yellow)
                },
            ));

            list_items.push(ListItem::new(Line::from(spans)).style(row_style));
        } else {
            list_items.push(ListItem::new(Line::from("")));
        }
    }

    let list_widget = List::new(list_items);
    f.render_widget(Clear, area);
    f.render_widget(outer_block, area);
    f.render_widget(filter_line, filter_area);
    f.render_widget(list_widget, list_area);
}

fn draw_tag_candidates(f: &mut Frame, editor: &Editor, screen: Rect) {
    let popup = match &editor.popup.tag_candidates {
        Some(p) => p,
        None => return,
    };

    let width = popup_width(screen);
    let max_height = (screen.height / 2).max(10) as usize;
    let content_height = popup.entries.len().min(max_height.saturating_sub(2));
    let total_height = (content_height as u16).saturating_add(2); // +2 for borders

    // Anchor: bottom of screen, grow upward (2 rows margin for status/cmd bars)
    let x = (screen.width.saturating_sub(width)) / 2;
    let y = screen.height.saturating_sub(total_height).saturating_sub(2);

    let area = clamp(
        screen,
        Rect {
            x,
            y,
            width,
            height: total_height,
        },
    );

    let title = format!(" Tag Candidates ({}) ", popup.entries.len());
    let footer = " [Enter] jump  [Esc] cancel ";

    let outer_block = Block::default()
        .title(Span::styled(
            title,
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ))
        .title_bottom(Span::styled(footer, Style::default().fg(Color::DarkGray)))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::DarkGray))
        .style(Style::default().bg(Color::Black));

    let inner = outer_block.inner(area);

    let mut list_items = Vec::new();
    for (i, entry) in popup.entries.iter().enumerate() {
        let is_selected = i == popup.selected;
        let row_style = if is_selected {
            Style::default().bg(Color::DarkGray).fg(Color::White)
        } else {
            Style::default()
        };

        let kind_str = format!("{:<10}", entry.kind.as_deref().unwrap_or("unknown"));
        let kind_style = if is_selected {
            Style::default()
                .fg(Color::Black)
                .bg(Color::DarkGray)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(Color::Magenta)
        };

        let name_style = if is_selected {
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(Color::Blue)
        };

        let file_str = entry.file.display().to_string();
        let file_style = if is_selected {
            Style::default().fg(Color::White)
        } else {
            Style::default().fg(Color::DarkGray)
        };

        let line_str = format!(":{}", entry.line);
        let line_style = if is_selected {
            Style::default().fg(Color::White)
        } else {
            Style::default().fg(Color::Green)
        };

        let line = Line::from(vec![
            Span::styled(format!(" {} ", kind_str), kind_style),
            Span::styled(format!("{} ", entry.name), name_style),
            Span::styled(file_str, file_style),
            Span::styled(line_str, line_style),
        ])
        .style(row_style);

        list_items.push(ListItem::new(line));
    }

    // Pad to content_height so the block doesn't collapse
    while list_items.len() < content_height {
        list_items.push(ListItem::new(Line::from("")));
    }

    let list_widget = List::new(list_items);

    f.render_widget(Clear, area);
    f.render_widget(outer_block, area);
    f.render_widget(list_widget, inner);
}

fn draw_function_list(f: &mut Frame, editor: &Editor, screen: Rect) {
    let popup = match &editor.popup.function_list {
        Some(p) => p,
        None => return,
    };
    let list = &popup.list;

    let width = popup_width(screen);
    const VISIBLE_ROWS: usize = 20;
    const TOTAL_HEIGHT: u16 = VISIBLE_ROWS as u16 + 3; // +3 borders+filter
    let area = centered_rect(screen, width, TOTAL_HEIGHT);

    // Scroll offset
    let scroll_offset = if list.selected >= VISIBLE_ROWS {
        list.selected - VISIBLE_ROWS + 1
    } else {
        0
    };

    let title = format!(" Functions ({}) ", list.filtered.len());
    let footer = format!(
        "[Esc] close  [Enter] jump  {}/{}",
        if list.filtered.is_empty() {
            0
        } else {
            list.selected + 1
        },
        list.filtered.len()
    );

    let outer_block = Block::default()
        .title(Span::styled(
            title,
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ))
        .title_bottom(Span::styled(footer, Style::default().fg(Color::DarkGray)))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::DarkGray))
        .style(Style::default().bg(Color::Black));

    let inner = outer_block.inner(area);
    let filter_area = Rect {
        x: inner.x,
        y: inner.y,
        width: inner.width,
        height: 1,
    };
    let list_area = Rect {
        x: inner.x,
        y: inner.y + 1,
        width: inner.width,
        height: inner.height - 1,
    };

    let filter_line = if list.filter.is_empty() {
        Paragraph::new(Line::from(vec![
            Span::styled("Filter: ", Style::default().fg(Color::Gray)),
            Span::styled(
                "█",
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::ITALIC),
            ),
        ]))
    } else {
        Paragraph::new(Line::from(vec![
            Span::styled("Filter: ", Style::default().fg(Color::Gray)),
            Span::styled(
                list.filter.clone(),
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled("█", Style::default().fg(Color::Yellow)),
        ]))
    };

    // Build exactly VISIBLE_ROWS items
    let mut list_items = Vec::with_capacity(VISIBLE_ROWS);
    for i in 0..VISIBLE_ROWS {
        let entry_idx = scroll_offset + i;
        if entry_idx < list.filtered.len() {
            let real_idx = list.filtered[entry_idx];
            if let Some(entry) = list.entries.get(real_idx) {
                let is_selected = entry_idx == list.selected;

                // 1. Combine name and caution symbol directly (if duplicate)
                let name_with_caution = if entry.is_duplicate {
                    format!("{} 🔴 ", entry.name)
                } else {
                    entry.name.clone()
                };

                // 2. Dynamic sizing to push line number to the right edge
                let kind_width = 13; // display_kind() is always 13 chars
                let line_width = 8; // "[L 1234]" is always 8 chars

                // Ensure inner_width is a usize value, not a function pointer
                let inner_width = inner.width as usize;

                // Calculate max allowed name width (leaving at least 2 spaces gap)
                let max_name_width = inner_width
                    .saturating_sub(kind_width)
                    .saturating_sub(line_width)
                    .saturating_sub(2);

                let name_display = if name_with_caution.chars().count() > max_name_width {
                    let mut truncated: String =
                        name_with_caution.chars().take(max_name_width - 1).collect();
                    truncated.push('…');
                    truncated
                } else {
                    name_with_caution.clone() // No trailing spaces, we pad dynamically
                };

                let kind_display = entry.display_kind(); // fixed 13 chars
                let line_display = format!("[L{:>5}]", entry.line + 1);

                // 3. Set styles for uniform rendering on selection or highlight
                let name_style = if is_selected {
                    Style::default()
                        .fg(Color::Yellow)
                        .bg(Color::DarkGray)
                        .add_modifier(Modifier::BOLD)
                } else if entry.is_duplicate {
                    Style::default().fg(Color::Yellow)
                } else {
                    Style::default().fg(Color::Gray)
                };

                let kind_style = if is_selected {
                    Style::default()
                        .fg(Color::Black)
                        .bg(Color::DarkGray)
                        .add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(Color::Magenta)
                };

                let line_style = if is_selected {
                    Style::default().fg(Color::Black).bg(Color::DarkGray)
                } else {
                    Style::default().fg(Color::DarkGray)
                };

                let pad_style = if is_selected {
                    Style::default().bg(Color::DarkGray)
                } else {
                    Style::default()
                };

                // 4. Calculate dynamic padding to push line_display to the right edge
                let name_len = name_display.chars().count();
                let used_width = kind_width + name_len + line_width;
                let padding_needed = inner_width.saturating_sub(used_width);

                let mut spans = vec![
                    Span::styled(kind_display, kind_style),
                    Span::styled(name_display, name_style),
                ];

                if padding_needed > 0 {
                    spans.push(Span::styled(" ".repeat(padding_needed), pad_style));
                }

                spans.push(Span::styled(line_display, line_style));

                list_items.push(ListItem::new(Line::from(spans)));

                continue;
            }
        }
        // Empty row
        list_items.push(ListItem::new(Line::from("")));
    }

    let list_widget = List::new(list_items);
    f.render_widget(Clear, area);
    f.render_widget(outer_block, area);
    f.render_widget(filter_line, filter_area);
    f.render_widget(list_widget, list_area);
}

fn draw_git_hunk_popup(f: &mut Frame, editor: &Editor, screen: Rect) {
    let popup = match &editor.popup.git_hunk {
        Some(p) => p,
        None => return,
    };

    let width = popup_width(screen);
    let max_height = (screen.height / 2).max(10) as usize;
    let content_height = popup.lines.len().min(max_height.saturating_sub(2));
    let total_height = (content_height as u16).saturating_add(2); // +2 for borders

    // Anchor: bottom of screen, grow upward (2 rows margin for status bar)
    let x = (screen.width.saturating_sub(width)) / 2;
    let y = screen.height.saturating_sub(total_height).saturating_sub(2);

    let area = clamp(
        screen,
        Rect {
            x,
            y,
            width,
            height: total_height,
        },
    );

    let title = " Git Hunk Diff ";
    let footer = " [Esc] close  [+] yank added  [-] yank deleted ";

    let outer_block = Block::default()
        .title(Span::styled(
            title,
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ))
        .title_bottom(Span::styled(footer, Style::default().fg(Color::DarkGray)))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::DarkGray))
        .style(Style::default().bg(Color::Black));

    let inner = outer_block.inner(area);
    let mut list_items = Vec::new();

    for (i, line) in popup.lines.iter().enumerate() {
        if i < popup.scroll {
            continue;
        }
        if list_items.len() >= content_height {
            break;
        }

        let style = if line.starts_with("@@") {
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD)
        } else if line.starts_with('+') {
            Style::default().fg(Color::Green)
        } else if line.starts_with('-') {
            Style::default().fg(Color::Red)
        } else {
            Style::default().fg(Color::White)
        };

        list_items.push(ListItem::new(Line::from(vec![Span::styled(
            line.clone(),
            style,
        )])));
    }

    // Pad to content_height so the block doesn't collapse
    while list_items.len() < content_height {
        list_items.push(ListItem::new(Line::from("")));
    }

    let list_widget = List::new(list_items);

    f.render_widget(Clear, area);
    f.render_widget(outer_block, area);
    f.render_widget(list_widget, inner);
}

fn draw_marks(f: &mut Frame, editor: &Editor, screen: Rect) {
    let popup = match &editor.popup.marks {
        Some(p) => p,
        None => return,
    };

    let width = popup_width(screen); // Use 90% screen width like git hunk
    let max_height = (screen.height / 2).max(10) as usize;
    let content_height = popup.entries.len().min(max_height.saturating_sub(2));
    let total_height = (content_height as u16).saturating_add(2); // +2 for borders

    // Anchor: bottom of screen, grow upward (2 rows margin for status bar)
    let x = (screen.width.saturating_sub(width)) / 2;
    let y = screen.height.saturating_sub(total_height).saturating_sub(2);

    let area = clamp(
        screen,
        Rect {
            x,
            y,
            width,
            height: total_height,
        },
    );

    let title = format!(" Marks ({}) ", popup.entries.len());
    let footer = " [Enter] jump  [a-z] quick jump  [Esc] close ";

    let outer_block = Block::default()
        .title(Span::styled(
            title,
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ))
        .title_bottom(Span::styled(footer, Style::default().fg(Color::DarkGray)))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::DarkGray))
        .style(Style::default().bg(Color::Black));

    let inner = outer_block.inner(area);

    let mut list_items = Vec::new();
    for i in 0..content_height {
        let entry_idx = popup.scroll + i;
        if entry_idx < popup.entries.len() {
            let entry = &popup.entries[entry_idx];
            let is_selected = entry_idx == popup.selected;

            let row_style = if is_selected {
                Style::default().bg(Color::DarkGray).fg(Color::White)
            } else {
                Style::default()
            };

            // "Glow" effect for the mark letter
            let mark_style = if is_selected {
                Style::default()
                    .fg(Color::Magenta)
                    .bg(Color::DarkGray)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default()
                    .fg(Color::Magenta)
                    .add_modifier(Modifier::BOLD)
            };

            let name_style = if is_selected {
                Style::default()
                    .fg(Color::Blue)
                    .bg(Color::DarkGray)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(Color::Blue)
            };

            let info_style = if is_selected {
                Style::default().fg(Color::Yellow).bg(Color::DarkGray)
            } else {
                Style::default().fg(Color::Gray)
            };

            let line = Line::from(vec![
                Span::styled(format!(" {} ", entry.ch), mark_style),
                Span::styled(format!(" {:<20} ", entry.buffer_name), name_style),
                Span::styled(
                    format!("line {}, col {}", entry.row + 1, entry.col + 1),
                    info_style,
                ),
            ]);
            list_items.push(ListItem::new(line.style(row_style)));
        } else {
            list_items.push(ListItem::new(Line::from("")));
        }
    }

    let list_widget = List::new(list_items);

    f.render_widget(Clear, area);
    f.render_widget(outer_block, area);
    f.render_widget(list_widget, inner);
}

fn draw_mru(f: &mut Frame, editor: &Editor, screen: Rect) {
    let popup = match &editor.popup.mru {
        Some(p) => p,
        None => return,
    };
    let list = &popup.list;

    let width = popup_width(screen);

    let max_h = screen.height.saturating_sub(4) as usize;
    let list_height = list.visible_height.min(20).min(max_h);

    // +2 for top/bottom borders, +1 for the filter line
    let total_height = (list_height as u16).saturating_add(3);
    let area = centered_rect(screen, width, total_height);

    let sort_label = if popup.sort_by_frequency {
        "by freq"
    } else {
        "recent"
    };
    let repo_label = if popup.repo_only { " [repo]" } else { "" };
    let title = format!(
        " Recent Files ({}){} ({}/{}) ",
        sort_label,
        repo_label,
        list.filtered.len(),
        popup.all_mru_entries.len()
    );

    let footer = format!(
        " [Home] sort  [Tab] repo  [Del] remove  [Enter] open  [Esc] close  {}/{}",
        if list.filtered.is_empty() {
            0
        } else {
            list.selected + 1
        },
        list.filtered.len()
    );

    let outer_block = Block::default()
        .title(Span::styled(
            title,
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ))
        .title_bottom(Span::styled(footer, Style::default().fg(Color::DarkGray)))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::DarkGray))
        .style(Style::default().bg(Color::Black));

    let inner = outer_block.inner(area);
    let filter_area = Rect {
        x: inner.x,
        y: inner.y,
        width: inner.width,
        height: 1,
    };
    let list_area = Rect {
        x: inner.x,
        y: inner.y.saturating_add(1),
        width: inner.width,
        height: inner.height.saturating_sub(1),
    };

    let filter_line = if list.filter.is_empty() {
        Paragraph::new(Line::from(vec![
            Span::styled("Filter: ", Style::default().fg(Color::Gray)),
            Span::styled(
                "█",
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::ITALIC),
            ),
        ]))
    } else {
        Paragraph::new(Line::from(vec![
            Span::styled("Filter: ", Style::default().fg(Color::Gray)),
            Span::styled(
                list.filter.clone(),
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled("█", Style::default().fg(Color::Yellow)),
        ]))
    };

    let inner_width = inner.width.saturating_sub(2) as usize;
    let file_name_width: usize = 25;

    let pos_w = 8;
    let count_w = 5;
    let time_w = 9;

    let left_w = 2 + 2 + file_name_width + 2;
    let meta_right_w = 1 + pos_w + 1 + count_w + 1 + time_w;
    let dir_field_w = inner_width.saturating_sub(left_w + meta_right_w);

    let mut scroll = list.scroll;
    if !list.filtered.is_empty() && list.selected >= scroll + list_height {
        scroll = list.selected - list_height + 1;
    }
    if list.selected < scroll {
        scroll = list.selected;
    }

    let mut list_items = Vec::with_capacity(list_height);
    for i in 0..list_height {
        let entry_idx = scroll + i;
        if entry_idx < list.filtered.len() {
            let real_idx = list.filtered[entry_idx];
            let entry = &list.entries[real_idx];
            let is_selected = entry_idx == list.selected;

            let row_style = if is_selected {
                Style::default().bg(Color::DarkGray).fg(Color::White)
            } else {
                Style::default()
            };

            let file_stem_raw = entry
                .path
                .file_name()
                .and_then(|n: &std::ffi::OsStr| n.to_str())
                .unwrap_or("?")
                .to_string();

            let file_stem = if file_stem_raw.chars().count() > file_name_width {
                let mut s: String = file_stem_raw.chars().take(file_name_width - 1).collect();
                s.push('…');
                s
            } else {
                file_stem_raw.clone()
            };

            let idx_str = format!("{:>2}", entry_idx + 1);
            let idx_style = if is_selected {
                Style::default().fg(Color::White)
            } else {
                Style::default().fg(Color::DarkGray)
            };

            let name_color = if is_selected {
                Color::White
            } else {
                Color::Blue
            };
            let mut name_spans = vec![];

            if !list.filter.is_empty() {
                if let Some((start, end)) = fuzzy::substring_find(&file_stem, &list.filter) {
                    let prefix = &file_stem[..start];
                    let matched = &file_stem[start..end];
                    let suffix = &file_stem[end..];

                    if !prefix.is_empty() {
                        name_spans.push(Span::styled(
                            prefix.to_string(),
                            Style::default().fg(name_color),
                        ));
                    }
                    name_spans.push(Span::styled(
                        matched.to_string(),
                        Style::default()
                            .fg(Color::Yellow)
                            .add_modifier(Modifier::BOLD),
                    ));
                    if !suffix.is_empty() {
                        name_spans.push(Span::styled(
                            suffix.to_string(),
                            Style::default().fg(name_color),
                        ));
                    }
                } else {
                    name_spans.push(Span::styled(
                        file_stem.clone(),
                        Style::default().fg(name_color),
                    ));
                }
            } else {
                name_spans.push(Span::styled(
                    file_stem.clone(),
                    Style::default().fg(name_color),
                ));
            }

            let name_len = file_stem.chars().count();
            let name_pad = " ".repeat(file_name_width.saturating_sub(name_len));
            name_spans.push(Span::raw(name_pad));

            let dir_str = entry
                .path
                .parent()
                .and_then(|p: &std::path::Path| p.to_str())
                .unwrap_or("")
                .to_string();

            let compressed = if dir_str.chars().count() <= dir_field_w {
                dir_str.clone()
            } else if dir_field_w > 0 {
                let mut c = String::new();
                let mut first = true;
                for part in dir_str.split('/').collect::<Vec<&str>>() {
                    let part: &str = part;
                    if !first {
                        c.push('/');
                    }
                    first = false;
                    if part.is_empty() {
                        continue;
                    }
                    if let Some(ch) = part.chars().next() {
                        c.push(ch);
                    }
                }
                if c.chars().count() <= dir_field_w {
                    c
                } else {
                    let trunc = dir_field_w.saturating_sub(1);
                    if trunc == 0 {
                        String::new()
                    } else {
                        let chars: Vec<char> = c.chars().collect();
                        let start = chars.len().saturating_sub(trunc);
                        format!("…{}", chars[start..].iter().collect::<String>())
                    }
                }
            } else {
                String::new()
            };

            let dir_w = compressed.chars().count();
            let dir_display = if dir_w < dir_field_w {
                format!("{}{}", compressed, " ".repeat(dir_field_w - dir_w))
            } else {
                compressed
            };
            let dir_style = if is_selected {
                Style::default().fg(Color::White)
            } else {
                Style::default().fg(Color::DarkGray)
            };

            let pos_raw = format!("{}:{}", entry.line + 1, entry.col + 1);
            let pos_pad = " ".repeat(pos_w.saturating_sub(pos_raw.chars().count()));
            let pos_col = format!("{}{}", pos_pad, pos_raw);

            let count_raw = if entry.open_count > 1 {
                format!("×{}", entry.open_count)
            } else {
                String::new()
            };
            let count_pad = " ".repeat(count_w.saturating_sub(count_raw.chars().count()));
            let count_col = format!("{}{}", count_pad, count_raw);

            let time_raw = entry.relative_time();
            let time_pad = " ".repeat(time_w.saturating_sub(time_raw.chars().count()));
            let time_col = format!("{}{}", time_pad, time_raw);

            let mut line_spans = vec![
                Span::styled(idx_str, idx_style),
                Span::styled(" ", Style::default().fg(Color::DarkGray)),
            ];
            line_spans.extend(name_spans);
            line_spans.push(Span::styled(" ", Style::default().fg(Color::DarkGray)));
            line_spans.push(Span::styled(dir_display, dir_style));
            line_spans.push(Span::styled(" ", Style::default().fg(Color::DarkGray)));
            line_spans.push(Span::styled(pos_col, Style::default().fg(Color::Yellow)));
            line_spans.push(Span::styled(" ", Style::default().fg(Color::DarkGray)));
            line_spans.push(Span::styled(count_col, Style::default().fg(Color::Red)));
            line_spans.push(Span::styled(" ", Style::default().fg(Color::DarkGray)));
            line_spans.push(Span::styled(time_col, Style::default().fg(Color::DarkGray)));

            list_items.push(ListItem::new(Line::from(line_spans).style(row_style)));
        } else {
            list_items.push(ListItem::new(Line::from("")));
        }
    }

    let list_widget = List::new(list_items);

    f.render_widget(Clear, area);
    f.render_widget(outer_block, area);
    f.render_widget(filter_line, filter_area);
    f.render_widget(list_widget, list_area);
}

// ── Tier 2: live which-key overlay ────────────────────────────────

/// Derives suggestions fresh from `editor.pending_keys` every frame.
/// Auto-fits width and height to actual content.
/// Collapses duplicate first-key prefixes into `key+(N)` rows.
/// Called once from `render::draw`, after `draw_popup`.
pub fn draw_which_key(f: &mut Frame, editor: &Editor) {
    if editor.pending_keys.is_empty() {
        return;
    }
    if editor.popup.is_open() {
        return;
    }

    // Pass `editor.mode()` directly into the generator
    let suggestions = crate::keybind::get_sequence_suggestions(
        &editor.config,
        &editor.pending_keys,
        editor.mode(),
    );
    let screen = f.area();
    let display_pending = compact_mods(&editor.pending_keys);

    // ── No continuations: bare badge ──────────────────────────────
    if suggestions.is_empty() {
        let hint = format!(" {display_pending} ");
        let width = (hint.len().saturating_add(2)) as u16;
        let area = clamp(
            screen,
            Rect {
                x: screen.width.saturating_sub(width.saturating_add(1)),
                y: screen.height.saturating_sub(3),
                width,
                height: 1,
            },
        );
        f.render_widget(Clear, area);
        f.render_widget(
            Paragraph::new(hint).style(
                Style::default()
                    .fg(Color::Yellow)
                    .bg(Color::Rgb(50, 50, 60)),
            ),
            area,
        );
        return;
    }

    // ── Group suggestions by first token of suffix ────────────────
    // When multiple bindings share the same next key (e.g. "p v" and "p p"),
    // collapse them into a single row: "p+(2) → 2 bindings"
    struct DisplayRow {
        suffix: String,
        description: String,
        is_collapse: bool,
    }

    let mut first_token_groups: std::collections::BTreeMap<String, Vec<usize>> =
        std::collections::BTreeMap::new();

    for (i, sug) in suggestions.iter().enumerate() {
        let first_token = sug
            .suffix
            .split_whitespace()
            .next()
            .unwrap_or(&sug.suffix)
            .to_string();
        first_token_groups.entry(first_token).or_default().push(i);
    }

    let mut display_rows: Vec<DisplayRow> = Vec::new();
    for (first_token, indices) in &first_token_groups {
        if indices.len() > 1 {
            // Multiple bindings share this first key → collapse with count
            display_rows.push(DisplayRow {
                suffix: format!("{}+", compact_mods(first_token)),
                description: format!("{} bindings", indices.len()),
                is_collapse: true,
            });
        } else {
            let sug = &suggestions[indices[0]];
            display_rows.push(DisplayRow {
                suffix: compact_mods(&sug.suffix),
                description: sug.description.clone(),
                is_collapse: false,
            });
        }
    }

    let is_last_match = suggestions.len() == 1;

    // ── Column widths from display entries (auto-fit) ─────────────
    let max_suffix_w = display_rows
        .iter()
        .map(|e| e.suffix.len())
        .max()
        .unwrap_or(1);
    let max_desc_w = display_rows
        .iter()
        .map(|e| e.description.len())
        .max()
        .unwrap_or(1);

    // " {suffix}  →  {description} " + 2 border cols
    let inner_w = 1usize
        .saturating_add(max_suffix_w)
        .saturating_add(1)
        .saturating_add(3)
        .saturating_add(max_desc_w)
        .saturating_add(1);
    let box_w = (inner_w.saturating_add(2)).max(30).min(u16::MAX as usize) as u16;
    let box_h = (display_rows.len().saturating_add(2)).min(u16::MAX as usize) as u16;

    let area = clamp(
        screen,
        Rect {
            x: screen.width.saturating_sub(box_w.saturating_add(1)),
            y: screen.height.saturating_sub(box_h.saturating_add(2)),
            width: box_w,
            height: box_h,
        },
    );

    // ── Theme-consistent layout colors ────────────────────────────
    let suffix_sty = Style::default()
        .fg(Color::Yellow)
        .add_modifier(Modifier::BOLD);
    let arrow_sty = Style::default().fg(Color::DarkGray);
    let desc_sty = Style::default().fg(Color::White);
    let collapse_suffix_sty = Style::default()
        .fg(Color::Cyan)
        .add_modifier(Modifier::BOLD);

    let lines: Vec<Line> = display_rows
        .iter()
        .map(|row| {
            let padded_suffix = format!(" {:<width$} ", row.suffix, width = max_suffix_w);
            let padded_desc = format!("{:<width$} ", row.description, width = max_desc_w);

            let s_sty = if row.is_collapse {
                collapse_suffix_sty
            } else {
                suffix_sty
            };

            Line::from(vec![
                Span::styled(padded_suffix, s_sty),
                Span::styled("→ ", arrow_sty),
                Span::styled(padded_desc, desc_sty),
            ])
        })
        .collect();

    // ── Title: append "auto ▶" when about to fire ──────────────────
    let title = if is_last_match {
        format!(" Which key ({display_pending}) — auto ▶ ")
    } else {
        format!(" Which key ({display_pending}) ")
    };

    // Keep border color consistently DarkGray to match other popup blocks
    let border_color = Color::DarkGray;

    let block = Block::default()
        .title(Span::styled(
            title,
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(border_color))
        .style(Style::default().bg(Color::Black));

    f.render_widget(Clear, area);
    f.render_widget(Paragraph::new(lines).block(block), area);
}

fn draw_error(f: &mut Frame, editor: &Editor, screen: Rect) {
    let popup = match &editor.popup.error {
        Some(p) => p,
        None => return,
    };

    let width = popup_width(screen);
    let max_height = (screen.height / 2).max(10) as usize;
    let content_height = popup.lines.len().min(max_height.saturating_sub(2));
    let total_height = (content_height as u16).saturating_add(2); // +2 for borders

    // Anchor: bottom of screen, grow upward (2 rows margin for status bar)
    let x = (screen.width.saturating_sub(width)) / 2;
    let y = screen.height.saturating_sub(total_height).saturating_sub(2);

    let area = clamp(
        screen,
        Rect {
            x,
            y,
            width,
            height: total_height,
        },
    );

    let dark_yellow = Color::Rgb(120, 90, 18);

    let outer_block = Block::default()
        .title(Span::styled(
            " Error ",
            Style::default()
                .fg(dark_yellow)
                .add_modifier(Modifier::BOLD),
        ))
        .title_bottom(Span::styled(
            " [Esc] close ",
            Style::default().fg(Color::DarkGray),
        ))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(dark_yellow))
        .style(Style::default().bg(Color::Black));

    let inner = outer_block.inner(area);

    let mut list_items = Vec::new();
    for line in &popup.lines {
        list_items.push(ListItem::new(Line::from(Span::styled(
            format!(" {}", line),
            Style::default().fg(dark_yellow),
        ))));
    }

    // Pad to content_height so the block doesn't collapse
    while list_items.len() < content_height {
        list_items.push(ListItem::new(Line::from("")));
    }

    let list_widget = List::new(list_items);

    f.render_widget(Clear, area);
    f.render_widget(outer_block, area);
    f.render_widget(list_widget, inner);
}
