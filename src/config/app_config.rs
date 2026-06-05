// File: ./config/app_config.rs

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;

// ═══════════════════════════════════════════════════════════════════════════
// Serde Default Helpers
// ═══════════════════════════════════════════════════════════════════════════

/// Generates a zero-arg function returning `String` for use with
/// `#[serde(default = "...")]`.  Each invocation produces exactly one
/// public function whose name and body come from the macro arguments.
macro_rules! default_string {
    ($fn_name:ident, $value:expr) => {
        fn $fn_name() -> String {
            $value.to_string()
        }
    };
}

/// Same idea, but for non-string `Copy` types.
macro_rules! default_value {
    ($fn_name:ident, $value:expr, $ty:ty) => {
        fn $fn_name() -> $ty {
            $value
        }
    };
}

// ── String defaults ────────────────────────────────────────────────────────
default_string!(default_init_mode, "vim");
default_string!(default_leader, "space");
default_string!(default_editor_language, "plaintext");
default_string!(default_llm_url, "127.0.0.1");
default_string!(default_ollama_url, "127.0.0.1");
default_string!(default_ollama_model, "gemma4:31b-cloud");
default_string!(default_cursor_highlight_color, "Cyan");
default_string!(default_cursor_text_color, "Black");
default_string!(default_cursor_line_highlight_color, "Rgb(40, 40, 55)");
default_string!(default_mqtt_host, "localhost");
default_string!(default_mqtt_topic, "/translate");

default_string!(
    default_llm_system_prompt,
    "You are a helpful, concise coding assistant inside a terminal text editor. \
     Provide clear, accurate answers. Use markdown code blocks when providing \
     code examples. Keep responses relatively brief."
);

// ── Primitive defaults ─────────────────────────────────────────────────────
default_value!(default_true, true, bool);
default_value!(default_false, false, bool);
default_value!(default_llm_port, 8080, u16);
default_value!(default_ollama_port, 11434, u16);
default_value!(default_mqtt_port, 1883, u16);
default_value!(default_scroll_offset, 0, usize);
default_value!(default_tab_size, 4, usize);
default_value!(default_lsp_completion_min_prefix, 3, usize);
default_value!(default_which_key_delay_ms, 300, u64);
default_value!(default_completion_delay_ms, 150, u64);

// ── Enum defaults (need `Copy` impl) ───────────────────────────────────────
fn default_commit_backend() -> LlmBackend {
    LlmBackend::Llamacpp
}

fn default_copilot_style() -> CompletionStyle {
    CompletionStyle::GhostText
}

fn default_codeium_style() -> CompletionStyle {
    CompletionStyle::GhostText
}

fn default_lsp_completion_style() -> CompletionStyle {
    CompletionStyle::GhostText
}

fn default_word_completion_style() -> CompletionStyle {
    CompletionStyle::GhostText
}

// ═══════════════════════════════════════════════════════════════════════════
// CompletionStyle
// ═══════════════════════════════════════════════════════════════════════════

/// How a completion source renders its suggestions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CompletionStyle {
    /// Show as ghost / virtual text after the cursor.
    GhostText,
    /// Show as a popup menu below the cursor.
    Popup,
}

impl Default for CompletionStyle {
    fn default() -> Self {
        CompletionStyle::Popup
    }
}

impl std::fmt::Display for CompletionStyle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CompletionStyle::GhostText => write!(f, "ghost_text"),
            CompletionStyle::Popup => write!(f, "popup"),
        }
    }
}

impl std::str::FromStr for CompletionStyle {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "ghost_text" | "ghost" => Ok(CompletionStyle::GhostText),
            "popup" | "menu" => Ok(CompletionStyle::Popup),
            other => Err(format!(
                "Unknown completion style '{}'. Expected 'ghost_text' or 'popup'",
                other
            )),
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// LlmActionConfig
// ═══════════════════════════════════════════════════════════════════════════

/// A user-defined LLM action (triggered via `:llm <action_name>`).
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct LlmActionConfig {
    /// Optional system prompt override for this specific action.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub system_prompt: Option<String>,

    /// The prompt template sent to the LLM.
    /// Supports placeholders: `{selection}`, `{file}`
    pub prompt: String,

    /// Whether to automatically send the request immediately.
    /// If false, the prompt is injected into the buffer for the user to edit.
    #[serde(default = "default_true")]
    pub auto_send: bool,
}

// ═══════════════════════════════════════════════════════════════════════════
// LlmBackend
// ═══════════════════════════════════════════════════════════════════════════

/// Which local LLM server to route requests to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LlmBackend {
    Llamacpp,
    Ollama,
}

impl Default for LlmBackend {
    fn default() -> Self {
        LlmBackend::Llamacpp
    }
}

impl std::fmt::Display for LlmBackend {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LlmBackend::Llamacpp => write!(f, "llamacpp"),
            LlmBackend::Ollama => write!(f, "ollama"),
        }
    }
}

impl std::str::FromStr for LlmBackend {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "llamacpp" | "llama_cpp" | "llama.cpp" => Ok(LlmBackend::Llamacpp),
            "ollama" => Ok(LlmBackend::Ollama),
            other => Err(format!(
                "Unknown LLM backend '{}'. Expected 'llamacpp' or 'ollama'",
                other
            )),
        }
    }
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
    // ── External Service Keys ──────────────────────────────────────
    pub api_key: Option<String>,
    #[serde(default = "default_llm_url")]
    pub api_url: String,
    #[serde(default = "default_llm_url")]
    pub portal_url: String,
    #[serde(default)]
    pub max_tokens: i32,

    // ── Editor Core ────────────────────────────────────────────────
    #[serde(default = "default_editor_language")]
    pub editor_language: String,

    #[serde(default = "default_init_mode")]
    pub init_mode: String,

    #[serde(default = "default_leader")]
    pub leader: String,

    #[serde(default = "default_true")]
    pub insert_spaces: bool,

    #[serde(default = "default_tab_size")]
    pub tab_size: usize,

    #[serde(default = "default_scroll_offset")]
    pub scroll_offset: usize,

    #[serde(default = "default_false")]
    pub format_on_save: bool,

    #[serde(default = "default_false")]
    pub search_wrap_enabled: bool,

    // ── LLM Subsystem — llama.cpp ──────────────────────────────────
    #[serde(default = "default_llm_url")]
    pub llm_url: String,
    #[serde(default = "default_llm_port")]
    pub llm_port: u16,
    #[serde(default)]
    pub llm_api_key: Option<String>,

    // ── LLM Subsystem — Ollama ─────────────────────────────────────
    #[serde(default = "default_ollama_url")]
    pub ollama_url: String,
    #[serde(default = "default_ollama_port")]
    pub ollama_port: u16,
    #[serde(default = "default_ollama_model")]
    pub ollama_model: String,

    // ── Backend selection ────────────────────────────────────────────
    /// Which backend to use for general LLM chat / prompts.
    #[serde(default)]
    pub llm_backend: LlmBackend,

    /// Which backend to use for git commit message generation.
    /// Defaults to whatever `llm_backend` is set to.
    #[serde(default = "default_commit_backend")]
    pub commit_backend: LlmBackend,

    /// System prompt for LLM assistant
    #[serde(default = "default_llm_system_prompt")]
    pub llm_system_prompt: String,
    #[serde(default)]
    pub commit_system_prompt: Option<String>,
    #[serde(default)]
    pub llm_actions: HashMap<String, LlmActionConfig>,

    // ── Completion Sources ─────────────────────────────────────────
    #[serde(default)]
    pub copilot_enabled: bool,

    #[serde(default)]
    pub codeium_enabled: bool,

    /// Completely disable the LSP subsystem. No language server will be
    /// started, and no LSP requests (diagnostics, completion,
    /// goto-definition, formatting, etc.) will be sent.
    #[serde(default = "default_false")]
    pub lsp_enabled: bool,

    /// Enable LSP completion requests. Other LSP features (diagnostics,
    /// goto-definition, signature help, formatting) remain active.
    #[serde(default = "default_false")]
    pub lsp_completion_enabled: bool,

    /// Enable scanning the current buffer for completion candidates.
    #[serde(default = "default_false")]
    pub buffer_word_scan: bool,

    /// Enable loading the user wordlist file (~/.config/ce/wordlist.txt).
    #[serde(default = "default_true")]
    pub vocab_wordlist: bool,

    // ── Completion Display & Behavior ──────────────────────────────
    /// How Copilot inline suggestions are rendered.
    #[serde(default = "default_copilot_style")]
    pub copilot_style: CompletionStyle,

    /// How Codeium inline suggestions are rendered.
    #[serde(default = "default_codeium_style")]
    pub codeium_style: CompletionStyle,

    /// How LSP completions are rendered.
    #[serde(default = "default_lsp_completion_style")]
    pub lsp_completion_style: CompletionStyle,

    /// How word-based completions (buffer words, vocab, file paths) are rendered.
    #[serde(default = "default_word_completion_style")]
    pub word_completion_style: CompletionStyle,

    /// Debounce delay in milliseconds before showing completions after
    /// the user stops typing.  Set to 0 for instant display.
    #[serde(default = "default_completion_delay_ms")]
    pub completion_delay_ms: u64,

    /// When true, only show LSP completions that start with the typed
    /// prefix. When false, show all LSP results (fuzzy-like).
    /// Member access (`foo.bar`) and scope resolution (`Type::func`)
    /// automatically bypass this filter.
    #[serde(default = "default_true")]
    pub lsp_comp_strict_prefix: bool,

    /// Minimum prefix length before sending LSP completion requests.
    /// Special triggers (`.`, `::`, `->`) bypass this limit.
    /// Set to 0 to always send requests.
    #[serde(default = "default_lsp_completion_min_prefix")]
    pub lsp_completion_min_prefix: usize,

    // ── Gutter & Visual ────────────────────────────────────────────
    #[serde(default = "default_true")]
    pub line_numbers_enabled: bool,

    #[serde(default = "default_true")]
    pub relative_line_numbers: bool,
    #[serde(default = "default_true")]
    pub git_gutter_enabled: bool,
    #[serde(default = "default_true")]
    pub bookmarks_enabled: bool,
    #[serde(default = "default_false")]
    pub bookmark_popup_goto: bool,

    #[serde(default = "default_true")]
    pub show_indent_guides: bool,

    // ── Cursor Style ───────────────────────────────────────────────
    #[serde(default = "default_cursor_highlight_color")]
    pub cursor_highlight_color: String,
    #[serde(default = "default_cursor_text_color")]
    pub cursor_text_color: String,

    #[serde(default = "default_false")]
    pub cursor_line_highlight: bool,
    #[serde(default = "default_cursor_line_highlight_color")]
    pub cursor_line_highlight_color: String,

    // ── UI / Popups ────────────────────────────────────────────────
    /// Whether the which-key popup is enabled.
    #[serde(default = "default_true")]
    pub popup_enabled: bool,

    /// Which-key popup debounce delay in milliseconds.
    /// Set to 0 to show instantly, or a higher number to hide during fast typing.
    #[serde(default = "default_which_key_delay_ms")]
    pub which_key_delay_ms: u64,

    #[serde(default = "default_true")]
    pub show_startup_hints: bool,

    // ── Keybindings ────────────────────────────────────────────────
    #[serde(default)]
    pub keybindings: KeybindingsConfig,
    // ── MQTT Integration ──────────────────────────────────────────
    /// Enable MQTT subscriber for CodeLlm auto-input.
    /// Disabled by default — must be explicitly turned on in config.
    #[serde(default = "default_false")]
    pub mqtt_enabled: bool,

    /// MQTT broker hostname.
    #[serde(default = "default_mqtt_host")]
    pub mqtt_host: String,

    /// MQTT broker port.
    #[serde(default = "default_mqtt_port")]
    pub mqtt_port: u16,

    /// MQTT topic to subscribe to.
    #[serde(default = "default_mqtt_topic")]
    pub mqtt_topic: String,

    /// When true, MQTT messages are auto-sent to the LLM
    /// instead of just inserted into the prompt area.
    #[serde(default = "default_false")]
    pub mqtt_auto_send: bool,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            api_key: None,
            api_url: "https://server.codeium.com".to_string(),
            portal_url: "https://codeium.com".to_string(),
            max_tokens: 256,

            editor_language: default_editor_language(),
            init_mode: default_init_mode(),
            leader: default_leader(),
            insert_spaces: true,
            tab_size: default_tab_size(),
            scroll_offset: default_scroll_offset(),
            format_on_save: false,
            search_wrap_enabled: false,

            llm_url: default_llm_url(),
            llm_port: default_llm_port(),
            llm_api_key: None,

            ollama_url: default_ollama_url(),
            ollama_port: default_ollama_port(),
            ollama_model: default_ollama_model(),

            llm_backend: LlmBackend::default(),
            commit_backend: default_commit_backend(),

            llm_system_prompt: default_llm_system_prompt(),
            commit_system_prompt: None,
            llm_actions: HashMap::new(),

            copilot_enabled: false,
            codeium_enabled: false,
            lsp_enabled: false,
            lsp_completion_enabled: false,
            buffer_word_scan: false,
            vocab_wordlist: true,
            word_completion_style: default_word_completion_style(),

            copilot_style: default_copilot_style(),
            codeium_style: default_codeium_style(),
            lsp_completion_style: default_lsp_completion_style(),
            completion_delay_ms: default_completion_delay_ms(),
            lsp_comp_strict_prefix: true,
            lsp_completion_min_prefix: default_lsp_completion_min_prefix(),

            line_numbers_enabled: true,
            relative_line_numbers: true,
            git_gutter_enabled: true,
            bookmarks_enabled: true,
            bookmark_popup_goto: false,
            show_indent_guides: true,

            cursor_highlight_color: default_cursor_highlight_color(),
            cursor_text_color: default_cursor_text_color(),
            cursor_line_highlight: false,
            cursor_line_highlight_color: default_cursor_line_highlight_color(),

            popup_enabled: true,
            which_key_delay_ms: default_which_key_delay_ms(),
            show_startup_hints: true,

            mqtt_enabled: false,
            mqtt_host: default_mqtt_host(),
            mqtt_port: default_mqtt_port(),
            mqtt_topic: default_mqtt_topic(),
            mqtt_auto_send: false,

            //-- default config  (anchor dont removed) --//
            keybindings: KeybindingsConfig::default(),
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
            if c.llm_system_prompt.is_empty() {
                c.llm_system_prompt = default_llm_system_prompt();
            }
            c
        } else {
            Config::default()
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

    pub fn descriptions_path() -> Result<PathBuf> {
        Ok(Self::config_dir()?.join("desc.json"))
    }

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
