// ed/health.rs
//! `:checkhealth` — diagnostic checks for dependencies and configuration.

use crate::ed::buffer::{Buffer, BufferKind};
use crate::ed::editor::Editor;
use crate::ed::mode::MessageKind;
use std::process::Command;

// ---------------------------------------------------------------------------
// Health-report builder
// ---------------------------------------------------------------------------

struct HealthBuilder {
    lines: Vec<String>,
}

impl HealthBuilder {
    fn new() -> Self {
        Self { lines: Vec::new() }
    }

    fn header(&mut self, title: &str) {
        self.lines.push(String::new());
        self.lines.push(format!("## {}", title));
        self.lines.push(String::new());
    }

    fn ok(&mut self, msg: &str) {
        self.lines.push(format!("  ✓ {}", msg));
    }

    fn error(&mut self, msg: &str) {
        self.lines.push(format!("  ✗ {}", msg));
    }

    fn warn(&mut self, msg: &str) {
        self.lines.push(format!("  ⚠ {}", msg));
    }

    fn info(&mut self, msg: &str) {
        self.lines.push(format!("    {}", msg));
    }

    fn build(self) -> String {
        self.lines.join("\n")
    }
}

// ---------------------------------------------------------------------------
// Individual checks
// ---------------------------------------------------------------------------

fn check_clipboard(hb: &mut HealthBuilder, config: &crate::config::app_config::Config) {
    hb.header("Clipboard");

    match arboard::Clipboard::new() {
        Ok(_) => {
            hb.ok("System clipboard (arboard) is available");
        }
        Err(e) => {
            hb.error(&format!("System clipboard (arboard) not available: {}", e));
            hb.info("On Linux, install xclip or xsel for X11, or check Wayland support");
            hb.info("On macOS/Windows, this should work out of the box");
        }
    }
}

fn check_git(hb: &mut HealthBuilder) {
    hb.header("Git");

    match Command::new("git").arg("--version").output() {
        Ok(output) if output.status.success() => {
            let version = String::from_utf8_lossy(&output.stdout).trim().to_string();
            hb.ok(&format!("git: {}", version));
        }
        Ok(output) => {
            let stderr = String::from_utf8_lossy(&output.stderr);
            hb.error(&format!("git error: {}", stderr.trim()));
        }
        Err(e) => {
            hb.error(&format!("git not found: {}", e));
            hb.info("Install git: https://git-scm.com/");
        }
    }

    // git2 library (bundled)
    hb.ok("git2 library: loaded (bundled with ce)");
}

fn check_ripgrep(hb: &mut HealthBuilder) {
    hb.header("Ripgrep");

    match Command::new("rg").arg("--version").output() {
        Ok(output) if output.status.success() => {
            let version = String::from_utf8_lossy(&output.stdout).trim().to_string();
            hb.ok(&format!("ripgrep: {}", version));
        }
        Ok(output) => {
            let stderr = String::from_utf8_lossy(&output.stderr);
            hb.error(&format!("ripgrep error: {}", stderr.trim()));
        }
        Err(e) => {
            hb.error(&format!("ripgrep not found: {}", e));
            hb.info("Install ripgrep: https://github.com/BurntSushi/ripgrep");
            hb.info("  brew install ripgrep | apt install ripgrep | cargo install ripgrep");
        }
    }
}

fn check_rustfmt(hb: &mut HealthBuilder) {
    hb.header("Rustfmt (optional)");

    match Command::new("rustfmt").arg("--version").output() {
        Ok(output) if output.status.success() => {
            let version = String::from_utf8_lossy(&output.stdout).trim().to_string();
            hb.ok(&format!("rustfmt: {}", version));
        }
        _ => {
            hb.warn("rustfmt not found");
            hb.info("Needed for auto-formatting .rs files on save");
            hb.info("Install with: rustup component add rustfmt");
        }
    }
}

fn check_python_formatters(hb: &mut HealthBuilder) {
    hb.header("Python Formatters (optional)");

    let ruff_found = match std::process::Command::new("ruff").arg("--version").output() {
        Ok(output) if output.status.success() => {
            let version = String::from_utf8_lossy(&output.stdout).trim().to_string();
            hb.ok(&format!("ruff: {}", version));
            true
        }
        _ => {
            hb.warn("ruff not found");
            false
        }
    };

    let black_found = match std::process::Command::new("black")
        .arg("--version")
        .output()
    {
        Ok(output) if output.status.success() => {
            let version = String::from_utf8_lossy(&output.stdout).trim().to_string();
            hb.ok(&format!("black: {}", version));
            true
        }
        _ => {
            hb.warn("black not found");
            false
        }
    };

    if !ruff_found && !black_found {
        hb.info("Install ruff or black for .py auto-formatting on save");
        hb.info("  pip install ruff  |  pip install black");
    } else if ruff_found {
        hb.info("ruff format will be used for .py files on save (preferred)");
    } else {
        hb.info("black will be used for .py files on save");
    }
}

fn check_config(hb: &mut HealthBuilder, config: &crate::config::app_config::Config) {
    hb.header("Configuration");

    match crate::config::app_config::Config::config_path() {
        Ok(path) => {
            if path.exists() {
                hb.ok(&format!("Config file: {}", path.display()));

                // Simple JSON syntax check
                match std::fs::read_to_string(&path) {
                    Ok(content) => match serde_json::from_str::<serde_json::Value>(&content) {
                        Ok(_) => hb.ok("Config file is valid JSON"),
                        Err(e) => hb.error(&format!("Config file has invalid JSON: {}", e)),
                    },
                    Err(e) => hb.error(&format!("Cannot read config file: {}", e)),
                }
            } else {
                hb.ok("Using default configuration (no config file found)");
                hb.info(&format!("Would be at: {}", path.display()));
            }
        }
        Err(e) => {
            hb.error(&format!("Cannot determine config path: {}", e));
        }
    }

    // Core settings
    hb.info(&format!("init_mode: {}", config.init_mode));
    hb.info(&format!("format_on_save: {}", config.format_on_save));
    hb.info(&format!(
        "show_startup_hints: {}",
        config.show_startup_hints
    ));
    hb.info(&format!("popup_enabled: {}", config.popup_enabled));

    // Keybindings parsed count
    hb.info("Custom keybindings parsed:");
    let kb = &config.keybindings;
    hb.info(&format!("  Normal:   {} bindings", kb.normal.len()));
    hb.info(&format!("  Insert:   {} bindings", kb.insert.len()));
    hb.info(&format!("  Brief:    {} bindings", kb.brief.len()));
    hb.info(&format!("  Command:  {} bindings", kb.command.len()));
    hb.info(&format!("  Visual:   {} bindings", kb.visual.len()));
    hb.info(&format!("  Global:   {} bindings", kb.global.len()));

    let total = kb.normal.len()
        + kb.insert.len()
        + kb.brief.len()
        + kb.command.len()
        + kb.visual.len()
        + kb.global.len();

    if total == 0 {
        hb.info("  (Using default keybindings)");
    } else {
        hb.ok(&format!("Total: {} custom bindings loaded", total));
    }
}

fn check_treesitter(hb: &mut HealthBuilder) {
    hb.header("Tree-sitter");

    hb.ok(&format!(
        "Tree-sitter ABI version: {}",
        tree_sitter::LANGUAGE_VERSION
    ));

    // Check which grammars are compiled in
    let grammars: &[(&str, fn() -> Option<tree_sitter::Language>)] = &[
        ("rust", || Some(tree_sitter_rust::LANGUAGE.into())),
        ("python", || Some(tree_sitter_python::LANGUAGE.into())),
        ("javascript / typescript", || {
            Some(tree_sitter_javascript::LANGUAGE.into())
        }),
        ("diff / patch", || Some(tree_sitter_diff::LANGUAGE.into())),
    ];

    hb.info("Compiled-in grammars:");
    for (name, get_lang) in grammars {
        if get_lang().is_some() {
            hb.ok(&format!("  {}", name));
        } else {
            hb.warn(&format!("  {} (not available)", name));
        }
    }

    hb.info("Other languages use regex / line-based highlighting");
}

fn check_llm(hb: &mut HealthBuilder, config: &crate::config::app_config::Config) {
    hb.header("LLM Assistant");

    let url = &config.llm_url;
    let port = config.llm_port;
    let addr = format!("{}:{}", url, port);

    hb.info(&format!("Configured endpoint: {}", addr));

    if config.llm_api_key.is_some() {
        hb.ok("API key is configured");
    } else {
        hb.warn("No API key configured (optional for local LLMs)");
    }

    // Try TCP connection
    use std::net::ToSocketAddrs;
    match addr.to_socket_addrs() {
        Ok(mut addrs) => {
            if let Some(socket_addr) = addrs.next() {
                match std::net::TcpStream::connect_timeout(
                    &socket_addr,
                    std::time::Duration::from_secs(2),
                ) {
                    Ok(_) => hb.ok(&format!("LLM server is reachable at {}", addr)),
                    Err(e) => {
                        hb.warn(&format!("Cannot connect to LLM server at {}: {}", addr, e));
                        hb.info("Start your LLM server, or update llm_url / llm_port in config");
                    }
                }
            } else {
                hb.warn(&format!("Cannot resolve LLM address: {}", addr));
            }
        }
        Err(e) => {
            hb.warn(&format!("Invalid LLM address '{}': {}", addr, e));
        }
    }
}

fn check_terminal(hb: &mut HealthBuilder) {
    hb.header("Terminal");

    match crossterm::terminal::size() {
        Ok((cols, rows)) => {
            hb.ok(&format!("Terminal size: {} cols × {} rows", cols, rows));
            if cols < 80 {
                hb.warn("Terminal width < 80 columns; some UI elements may clip");
            }
            if rows < 20 {
                hb.warn("Terminal height < 20 rows; consider a larger terminal");
            }
        }
        Err(e) => {
            hb.warn(&format!("Cannot determine terminal size: {}", e));
        }
    }

    hb.ok("Running in interactive terminal mode");
}

// ---------------------------------------------------------------------------
// Syntax highlighting for checkhealth output
// ---------------------------------------------------------------------------

/// Line-based styling for checkhealth buffers.
pub fn style_for_checkhealth_line(line: &str) -> Vec<Option<ratatui::style::Style>> {
    use ratatui::style::{Color, Modifier, Style};

    let chars: Vec<char> = line.chars().collect();
    let mut styles = vec![None; chars.len()];

    // Title line (══)
    if line.starts_with("══") {
        let s = Style::default()
            .fg(Color::Rgb(97, 175, 239))
            .add_modifier(Modifier::BOLD);
        styles.fill(Some(s));
        return styles;
    }

    // Footer line (──)
    if line.starts_with("──") {
        let s = Style::default().fg(Color::Rgb(92, 99, 112));
        styles.fill(Some(s));
        return styles;
    }

    // Section header (## ...)
    if line.starts_with("## ") {
        let s = Style::default()
            .fg(Color::Rgb(229, 192, 123))
            .add_modifier(Modifier::BOLD);
        styles.fill(Some(s));
        return styles;
    }

    // OK line (✓)
    if line.starts_with("  ✓") {
        if chars.len() > 2 {
            styles[2] = Some(Style::default().fg(Color::Rgb(152, 195, 121))); // ✓ in green
        }
        for i in 3..chars.len() {
            styles[i] = Some(Style::default().fg(Color::Rgb(152, 195, 121)));
        }
        return styles;
    }

    // Error line (✗)
    if line.starts_with("  ✗") {
        if chars.len() > 2 {
            styles[2] = Some(
                Style::default()
                    .fg(Color::Rgb(224, 108, 117))
                    .add_modifier(Modifier::BOLD),
            );
        }
        for i in 3..chars.len() {
            styles[i] = Some(Style::default().fg(Color::Rgb(224, 108, 117)));
        }
        return styles;
    }

    // Warning line (⚠)
    if line.starts_with("  ⚠") {
        if chars.len() > 2 {
            styles[2] = Some(Style::default().fg(Color::Rgb(229, 192, 123)));
        }
        for i in 3..chars.len() {
            styles[i] = Some(Style::default().fg(Color::Rgb(229, 192, 123)));
        }
        return styles;
    }

    // Info line (4-space indent)
    if line.starts_with("    ") {
        let s = Style::default().fg(Color::Rgb(171, 178, 191));
        styles.fill(Some(s));
        return styles;
    }

    styles
}

// ---------------------------------------------------------------------------
// Main entry point
// ---------------------------------------------------------------------------

impl Editor {
    /// Run health checks and display results in a special read-only buffer.
    pub fn open_checkhealth(&mut self) {
        let config = self.config.clone();

        let mut hb = HealthBuilder::new();

        // Title
        hb.lines.push(
            "══════════════════════════════════════════════════════════════════════════════"
                .to_string(),
        );
        hb.lines.push("  ce health check".to_string());
        hb.lines.push(
            "══════════════════════════════════════════════════════════════════════════════"
                .to_string(),
        );

        // Run all checks
        check_clipboard(&mut hb, &config);
        check_git(&mut hb);
        check_ripgrep(&mut hb);
        check_rustfmt(&mut hb);
        check_python_formatters(&mut hb);
        check_config(&mut hb, &config);
        check_treesitter(&mut hb);
        check_llm(&mut hb, &config);
        check_terminal(&mut hb);

        // Footer
        hb.lines.push(String::new());
        hb.lines.push(
            "──────────────────────────────────────────────────────────────────────────────"
                .to_string(),
        );
        hb.lines.push("  Press q to close ".to_string());

        let text = hb.build();

        // Find or create the checkhealth buffer
        let target_filename = "*checkhealth*";

        let existing_id = self
            .buffers
            .iter()
            .find(|buf| buf.filename.as_deref() == Some(target_filename))
            .map(|buf| buf.id);

        let buffer_id = if let Some(id) = existing_id {
            if let Some(buf) = self.buf_mut_by_id(id) {
                buf.rope = ropey::Rope::from_str(&text);
                buf.parse_syntax();
            }
            id
        } else {
            let id = self.next_buf_id;
            self.next_buf_id += 1;

            let buf = Buffer {
                id,
                rope: ropey::Rope::from_str(&text),
                filename: Some(target_filename.to_string()),
                modified: false,
                undo_stack: Vec::new(),
                redo_stack: Vec::new(),
                syntax: crate::ed::syntax::SyntaxState::new(),
                bookmarks: std::collections::HashSet::new(),
                git_diffs: std::collections::HashMap::new(),
                named_bookmarks: std::collections::HashMap::new(),
                kind: BufferKind::CheckHealth,
                git_log_state: None,
                git_status_state: None,
                diff_alignment: None,
                ripgrep_results: Vec::new(),
                ripgrep_line_map: Vec::new(),
                search_pattern: None,
            };

            self.buffers.push(buf);
            self.buffers.last_mut().unwrap().parse_syntax();
            id
        };

        self.switch_window_to_buffer(buffer_id);
        self.enter_normal();
        self.set_status_msg("Health check — q: close", MessageKind::Info);
    }

    /// Handle key presses in the checkhealth buffer.
    pub fn handle_checkhealth_key(&mut self, key: crossterm::event::KeyEvent) -> bool {
        match key.code {
            crossterm::event::KeyCode::Char('q') => {
                self.close_buffer();
                true
            }
            _ => false, // Fall through to normal navigation (j/k/C-d/C-u/G/gg)
        }
    }
}
