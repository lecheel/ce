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

// Helper to default LLM system prompt
fn default_llm_system_prompt() -> String {
    "You are a helpful, concise coding assistant inside a terminal text editor. Provide clear, accurate answers. Use markdown code blocks when providing code examples. Keep responses relatively brief.".to_string()
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
    #[serde(default)]
    pub search_wrap_enabled: bool,
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

            // Gutter Defaults
            line_numbers_enabled: true,
            relative_line_numbers: true,
            git_gutter_enabled: true,
            bookmarks_enabled: true,
            search_wrap_enabled: false,
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

                line_numbers_enabled: true,
                relative_line_numbers: true,
                git_gutter_enabled: true,
                search_wrap_enabled: false,
                bookmarks_enabled: true,
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
}
