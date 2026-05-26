//! codeium-editor — Mini vim buffer editor with AI ghost-text completions.
//!
//! Module layout:
//!
//! - `ed`      — Editor core (buffer, movement, editing, mode, extension trait)
//! - `config`  — Persistent configuration
//! - `keybind` — Action enum & modalkit keybinding machine
//! - `comp`    — Completion state machine & provider trait
//! - `ai`      — AI backends (Codeium cloud + local LSP)
//! - `render`  — Ratatui UI rendering
//! - `popup`   — Popup menu system (stub)
//! - `repl`    — Command-line (`:`) execution

mod ai;
mod comp;
mod config;
mod ed;
mod git;
mod keybind;
mod popup;
mod render;
mod repl;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use crossterm::{
    event::{self, DisableBracketedPaste, EnableBracketedPaste, Event, KeyEventKind},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use std::io;

use config::Config;
use ed::Editor;

// ---------------------------------------------------------------------------
// Shared server handle type alias (reduces repetition in signatures)
// ---------------------------------------------------------------------------

type ServerCell =
    std::sync::Arc<std::sync::RwLock<Option<std::sync::Arc<ai::codeium::CodeiumServer>>>>;

// ---------------------------------------------------------------------------
// AppMessage — internal event bus
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub enum AppMessage {
    Input(crossterm::event::KeyEvent),
    Paste(String),
    /// `(request_id, items)` — items are already prefix-trimmed.
    /// Always sent, even on error (empty vec so the machine can reset).
    CompletionResponse(usize, Vec<String>),
    Tick,
}

// ---------------------------------------------------------------------------
// CLI
// ---------------------------------------------------------------------------

#[derive(Parser)]
#[command(name = "codeium-editor")]
#[command(
    version,
    about = "Mini vim buffer editor with Codeium AI ghost-text completions and multi-buffer support"
)]
#[command(args_conflicts_with_subcommands = true)]
struct Cli {
    /// File path(s) to open (each becomes a separate buffer).
    path: Vec<String>,

    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Subcommand)]
enum Commands {
    /// Check if the language server binary is discoverable.
    Status,
    /// Authenticate with Codeium to obtain an API key.
    Auth,
}

// ---------------------------------------------------------------------------
// Entry point
// ---------------------------------------------------------------------------

#[tokio::main]
async fn main() -> Result<()> {
    env_logger::init();
    log::debug!("--- APPLICATION START ---");

    let cli = Cli::parse();
    match cli.command {
        Some(Commands::Status) => cmd_status(),
        Some(Commands::Auth) => cmd_auth().await,
        None => {
            let first = cli.path.first().cloned();
            cmd_edit(first, cli.path).await
        }
    }
}

// ---------------------------------------------------------------------------
// Subcommands
// ---------------------------------------------------------------------------

async fn cmd_auth() -> Result<()> {
    log::debug!("Auth subcommand");
    let mut config = Config::load().context("Failed to load config")?;
    let cert_handler = ai::codeium::CertHandler::new().context("Failed to init CertHandler")?;
    let auth_manager = ai::codeium::AuthManager::new(config.clone(), &cert_handler)?;
    let api_key = auth_manager.login_flow().await?;
    config.api_key = Some(api_key);
    config.save().context("Failed to save config")?;
    println!("\nKey successfully updated and saved locally!");
    Ok(())
}

fn cmd_status() -> Result<()> {
    log::debug!("Status subcommand");
    match ai::codeium::CodeiumServer::find_binary() {
        Ok(bin) => {
            let config = Config::load()?;
            let key_status = if config.api_key.is_some() {
                "OK"
            } else {
                "MISSING"
            };
            println!("Binary : {:?}", bin);
            println!("API key: {}", key_status);
        }
        Err(e) => {
            eprintln!("Error: {}", e);
            std::process::exit(1);
        }
    }
    Ok(())
}

async fn cmd_edit(first_file: Option<String>, all_paths: Vec<String>) -> Result<()> {
    log::debug!("Edit flow — files: {:?}", all_paths);

    let config = Config::load().context("Failed to load config")?;

    let api_key = if config.codeium_enabled {
        let key = config
            .api_key
            .as_ref()
            .context(
                "No API key found and Codeium is enabled.\n\
             Please authenticate first:\n  \
               1. Run: cargo run -- auth\n  \
               2. Or set the key manually in ~/.config/codeium-cli/config.json\n\
             Alternatively, set \"codeium_enabled\": false in your config.",
            )?
            .clone();
        Some(key)
    } else {
        None
    };

    let mut editor = Editor::new(first_file)?;
    for extra in all_paths.iter().skip(1) {
        editor.open_buffer(Some(extra.clone()));
    }

    // Terminal setup - Enabling raw mode, alternate screen, and bracketed paste mode
    enable_raw_mode().context("Failed to enable raw mode")?;
    let mut stdout = io::stdout().lock();
    execute!(stdout, EnterAlternateScreen, EnableBracketedPaste)
        .context("Failed to enter alternate screen")?;
    let backend = ratatui::backend::CrosstermBackend::new(stdout);
    let mut term = ratatui::Terminal::new(backend).context("Failed to create terminal")?;

    // Shared Codeium server cell — populated by a background task.
    let server_cell: ServerCell = std::sync::Arc::new(std::sync::RwLock::new(None));

    if let Some(key) = api_key {
        let cell = server_cell.clone();
        tokio::spawn(async move {
            log::debug!("Starting Codeium server in background…");
            match ai::codeium::CodeiumServer::new(key).await {
                Ok(srv) => {
                    log::debug!("Codeium server ready on port {}", srv.port());
                    if let Ok(mut g) = cell.write() {
                        *g = Some(std::sync::Arc::new(srv));
                    }
                }
                Err(e) => log::error!("Failed to start Codeium server: {:?}", e),
            }
        });
    } else {
        log::debug!("Codeium disabled — skipping server start.");
    }

    let result = run_loop(&mut term, &mut editor, server_cell).await;

    // Terminal teardown - Disabling raw mode, leaving screen, and turning off bracketed paste mode
    disable_raw_mode().context("Failed to disable raw mode")?;
    execute!(io::stdout(), LeaveAlternateScreen, DisableBracketedPaste)
        .context("Failed to leave alternate screen")?;
    result
}

// ---------------------------------------------------------------------------
// Completion providers
// ---------------------------------------------------------------------------

/// Run local word-completion in a background task.
async fn local_complete(
    vocab: Vec<String>,
    buf_words: Vec<String>,
    current_line: String,
    prefix: String,
) -> Vec<String> {
    if prefix.is_empty() {
        return Vec::new();
    }

    let mut words = std::collections::HashSet::new();
    for w in vocab {
        if w.len() >= 4 {
            words.insert(w);
        }
    }
    for w in buf_words {
        words.insert(w);
    }
    for w in current_line.split(|c: char| !c.is_alphanumeric() && c != '_') {
        if w.len() >= 6 {
            words.insert(w.to_string());
        }
    }

    let mut out: Vec<String> = words
        .into_iter()
        .filter(|w| w.starts_with(&prefix) && w.as_str() != prefix)
        .collect();
    out.sort();
    out
}

/// Spawn whichever completion provider is appropriate and send the result
/// back as a `CompletionResponse`.  Always sends — even on error — so the
/// machine can exit `Pending` state.
fn spawn_completion(
    id: usize,
    text: String,
    offset: usize,
    lang: String,
    editor: &Editor,
    server_cell: &ServerCell,
    tx: tokio::sync::mpsc::Sender<AppMessage>,
) {
    let server_opt = if editor.config.codeium_enabled {
        server_cell.read().ok().and_then(|g| g.clone())
    } else {
        None
    };

    if let Some(server) = server_opt {
        // --- Codeium cloud ---
        tokio::spawn(async move {
            let items = server
                .fetch_completion_items(&text, offset, &lang)
                .await
                .unwrap_or_else(|e| {
                    log::debug!("Codeium error: {:?}", e);
                    Vec::new()
                });
            let _ = tx.send(AppMessage::CompletionResponse(id, items)).await;
        });
    } else {
        // --- Local word completion ---
        let vocab = editor.vocab_words.iter().cloned().collect::<Vec<_>>();
        let cached = editor.buffer_words.clone();
        let line = editor.get_current_line_text();
        let prefix = editor.get_current_word_prefix();

        tokio::spawn(async move {
            let items = local_complete(vocab, cached, line, prefix).await;
            let _ = tx.send(AppMessage::CompletionResponse(id, items)).await;
        });
    }
}

// ---------------------------------------------------------------------------
// Atomic Paste Handler (Exactly One Undo Snapshot)
// ---------------------------------------------------------------------------

fn handle_paste_event(editor: &mut Editor, text: String) {
    if text.is_empty() {
        return;
    }
    match editor.mode() {
        ed::Mode::Command | ed::Mode::Search => {
            // Append directly to the command line input
            for ch in text.chars() {
                editor.push_command(ch);
            }
            editor.comp.on_edit();
        }
        _ => {
            // General buffer paste
            let (win, buf) = editor.active_window_and_buf_mut();
            let (row, col) = (win.row, win.col);

            // Save EXACTLY one undo snapshot covering the entire paste payload
            buf.push_undo(row, col);

            // Perform the multi-character text insertion
            crate::ed::editing::paste_text(win, buf, &text);

            // Sync syntax and completions
            buf.parse_syntax();
            editor.comp.on_edit();
            editor.refresh_buffer_words();
        }
    }
}

// ---------------------------------------------------------------------------
// Main event loop
// ---------------------------------------------------------------------------

async fn run_loop(
    terminal: &mut ratatui::Terminal<ratatui::backend::CrosstermBackend<io::StdoutLock<'_>>>,
    editor: &mut Editor,
    server_cell: ServerCell,
) -> Result<()> {
    let (tx, mut rx) = tokio::sync::mpsc::channel::<AppMessage>(100);

    // Blocking input reader thread - updated to poll and read Paste events
    {
        let tx = tx.clone();
        std::thread::spawn(move || loop {
            match event::poll(std::time::Duration::from_millis(10)) {
                Ok(true) => match event::read() {
                    Ok(Event::Key(key)) => {
                        if tx.blocking_send(AppMessage::Input(key)).is_err() {
                            break;
                        }
                    }
                    Ok(Event::Paste(data)) => {
                        if tx.blocking_send(AppMessage::Paste(data)).is_err() {
                            break;
                        }
                    }
                    _ => {}
                },
                Ok(false) => {}
                Err(e) => {
                    log::error!("Input poll error: {:?}", e);
                    break;
                }
            }
        });
    }

    // Tick timer — drives spinner + completion throttle checks
    {
        let tx = tx.clone();
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(std::time::Duration::from_millis(50));
            loop {
                interval.tick().await;
                if tx.send(AppMessage::Tick).await.is_err() {
                    break;
                }
            }
        });
    }

    while let Some(msg) = rx.recv().await {
        match msg {
            // ---------------------------------------------------------------
            // Keyboard input
            // ---------------------------------------------------------------
            AppMessage::Input(key) => {
                if key.kind != KeyEventKind::Press {
                    continue;
                }

                editor.handle_key(key);

                if editor.should_quit() {
                    log::debug!("Editor quit requested.");
                    break;
                }
            }

            // ---------------------------------------------------------------
            // Terminal Paste Event
            // ---------------------------------------------------------------
            AppMessage::Paste(data) => {
                handle_paste_event(editor, data);
            }

            // ---------------------------------------------------------------
            // Completion response — one line; machine handles ID check & phase
            // ---------------------------------------------------------------
            AppMessage::CompletionResponse(id, items) => {
                editor.ingest_completion_response(id, items);
            }

            // ---------------------------------------------------------------
            // Tick — three numbered steps, each a single call
            // ---------------------------------------------------------------
            AppMessage::Tick => {
                // 1. LSP loading indicator
                let server_ready = !editor.config.codeium_enabled
                    || server_cell.read().map(|g| g.is_some()).unwrap_or(false);
                editor.set_lsp_loading(!server_ready);
                editor.tick_spinner();

                // 2. Ask the machine if a request should fire.
                //    Poll via Editor method so the borrow-checker sees comp and
                //    buffers as disjoint fields rather than one &mut self borrow.
                if let Some((id, text, offset, lang)) = editor.poll_completion() {
                    spawn_completion(id, text, offset, lang, editor, &server_cell, tx.clone());
                }

                // 3. Poll git debounce timer and background diff results
                editor.run_git_tasks();

                // 4. Poll background LLM task responses
                editor.poll_llm_responses();

                // 5. Animate the git commit generation buffer (if currently active)
                editor.tick_git_commit();

                // 6. Animate general LLM prompt spinner (if currently active)
                editor.tick_llm_prompt();
            }
        }

        // Scroll + render after every message
        let viewport = terminal.size()?.height.saturating_sub(3) as usize;
        editor.ensure_cursor_visible(viewport);
        terminal.draw(|f| render::draw(f, editor))?;
    }

    Ok(())
}
