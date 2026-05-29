// File: ./config/app_config.rs

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;

// Helper to default serialize fields to true
fn default_true() -> bool {
    true
}

fn default_false() -> bool {
    false
}

// Helper to default init_mode to "vim"
fn default_init_mode() -> String {
    "vim".to_string()
}

fn default_leader() -> String {
    "space".to_string()
}

fn default_llm_url() -> String {
    "127.0.0.1".to_string()
}

fn default_llm_port() -> u16 {
    8080
}

fn default_scroll_offset() -> usize {
    0 // Default to 0 to maintain current behavior
}

fn default_tab_size() -> usize {
    4
}

fn default_cursor_highlight_color() -> String {
    "Cyan".to_string()
}

fn default_cursor_text_color() -> String {
    "Black".to_string()
}

fn default_cursor_line_highlight() -> bool {
    false
}

fn default_cursor_line_highlight_color() -> String {
    "Rgb(40, 40, 55)".to_string()
}

// Helper to default LLM system prompt
fn default_llm_system_prompt() -> String {
    "You are a helpful, concise coding assistant inside a terminal text editor. Provide clear, accurate answers. Use markdown code blocks when providing code examples. Keep responses relatively brief.".to_string()
}

fn default_which_key_delay_ms() -> u64 {
    300
}

// ---------------------------------------------------------------------------
// Command Palette Description Overrides
// ---------------------------------------------------------------------------

/// A single override entry for a command palette action.
/// Keyed by the `Action::snake_name()` (e.g. `"move_left"`).
#[derive(Debug, Default, Serialize, Deserialize, Clone)]
pub struct DescEntry {
    /// Override the auto-generated display name.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Override the default description.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Override the default key hint.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub key_hint: Option<String>,
}

/// Overrides loaded from `~/.config/ce/desc.json`
#[derive(Debug, Default, Serialize, Deserialize, Clone)]
pub struct DescOverrides {
    #[serde(default)]
    pub overrides: HashMap<String, DescEntry>,
}

// ---------------------------------------------------------------------------
// Namespaced Keybindings Layout
// ---------------------------------------------------------------------------

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct KeybindingsConfig {
    #[serde(default)]
    pub normal: HashMap<String, String>,
    #[serde(default)]
    pub insert: HashMap<String, String>,
    #[serde(default)]
    pub brief: HashMap<String, String>,
    #[serde(default)]
    pub command: HashMap<String, String>,
    #[serde(default)]
    pub visual: HashMap<String, String>,
    #[serde(default, rename = "global")]
    pub global: HashMap<String, String>,
}

impl Default for KeybindingsConfig {
    fn default() -> Self {
        Self {
            normal: HashMap::new(),
            insert: HashMap::new(),
            brief: HashMap::new(),
            command: HashMap::new(),
            visual: HashMap::new(),
            global: HashMap::new(),
        }
    }
}

// ---------------------------------------------------------------------------
// Config
// ---------------------------------------------------------------------------

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct Config {
    pub api_key: Option<String>,
    pub api_url: String,
    pub portal_url: String,
    pub max_tokens: i32,
    pub editor_language: String,

    #[serde(default)]
    pub codeium_enabled: bool,

    // ---- LLM Subsystem Endpoint Configurations ----
    #[serde(default = "default_llm_url")]
    pub llm_url: String,
    #[serde(default = "default_llm_port")]
    pub llm_port: u16,
    pub llm_api_key: Option<String>,

    /// System prompt for LLM assistant
    #[serde(default = "default_llm_system_prompt")]
    pub llm_system_prompt: String,

    /// Whether the which-key popup is enabled. Defaults to true.
    #[serde(default = "default_true")]
    pub popup_enabled: bool,
    #[serde(default = "default_init_mode")]
    pub init_mode: String,

    // Unified namespaced keybindings
    #[serde(default)]
    pub keybindings: KeybindingsConfig,

    #[serde(default = "default_leader")]
    pub leader: String,

    // ---- Gutter Feature Toggles ----
    #[serde(default = "default_true")]
    pub line_numbers_enabled: bool,
    #[serde(default)]
    pub relative_line_numbers: bool,
    #[serde(default = "default_true")]
    pub git_gutter_enabled: bool,
    #[serde(default = "default_true")]
    pub bookmarks_enabled: bool,
    #[serde(default = "default_false")]
    pub bookmark_popup_goto: bool,
    #[serde(default)]
    pub search_wrap_enabled: bool,
    // ---- Formatting Feature Toggle ----
    #[serde(default = "default_false")]
    pub format_on_save: bool,
    #[serde(default = "default_scroll_offset")]
    pub scroll_offset: usize,
    #[serde(default = "default_true")]
    pub show_indent_guides: bool,
    #[serde(default = "default_tab_size")]
    pub tab_size: usize,
    // ---- Cursor Style ----
    #[serde(default = "default_cursor_highlight_color")]
    pub cursor_highlight_color: String,
    #[serde(default = "default_cursor_text_color")]
    pub cursor_text_color: String,
    #[serde(default = "default_cursor_line_highlight")]
    pub cursor_line_highlight: bool,
    #[serde(default = "default_cursor_line_highlight_color")]
    pub cursor_line_highlight_color: String,
    // Which-key popup debounce delay in milliseconds.
    // Set to 0 to show instantly, or a higher number to hide it during fast typing.
    #[serde(default = "default_which_key_delay_ms")]
    pub which_key_delay_ms: u64,

    #[serde(default = "default_true")]
    pub show_startup_hints: bool,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            api_key: None,
            api_url: "https://server.codeium.com".to_string(),
            portal_url: "https://codeium.com".to_string(),
            max_tokens: 256,
            editor_language: "plaintext".to_string(),
            codeium_enabled: false,
            popup_enabled: true,
            init_mode: "vim".to_string(),
            keybindings: KeybindingsConfig::default(),
            leader: "space".to_string(),

            llm_url: "127.0.0.1".to_string(),
            llm_port: 8080,
            llm_api_key: None,
            llm_system_prompt: default_llm_system_prompt(),
            cursor_highlight_color: "Cyan".to_string(),
            cursor_text_color: "Black".to_string(),
            cursor_line_highlight: false,
            cursor_line_highlight_color: "Rgb(40, 40, 55)".to_string(),
            // Gutter Defaults
            line_numbers_enabled: true,
            relative_line_numbers: true,
            git_gutter_enabled: true,
            bookmarks_enabled: true,
            bookmark_popup_goto: false,
            search_wrap_enabled: false,
            show_startup_hints: true,
            scroll_offset: 0,
            which_key_delay_ms: 300,
            show_indent_guides: true,
            tab_size: 4,
            format_on_save: false,
        }
    }
}

impl Config {
    pub fn config_dir() -> Result<PathBuf> {
        let dir = dirs::config_dir()
            .context("Could not find config directory")?
            .join("ce");

        if !dir.exists() {
            fs::create_dir_all(&dir)?;
        }
        Ok(dir)
    }

    pub fn config_path() -> Result<PathBuf> {
        Ok(Self::config_dir()?.join("config.json"))
    }

    pub fn load() -> Result<Self> {
        let path = Self::config_path()?;
        let mut config = if path.exists() {
            let content = fs::read_to_string(&path)?;
            let mut c: Config = serde_json::from_str(&content)?;
            if c.api_url.is_empty() {
                c.api_url = "https://server.codeium.com".to_string();
            }
            if c.portal_url.is_empty() {
                c.portal_url = "https://codeium.com".to_string();
            }
            if c.max_tokens == 0 {
                c.max_tokens = 256;
            }
            // Fallback to default if the system prompt was somehow loaded as empty
            if c.llm_system_prompt.is_empty() {
                c.llm_system_prompt = default_llm_system_prompt();
            }
            c
        } else {
            Config {
                api_url: "https://server.codeium.com".to_string(),
                portal_url: "https://codeium.com".to_string(),
                max_tokens: 256,
                editor_language: "plaintext".to_string(),
                codeium_enabled: false,
                popup_enabled: true,
                init_mode: "vim".to_string(),

                llm_url: "127.0.0.1".to_string(),
                llm_port: 8080,
                llm_api_key: None,
                llm_system_prompt: default_llm_system_prompt(),
                cursor_line_highlight: false,
                cursor_line_highlight_color: "Rgb(40, 40, 55)".to_string(),
                cursor_highlight_color: "Cyan".to_string(),
                cursor_text_color: "Black".to_string(),
                line_numbers_enabled: true,
                relative_line_numbers: true,
                git_gutter_enabled: true,
                search_wrap_enabled: false,
                bookmarks_enabled: true,
                show_startup_hints: true,
                scroll_offset: 0,
                which_key_delay_ms: 300,
                show_indent_guides: true,
                tab_size: 4,
                format_on_save: false,
                ..Default::default()
            }
        };

        if config.api_key.is_none() || config.api_key.as_ref().map_or(true, |k| k.is_empty()) {
            config.api_key = Self::discover_neovim_key();
            if config.api_key.is_some() {
                let _ = config.save();
            }
        }

        Ok(config)
    }

    pub fn save(&self) -> Result<()> {
        let path = Self::config_path()?;
        let content = serde_json::to_string_pretty(self)?;
        fs::write(&path, content)?;
        Ok(())
    }

    pub fn discover_neovim_key() -> Option<String> {
        if let Some(home) = dirs::home_dir() {
            let paths = vec![
                home.join(".cache")
                    .join("nvim")
                    .join("codeium")
                    .join("config.json"),
                home.join(".local")
                    .join("share")
                    .join("nvim")
                    .join("codeium")
                    .join("config.json"),
                home.join(".codeium").join("config.json"),
            ];

            for path in &paths {
                if let Some(key) = Self::read_key_from_path(path) {
                    return Some(key);
                }
            }
        }
        None
    }

    fn read_key_from_path(path: &PathBuf) -> Option<String> {
        if !path.exists() {
            return None;
        }
        let content = fs::read_to_string(path).ok()?;
        let json: serde_json::Value = serde_json::from_str(&content).ok()?;
        let key = json.get("api_key")?.as_str()?;
        if key.is_empty() {
            None
        } else {
            Some(key.to_string())
        }
    }
    /// Returns the path to the description overrides file.
    pub fn descriptions_path() -> Result<PathBuf> {
        Ok(Self::config_dir()?.join("desc.json"))
    }

    /// Load user description overrides from `~/.config/ce/desc.json`.
    /// Returns a default (empty) struct if the file doesn't exist or fails to parse,
    /// so the editor always works even with a broken override file.
    pub fn load_descriptions() -> DescOverrides {
        let path = match Self::descriptions_path() {
            Ok(p) => p,
            Err(_) => return DescOverrides::default(),
        };

        if !path.exists() {
            return DescOverrides::default();
        }

        let content = match fs::read_to_string(&path) {
            Ok(c) => c,
            Err(e) => {
                log::warn!("Failed to read desc.json: {e}");
                return DescOverrides::default();
            }
        };

        serde_json::from_str(&content).unwrap_or_else(|e| {
            log::warn!("Failed to parse desc.json: {e}");
            DescOverrides::default()
        })
    }
    /// Resolve a color name string to a ratatui Color.
    pub fn resolve_color(&self, name: &str) -> ratatui::style::Color {
        match name.trim() {
            "Black" => ratatui::style::Color::Black,
            "Red" => ratatui::style::Color::Red,
            "Green" => ratatui::style::Color::Green,
            "Yellow" => ratatui::style::Color::Yellow,
            "Blue" => ratatui::style::Color::Blue,
            "Magenta" => ratatui::style::Color::Magenta,
            "Cyan" => ratatui::style::Color::Cyan,
            "White" => ratatui::style::Color::White,
            "Gray" | "DarkGray" => ratatui::style::Color::DarkGray,
            "LightRed" => ratatui::style::Color::LightRed,
            "LightGreen" => ratatui::style::Color::LightGreen,
            "LightYellow" => ratatui::style::Color::LightYellow,
            "LightBlue" => ratatui::style::Color::LightBlue,
            "LightMagenta" => ratatui::style::Color::LightMagenta,
            "LightCyan" => ratatui::style::Color::LightCyan,
            s if s.starts_with("Rgb(") && s.ends_with(')') => {
                let inner = &s[4..s.len() - 1];
                let parts: Vec<u8> = inner
                    .split(',')
                    .filter_map(|p| p.trim().parse::<u8>().ok())
                    .collect();
                if parts.len() == 3 {
                    ratatui::style::Color::Rgb(parts[0], parts[1], parts[2])
                } else {
                    ratatui::style::Color::Cyan
                }
            }
            _ => ratatui::style::Color::Cyan,
        }
    }
}
