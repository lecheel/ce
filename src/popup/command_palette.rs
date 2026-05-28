// src/popup/command_palette.rs

use crate::config::app_config::{Config, DescEntry, DescOverrides};
use crate::keybind::bindings::Action;
use crate::keybind::palette_defaults;
use crate::popup::filtered_list::{EntryFilter, FilteredList};
use crate::popup::fuzzy;
use crate::popup::Scrollable;

impl EntryFilter for CommandEntry {
    fn match_query(&self, query: &str) -> Option<Vec<usize>> {
        let haystack = format!("{} {}", self.name, self.description);
        let name_len = self.name.chars().count();
        fuzzy::fuzzy_match(&haystack, query)
            .map(|indices| indices.into_iter().filter(|&i| i < name_len).collect())
    }
}

/// Single command palette entry
#[derive(Debug, Clone)]
pub struct CommandEntry {
    pub action: Action,
    pub name: String,        // e.g. "Save"
    pub description: String, // e.g. "Save the current buffer"
    pub key_hint: String,    // e.g. ":w" or "Ctrl+S"
}

#[derive(Debug, Clone)]
pub struct CommandPalettePopup {
    pub list: FilteredList<CommandEntry>,
}

impl CommandPalettePopup {
    pub fn new(entries: Vec<CommandEntry>) -> Self {
        Self {
            list: FilteredList::new(entries),
        }
    }

    pub fn selected_entry(&self) -> Option<&CommandEntry> {
        self.list.selected_entry()
    }

    pub fn filter_push(&mut self, c: char) {
        self.list.filter_push(c);
    }

    pub fn filter_pop(&mut self) {
        self.list.filter_pop();
    }

    pub fn filter_clear(&mut self) {
        self.list.filter_clear();
    }

    pub fn move_up(&mut self) {
        self.list.move_up();
    }

    pub fn move_down(&mut self) {
        self.list.move_down();
    }
}

impl Scrollable for CommandPalettePopup {
    fn selected(&self) -> usize {
        self.list.selected()
    }
    fn selected_mut(&mut self) -> &mut usize {
        self.list.selected_mut()
    }
    fn scroll_mut(&mut self) -> &mut usize {
        self.list.scroll_mut()
    }
    fn len(&self) -> usize {
        self.list.len()
    }
    fn visible_rows(&self) -> usize {
        self.list.visible_rows()
    }
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
