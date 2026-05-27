//! User-facing description overrides loaded from `~/.config/ce/desc.toml`.

use serde::Deserialize;
use std::collections::HashMap;
use std::path::PathBuf;

#[derive(Debug, Default, Deserialize)]
pub struct DescOverrides {
    /// Key = `Action::snake_name()` (e.g. `"move_left"`, `"save"`).
    #[serde(default)]
    pub overrides: HashMap<String, DescEntry>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct DescEntry {
    /// Override the auto-generated display name.
    #[serde(default)]
    pub name: Option<String>,
    /// Override the default description.
    #[serde(default)]
    pub description: Option<String>,
    /// Override the default key hint.
    #[serde(default)]
    pub key_hint: Option<String>,
}

impl DescOverrides {
    /// Load from disk.  Returns an empty (no-override) struct on any error
    /// so the editor always works even with a broken config.
    pub fn load() -> Self {
        let path = Self::path();
        if !path.exists() {
            return Self::default();
        }
        let content = std::fs::read_to_string(&path).unwrap_or_default();
        toml::from_str(&content).unwrap_or_else(|e| {
            log::warn!("Failed to parse desc.toml: {e}");
            Self::default()
        })
    }

    pub fn path() -> PathBuf {
        dirs::config_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("ce")
            .join("desc.toml")
    }
}
