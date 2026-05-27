//! MRU (Most Recently Used) popup overlay for opening recent files.

use crate::popup::Scrollable;
use crate::render::rounded_box::*;
use crossterm::cursor::MoveTo;
use crossterm::execute;
use crossterm::style::{Print, ResetColor, SetBackgroundColor, SetForegroundColor};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

// ── Local Case-Insensitive Search Helper ──
pub fn case_insensitive_find(haystack: &str, needle: &str) -> Option<(usize, usize)> {
    if needle.is_empty() {
        return None;
    }
    let haystack_lower = haystack.to_lowercase();
    let needle_lower = needle.to_lowercase();

    if let Some(_byte_idx) = haystack_lower.find(&needle_lower) {
        let haystack_chars: Vec<char> = haystack.chars().collect();
        let needle_chars: Vec<char> = needle.chars().collect();

        for i in 0..=haystack_chars.len().saturating_sub(needle_chars.len()) {
            let mut matches = true;
            for j in 0..needle_chars.len() {
                if haystack_chars[i + j].to_lowercase().to_string()
                    != needle_chars[j].to_lowercase().to_string()
                {
                    matches = false;
                    break;
                }
            }
            if matches {
                let start_byte = haystack
                    .char_indices()
                    .nth(i)
                    .map(|(idx, _)| idx)
                    .unwrap_or(0);
                let end_byte = haystack
                    .char_indices()
                    .nth(i + needle_chars.len())
                    .map(|(idx, _)| idx)
                    .unwrap_or(haystack.len());
                return Some((start_byte, end_byte));
            }
        }
    }
    None
}

// ── MRU Entry (Now Serializeable) ──
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MruEntry {
    pub path: PathBuf,
    pub line: usize,
    pub col: usize,
    pub open_count: usize,
    pub last_opened: std::time::SystemTime,
}

impl MruEntry {
    pub fn relative_time(&self) -> String {
        let elapsed = self
            .last_opened
            .elapsed()
            .unwrap_or(std::time::Duration::ZERO);
        let secs = elapsed.as_secs();
        if secs < 60 {
            "just now".to_string()
        } else if secs < 3600 {
            format!("{}m ago", secs / 60)
        } else if secs < 86400 {
            format!("{}h ago", secs / 3600)
        } else {
            format!("{}d ago", secs / 86400)
        }
    }
}

// ── MRU Manager (With Persistent Disk I/O) ──
#[derive(Debug, Clone, Default)]
pub struct MruManager {
    pub entries: Vec<MruEntry>,
}

impl MruManager {
    pub fn new() -> Self {
        Self {
            entries: Vec::new(),
        }
    }

    /// Loads history from your user configuration directory (e.g. `~/.config/ce/mru.json`)
    pub fn load() -> Self {
        if let Ok(dir) = crate::config::app_config::Config::config_dir() {
            let path = dir.join("mru.json");
            if path.exists() {
                if let Ok(content) = std::fs::read_to_string(path) {
                    if let Ok(entries) = serde_json::from_str::<Vec<MruEntry>>(&content) {
                        return Self { entries };
                    }
                }
            }
        }
        Self::new()
    }

    /// Saves the MRU list to disk in JSON format
    pub fn save(&self) {
        if let Ok(dir) = crate::config::app_config::Config::config_dir() {
            let path = dir.join("mru.json");
            if let Ok(content) = serde_json::to_string_pretty(&self.entries) {
                let _ = std::fs::write(path, content);
            }
        }
    }

    pub fn get_entries(&self) -> Vec<MruEntry> {
        let mut sorted = self.entries.clone();
        sorted.sort_by(|a, b| b.last_opened.cmp(&a.last_opened));
        sorted
    }

    pub fn entries_by_frequency(&self) -> Vec<MruEntry> {
        let mut sorted = self.entries.clone();
        sorted.sort_by(|a, b| {
            b.open_count
                .cmp(&a.open_count)
                .then_with(|| b.last_opened.cmp(&a.last_opened))
        });
        sorted
    }

    pub fn insert(&mut self, path: PathBuf, line: usize, col: usize) {
        let canon_path = std::fs::canonicalize(&path).unwrap_or(path);

        if let Some(pos) = self.entries.iter().position(|e| e.path == canon_path) {
            let entry = &mut self.entries[pos];
            entry.line = line;
            entry.col = col;
            entry.open_count += 1;
            entry.last_opened = std::time::SystemTime::now();
        } else {
            self.entries.push(MruEntry {
                path: canon_path,
                line,
                col,
                open_count: 1,
                last_opened: std::time::SystemTime::now(),
            });
        }

        if self.entries.len() > 100 {
            let mut sorted = self.entries.clone();
            sorted.sort_by(|a, b| b.last_opened.cmp(&a.last_opened));
            sorted.truncate(100);
            self.entries = sorted;
        }

        self.save();
    }
}

// ── MRU Popup Struct ──
#[derive(Debug, Clone)]
pub struct MruPopup {
    pub entries: Vec<MruEntry>,
    pub filtered: Vec<usize>,
    pub selected: usize,
    pub scroll: usize,
    pub filter: String,
    pub sort_by_frequency: bool,
    pub repo_root: Option<PathBuf>,
    pub repo_only: bool,
}

impl MruPopup {
    pub fn new(entries: Vec<MruEntry>, repo_root: Option<PathBuf>, repo_only: bool) -> Self {
        let filtered: Vec<usize> = (0..entries.len()).collect();
        MruPopup {
            entries,
            filtered,
            selected: 0,
            scroll: 0,
            filter: String::new(),
            sort_by_frequency: false,
            repo_root,
            repo_only,
        }
    }

    pub fn selected_entry(&self) -> Option<&MruEntry> {
        self.filtered
            .get(self.selected)
            .and_then(|&idx| self.entries.get(idx))
    }

    pub(crate) fn apply_filter(&mut self) {
        self.filtered.clear();
        let query = self.filter.to_lowercase();
        for (i, entry) in self.entries.iter().enumerate() {
            if self.repo_only {
                if let Some(root) = &self.repo_root {
                    if !entry.path.starts_with(root) {
                        continue;
                    }
                }
            }

            let file_name = entry
                .path
                .file_name()
                .and_then(|n: &std::ffi::OsStr| n.to_str())
                .unwrap_or("")
                .to_string();
            let dir_str = entry
                .path
                .parent()
                .and_then(|p: &std::path::Path| p.to_str())
                .unwrap_or("")
                .to_string();

            if query.is_empty()
                || file_name.to_lowercase().contains(&query)
                || dir_str.to_lowercase().contains(&query)
            {
                self.filtered.push(i);
            }
        }
        self.clamp_scroll();
    }

    pub fn filter_push(&mut self, c: char) {
        self.filter.push(c);
        self.selected = 0;
        self.scroll = 0;
        self.apply_filter();
    }

    pub fn filter_pop(&mut self) {
        self.filter.pop();
        self.selected = 0;
        self.scroll = 0;
        self.apply_filter();
    }

    /// Removes selection from display and dynamically synchronizes deletion to disk
    pub fn remove_selected(&mut self, mru: &mut MruManager) {
        if let Some(&real_idx) = self.filtered.get(self.selected) {
            let entry = &self.entries[real_idx];
            mru.entries.retain(|e| e.path != entry.path);
            mru.save();

            self.entries.remove(real_idx);
            self.apply_filter();
        }
    }

    pub fn toggle_sort(&mut self, mru: &MruManager) -> bool {
        self.sort_by_frequency = !self.sort_by_frequency;
        if self.sort_by_frequency {
            self.entries = mru.entries_by_frequency();
        } else {
            self.entries = mru.get_entries();
        }
        self.selected = 0;
        self.scroll = 0;
        self.apply_filter();
        self.sort_by_frequency
    }

    pub fn toggle_repo_filter(&mut self) {
        if self.repo_root.is_none() {
            return;
        }
        self.repo_only = !self.repo_only;
        self.selected = 0;
        self.scroll = 0;
        self.apply_filter();
    }

    pub fn move_up(&mut self) {
        if self.filtered.is_empty() {
            return;
        }
        if self.selected > 0 {
            self.selected -= 1;
        } else {
            self.selected = self.filtered.len().saturating_sub(1);
        }
        self.clamp_scroll();
    }

    pub fn move_down(&mut self) {
        if self.filtered.is_empty() {
            return;
        }
        if self.selected + 1 < self.filtered.len() {
            self.selected += 1;
        } else {
            self.selected = 0;
        }
        self.clamp_scroll();
    }

    pub fn clamp_scroll(&mut self) {
        let len = self.filtered.len();
        if self.selected >= len {
            self.selected = len.saturating_sub(1);
        }
        if self.selected < self.scroll {
            self.scroll = self.selected;
        } else if self.selected >= self.scroll + 23 {
            self.scroll = self.selected + 1 - 23;
        }
    }
}

impl Scrollable for MruPopup {
    fn selected(&self) -> usize {
        self.selected
    }
    fn selected_mut(&mut self) -> &mut usize {
        &mut self.selected
    }
    fn scroll_mut(&mut self) -> &mut usize {
        &mut self.scroll
    }
    fn len(&self) -> usize {
        self.filtered.len()
    }
    fn visible_rows(&self) -> usize {
        23
    }
}

#[rustfmt::skip]
pub fn render_mru_popup(
    popup: &MruPopup,
    stdout: &mut std::io::Stdout,
    term_width: u16,
    term_height: u16,
) -> Result<(), Box<dyn std::error::Error>> {
    let status_h = 6u16;
    let edit_h = term_height.saturating_sub(status_h);
    let popup_width = clamp_width(100, term_width, 4);
    let content_rows = clamp_height(20, edit_h.saturating_sub(4), 5) as usize;
    let popup_height = content_rows as u16 + 3;

    let (x, y) = centered_in_edit(popup_width, popup_height, term_width, term_height, status_h);
    clear_rect(stdout, x, y, popup_width, popup_height, catppuccin::MANTLE)?;

    // ── Title bar ──────────────────────────────────────────────────────
    let sort_label = if popup.sort_by_frequency { "by freq" } else { "recent" };
    let repo_label = if popup.repo_only { " [repo]" } else { "" };
    let title = format!(
        " Recent Files ({}){} {} ",
        sort_label,
        repo_label,
        if popup.filtered.is_empty() {
            "(no match)".to_string()
        } else {
            format!("({}/{})", popup.filtered.len(), popup.entries.len())
        }
    );    
    let title_style = BoxStyle::default()
        .with_title(title)
        .with_bg(catppuccin::MANTLE);
    draw_top_border(stdout, x, y, popup_width, &title_style)?;

    // ── Filter row ─────────────────────────────────────────────────────
    let filter_y = y + 1;
    {
        let filter_style = RowStyle::normal().with_bg(catppuccin::CRUST).no_padding();
        let prompt_w = str_width(">");
        let max_filter_len = content_width(popup_width, &filter_style).saturating_sub(prompt_w + 1);
        let filter_display = truncate_to_width(&popup.filter, max_filter_len);

        let segments = [
            Segment::new(">", catppuccin::PEACH),
            Segment::new(filter_display, catppuccin::TEXT),
        ];
        draw_row(stdout, x, filter_y, popup_width, &segments, &filter_style)?;

        let cursor_x = x as usize + 1 + prompt_w + str_width(filter_display);
        if (cursor_x as u16) < x + popup_width.saturating_sub(1) {
            execute!(stdout, MoveTo(cursor_x as u16, filter_y))?;
            execute!(
                stdout,
                SetBackgroundColor(catppuccin::TEXT),
                SetForegroundColor(catppuccin::CRUST),
                Print(" ")
            )?;
        }
    }

    // ── Content rows ───────────────────────────────────────────────────
    let inner_width = popup_width.saturating_sub(2) as usize;
    let file_name_width: usize = 25;

    let pos_w = 8;
    let count_w = 5;
    let time_w = 9;

    let left_w = 2 + 2 + file_name_width + 2;
    let meta_right_w = 1 + pos_w + 1 + count_w + 1 + time_w;
    let dir_field_w = inner_width.saturating_sub(left_w + meta_right_w);

    let mut scroll = popup.scroll;
    if !popup.filtered.is_empty() && popup.selected >= scroll + content_rows {
        scroll = popup.selected - content_rows + 1;
    }
    if popup.selected < scroll {
        scroll = popup.selected;
    }

    for i in 0..content_rows {
        let row_y = filter_y + 1 + i as u16;
        let entry_idx = scroll + i;

        if entry_idx < popup.filtered.len() {
            let real_idx = popup.filtered[entry_idx];
            let entry = &popup.entries[real_idx];
            let is_selected = entry_idx == popup.selected;
            let row_style = if is_selected {
                RowStyle::selected().with_padding(0, 0)
            } else {
                RowStyle::normal().with_padding(0, 0)
            };

            let file_stem_raw = entry
                .path
                .file_name()
                .and_then(|n: &std::ffi::OsStr| n.to_str())
                .unwrap_or("?")
                .to_string();

            let displayed_name_raw = if str_width(&file_stem_raw) > file_name_width {
                let mut s = truncate_to_width(&file_stem_raw, file_name_width.saturating_sub(1))
                    .to_string();
                s.push('…');
                s
            } else {
                file_stem_raw.clone()
            };

            let idx_str = format!("{:>2}", entry_idx + 1);

            let pos_raw = format!("{}:{}", entry.line + 1, entry.col + 1);
            let pos_pad = " ".repeat(pos_w.saturating_sub(str_width(&pos_raw)));
            let pos_col = format!("{}{}", pos_pad, pos_raw);

            let count_raw = if entry.open_count > 1 {
                format!("×{}", entry.open_count)
            } else {
                String::new()
            };
            let count_pad = " ".repeat(count_w.saturating_sub(str_width(&count_raw)));
            let count_col = format!("{}{}", count_pad, count_raw);

            let time_raw = entry.relative_time();
            let time_pad = " ".repeat(time_w.saturating_sub(str_width(&time_raw)));
            let time_col = format!("{}{}", time_pad, time_raw);

            let dir_str = entry
                .path
                .parent()
                .and_then(|p: &std::path::Path| p.to_str())
                .unwrap_or("")
                .to_string();

            let compressed = if str_width(&dir_str) <= dir_field_w {
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
                if str_width(&c) <= dir_field_w {
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

            let dir_w = str_width(&compressed);
            let dir_display = if dir_w < dir_field_w {
                format!("{}{}", compressed, " ".repeat(dir_field_w - dir_w))
            } else {
                compressed
            };

            // ── Color Overrides for Selection State ──
            let name_color = if is_selected { catppuccin::TEXT } else { catppuccin::BLUE };
            let pos_color = if is_selected { catppuccin::TEXT } else { catppuccin::YELLOW };
            let count_color = if is_selected { catppuccin::SUBTEXT } else { catppuccin::PEACH };
            let time_color = if is_selected { catppuccin::SUBTEXT } else { catppuccin::OVERLAY0 };
            let index_color = if is_selected { catppuccin::SUBTEXT } else { catppuccin::OVERLAY0 };
            let dir_color = if is_selected { catppuccin::SUBTEXT } else { catppuccin::OVERLAY0 };
            let sep_color = if is_selected { catppuccin::SURFACE2 } else { catppuccin::SURFACE1 };

            let (prefix, matched, suffix): (String, String, String) = if !popup.filter.is_empty() {
                if let Some((start, end)) =
                    case_insensitive_find(&displayed_name_raw, &popup.filter)
                {
                    (
                        displayed_name_raw[..start].to_string(),
                        displayed_name_raw[start..end].to_string(),
                        displayed_name_raw[end..].to_string(),
                    )
                } else {
                    (displayed_name_raw.clone(), String::new(), String::new())
                }
            } else {
                (displayed_name_raw.clone(), String::new(), String::new())
            };

            let mut segments: Vec<Segment> = Vec::new();
            segments.push(Segment::new(" ", if is_selected { catppuccin::SURFACE0 } else { catppuccin::MANTLE }));
            segments.push(Segment::new(&idx_str, index_color));
            segments.push(Segment::new("  ", sep_color));

            if !prefix.is_empty() {
                segments.push(Segment::new(&prefix, name_color));
            }
            if !matched.is_empty() {
                segments.push(Segment::new(&matched, catppuccin::PEACH));
            }
            if !suffix.is_empty() {
                segments.push(Segment::new(&suffix, name_color));
            }

            let displayed_w = str_width(&displayed_name_raw);
            let name_pad = " ".repeat(file_name_width.saturating_sub(displayed_w));
            if !name_pad.is_empty() {
                segments.push(Segment::new(&name_pad, sep_color));
            }

            segments.push(Segment::new("  ", sep_color));
            segments.push(Segment::new(&dir_display, dir_color));

            segments.push(Segment::new(" ", sep_color));
            segments.push(Segment::new(&pos_col, pos_color));
            segments.push(Segment::new(" ", sep_color));
            segments.push(Segment::new(&count_col, count_color));
            segments.push(Segment::new(" ", sep_color));
            segments.push(Segment::new(&time_col, time_color));

            draw_row(stdout, x, row_y, popup_width, &segments, &row_style)?;
        } else {
            draw_empty_row(stdout, x, row_y, popup_width, &RowStyle::normal())?;
        }
    }

    // ── Bottom border with status ──────────────────────────────────────
    let bottom_y = filter_y + 1 + content_rows as u16;
    let repo_indicator = if popup.repo_only { " ✓" } else { "" };
    let footer = format!(
        "[Home] {} \u{f444} [Tab] repo{} \u{f444} [Del] remove \u{f444} [Enter] open \u{f444} [Esc]{}close  {}/{}",
        if popup.sort_by_frequency { "recency" } else { "freq" },
        repo_indicator,
        if popup.filter.is_empty() { " " } else { " clear " },
        if popup.filtered.is_empty() { 0 } else { popup.selected + 1 },
        popup.filtered.len(),
    );    
    let footer_style = BoxStyle::default()
        .with_footer(footer)
        .with_bg(catppuccin::MANTLE);
    draw_bottom_border(stdout, x, bottom_y, popup_width, &footer_style)?;

    execute!(stdout, ResetColor)?;
    Ok(())
}
