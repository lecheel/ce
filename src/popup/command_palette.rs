// src/popup/command_palette.rs

use crate::config::app_config::{Config, DescEntry, DescOverrides};
use crate::keybind::bindings::Action;
use crate::keybind::palette_defaults;
use crate::popup::Scrollable;

/// Single command palette entry
#[derive(Debug, Clone)]
pub struct CommandEntry {
    pub action: Action,
    pub name: String,        // e.g. "Save"
    pub description: String, // e.g. "Save the current buffer"
    pub key_hint: String,    // e.g. ":w" or "Ctrl+S"
}

/// Popup state for command palette
#[derive(Debug, Clone)]
pub struct CommandPalettePopup {
    pub all_entries: Vec<CommandEntry>,
    pub filtered: Vec<usize>,
    pub selected: usize,
    pub scroll: usize,
    pub filter: String,
    pub match_indices: Vec<Vec<usize>>, // for highlighting
}

impl CommandPalettePopup {
    pub fn new(entries: Vec<CommandEntry>) -> Self {
        let filtered: Vec<usize> = (0..entries.len()).collect();
        Self {
            all_entries: entries,
            filtered,
            selected: 0,
            scroll: 0,
            filter: String::new(),
            match_indices: Vec::new(),
        }
    }

    pub fn selected_entry(&self) -> Option<&CommandEntry> {
        self.filtered
            .get(self.selected)
            .and_then(|&i| self.all_entries.get(i))
    }

    fn apply_filter(&mut self) {
        self.filtered.clear();
        self.match_indices.clear();
        let query = self.filter.to_lowercase();
        if query.is_empty() {
            // no filter: show all
            self.filtered = (0..self.all_entries.len()).collect();
            self.match_indices = vec![vec![]; self.all_entries.len()];
        } else {
            for (i, entry) in self.all_entries.iter().enumerate() {
                let haystack = format!(
                    "{} {}",
                    entry.name.to_lowercase(),
                    entry.description.to_lowercase()
                );
                if let Some(indices) = fuzzy_match(&haystack, &query) {
                    self.filtered.push(i);
                    self.match_indices.push(indices);
                } else {
                    self.match_indices.push(vec![]); // placeholder
                }
            }
        }
        if self.selected >= self.filtered.len() && !self.filtered.is_empty() {
            self.selected = self.filtered.len() - 1;
        }
        self.clamp_scroll(20);
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

    pub fn filter_clear(&mut self) {
        self.filter.clear();
        self.selected = 0;
        self.scroll = 0;
        self.apply_filter();
    }

    pub fn clamp_scroll(&mut self, visible_height: usize) {
        if self.scroll > self.selected {
            self.scroll = self.selected;
        }
        if self.selected >= self.scroll + visible_height {
            self.scroll = self.selected - visible_height + 1;
        }
    }

    pub fn move_up(&mut self) {
        if self.selected > 0 {
            self.selected -= 1;
            self.clamp_scroll(20);
        }
    }

    pub fn move_down(&mut self) {
        if self.selected + 1 < self.filtered.len() {
            self.selected += 1;
            self.clamp_scroll(20);
        }
    }
}

impl Scrollable for CommandPalettePopup {
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
        20
    }
}

/// Simple fuzzy matching: returns indices of matched characters in haystack if all query chars are found in order.
fn fuzzy_match(haystack: &str, query: &str) -> Option<Vec<usize>> {
    let haystack_chars: Vec<char> = haystack.chars().collect();
    let query_chars: Vec<char> = query.chars().collect();
    let mut indices = Vec::new();
    let mut haystack_idx = 0;
    for qc in query_chars {
        while haystack_idx < haystack_chars.len() && haystack_chars[haystack_idx] != qc {
            haystack_idx += 1;
        }
        if haystack_idx == haystack_chars.len() {
            return None;
        }
        indices.push(haystack_idx);
        haystack_idx += 1;
    }
    Some(indices)
}

/// Build the palette by combining compile-time defaults with user overrides.
pub fn build_command_entries() -> Vec<CommandEntry> {
    let overrides = Config::load_descriptions();

    palette_defaults::palette_defaults()
        .iter()
        .map(|&(action, default_desc, default_hint)| {
            let snake = action.snake_name();
            let ov = overrides.overrides.get(&snake);

            CommandEntry {
                action,
                // name defaults to the auto-generated snake_name
                name: ov.and_then(|o| o.name.clone()).unwrap_or(snake),
                description: ov
                    .and_then(|o| o.description.clone())
                    .unwrap_or_else(|| default_desc.to_string()),
                key_hint: ov
                    .and_then(|o| o.key_hint.clone())
                    .unwrap_or_else(|| default_hint.to_string()),
            }
        })
        .collect()
}

/// Generates a `desc.json` file populated with all default action descriptions.
/// Overwrites existing file.
pub fn generate_default_desc_file() -> anyhow::Result<()> {
    let defaults = palette_defaults::palette_defaults();
    let mut overrides_map = std::collections::HashMap::new();

    for &(action, default_desc, default_hint) in defaults {
        let snake = action.snake_name();
        overrides_map.insert(
            snake,
            DescEntry {
                // Populate with actual current defaults so the user knows what they are overriding
                name: Some(action.snake_name()),
                description: Some(default_desc.to_string()),
                key_hint: Some(default_hint.to_string()),
            },
        );
    }

    let data = DescOverrides {
        overrides: overrides_map,
    };

    let path = Config::descriptions_path()?;
    let content = serde_json::to_string_pretty(&data)?;
    std::fs::write(&path, content)?;

    Ok(())
}
