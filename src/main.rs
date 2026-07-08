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
mod file_lang;
mod git;
mod keybind;
pub mod lsp;
mod msgbox;
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

use crate::ai::copilot::start_copilot_server_task;
use crate::ai::copilot::CopilotServerCell;
use config::Config;
use ed::Editor;
use std::io;

// ---------------------------------------------------------------------------
// Shared server handle type alias (reduces repetition in signatures)
// ---------------------------------------------------------------------------

type ServerCell =
    std::sync::Arc<std::sync::RwLock<Option<std::sync::Arc<ai::codeium::CodeiumServer>>>>;

// ---------------------------------------------------------------------------
// AppMessage — internal event bus for the main loop
// ---------------------------------------------------------------------------
// NOTE: This is the *main-loop* message enum. The LSP task uses its own
// `msgbox::AppMessage` for LSP-specific responses, which is drained
// separately via `editor.poll_lsp_responses()`.

#[derive(Debug, Clone)]
pub enum AppMessage {
    Input(crossterm::event::KeyEvent),
    Paste(String),
    /// `(request_id, items)` — items are already prefix-trimmed.
    /// Always sent, even on error (empty vec so the machine can reset).
    CompletionResponse(usize, Vec<String>, u64),
    Tick,
}

// ---------------------------------------------------------------------------
// CLI
// ---------------------------------------------------------------------------

#[derive(Parser)]
#[command(name = "ce")]
#[command(
    version,
    about = "Mini vim buffer editor with Codeium AI ghost-text completions and multi-buffer support"
)]
#[command(args_conflicts_with_subcommands = true)]
struct Cli {
    path: Vec<String>,
    #[arg(long, value_name = "FILTER")]
    fd: Option<String>,
    #[arg(long, num_args = 1.., value_name = "PATTERN")]
    rg: Option<Vec<String>>,
    #[arg(long, alias = "st", help = "Load modified files from git status")]
    gs: bool,
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
            let fd_filter = cli.fd;
            let rg_pattern = cli.rg.map(|parts| parts.join(" "));
            cmd_edit(cli.path, fd_filter, rg_pattern, cli.gs).await
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

async fn cmd_edit(
    all_args: Vec<String>,
    initial_fd_filter: Option<String>,
    initial_rg_pattern: Option<String>,
    load_git_status_files: bool,
) -> Result<()> {
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

    // ── Parse Vim-style +N arguments (e.g. ./ce +5 main.rs) ──────────
    let mut initial_line: Option<usize> = None;
    let mut files = Vec::new();
    for arg in all_args {
        if let Some(num_str) = arg.strip_prefix('+') {
            if num_str.is_empty() {
                initial_line = Some(usize::MAX);
            } else if let Ok(num) = num_str.parse::<usize>() {
                initial_line = Some(num);
            }
        } else {
            files.push(arg);
        }
    }

    if load_git_status_files {
        let start_dir = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
        if let Some(root) = crate::git::gutter::find_git_root(&start_dir) {
            if let Some((state, _)) = crate::git::status::GitStatusState::load(&root) {
                for file in state.files {
                    if file.status != crate::git::status::FileStatusType::Untracked
                        && file.status != crate::git::status::FileStatusType::Deleted
                    {
                        let full_path = root.join(&file.path);
                        files.push(full_path.to_string_lossy().to_string());
                    }
                }
            }
        }
    }

    let first_file = files.first().cloned();
    let mut editor = Editor::new(first_file)?;

    // ── Apply +N line override ────────────────────────────────────────
    if let Some(line) = initial_line {
        let row = line.saturating_sub(1);
        let (win, buf) = editor.active_window_and_buf_mut();
        win.row = row.min(buf.len_lines().saturating_sub(1));
        win.col = 0;
        win.desired_col = 0;
    }

    // Open any additional files specified after the first
    for extra in files.iter().skip(1) {
        editor.open_buffer(Some(extra.clone()));
    }

    // Terminal setup
    enable_raw_mode().context("Failed to enable raw mode")?;
    let mut stdout = io::stdout().lock();
    execute!(stdout, EnterAlternateScreen, EnableBracketedPaste)
        .context("Failed to enter alternate screen")?;
    let backend = ratatui::backend::CrosstermBackend::new(stdout);
    let mut term = ratatui::Terminal::new(backend).context("Failed to create terminal")?;

    // Shared Codeium server cell — populated by a background task.
    let server_cell: ServerCell = std::sync::Arc::new(std::sync::RwLock::new(None));

    // ── Shared Copilot server cell ────────────────────────────────
    let copilot_server_cell: CopilotServerCell = std::sync::Arc::new(std::sync::RwLock::new(None));

    // ── Start Codeium server (if enabled) ─────────────────────────
    if let Some(key) = api_key {
        let cell = server_cell.clone();
        tokio::spawn(async move {
            match ai::codeium::CodeiumServer::new(key).await {
                Ok(srv) => {
                    if let Ok(mut g) = cell.write() {
                        *g = Some(std::sync::Arc::new(srv));
                    }
                }
                Err(e) => log::error!("Failed to start Codeium server: {:?}", e),
            }
        });
    }

    // ── Start Copilot server (if enabled and token available) ─────
    if config.copilot_enabled {
        let copilot_token = crate::ai::copilot::auth::AuthManager::discover_local_token()
            .map(|(token, _path)| token);

        match copilot_token {
            Some(token) => {
                let (handle, resp_rx) = start_copilot_server_task(token);
                editor.copilot_handle = Some(handle);
                editor.copilot_response_rx = Some(resp_rx);
            }
            None => {}
        }
    }

    // ═══════════════════════════════════════════════════════════════════
    // Open fd popup with initial filter if requested via --fd
    // ═══════════════════════════════════════════════════════════════════
    if let Some(filter) = initial_fd_filter {
        let root = crate::git::gutter::find_git_root(&std::path::PathBuf::from(
            editor.active_filename().unwrap_or("."),
        ))
        .unwrap_or_else(|| std::path::PathBuf::from("."));

        if root.is_dir() {
            editor.popup.open_fd(&root, &filter);
        }
    }

    // ═══════════════════════════════════════════════════════════════════
    // Run ripgrep search and open quickfix popup if requested via --rg
    // ═══════════════════════════════════════════════════════════════════
    if let Some(pattern) = initial_rg_pattern {
        if pattern.is_empty() {
            editor.ripgrep_last();
        } else {
            editor.ripgrep_search(&pattern);
        }

        // Open quickfix popup to display results
        /*
        if !editor.quickfix_results.is_empty() {
            editor.open_quickfix_popup();
        } else {
            editor.set_status_msg(
                &format!("No results found for: {}", pattern),
                crate::ed::mode::MessageKind::Info,
            );
        }
        */
    }

    let result = run_loop(&mut term, &mut editor, server_cell).await;

    // ── Shutdown LSP before terminal teardown ────────────────────────
    editor.lsp_shutdown();

    // Terminal teardown
    disable_raw_mode().context("Failed to disable raw mode")?;
    execute!(io::stdout(), LeaveAlternateScreen, DisableBracketedPaste)
        .context("Failed to leave alternate screen")?;
    result
}

// ---------------------------------------------------------------------------
// Completion providers
// ---------------------------------------------------------------------------

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

fn spawn_completion(
    id: usize,
    text: String,
    offset: usize,
    lang: String,
    editor: &Editor,
    server_cell: &ServerCell,
    tx: tokio::sync::mpsc::Sender<AppMessage>,
    version: u64, // Added version
) {
    log::warn!(
        "[TRACE-SPAWN] Requesting completion id={}, lang={}, server_available={}",
        id,
        lang,
        server_cell.read().map(|g| g.is_some()).unwrap_or(false)
    );
    let server_opt = if editor.config.codeium_enabled {
        server_cell.read().ok().and_then(|g| g.clone())
    } else {
        None
    };

    if let Some(server) = server_opt {
        tokio::spawn(async move {
            let items = server
                .fetch_completion_items(&text, offset, &lang)
                .await
                .unwrap_or_else(|e| {
                    log::debug!("Codeium error: {:?}", e);
                    Vec::new()
                });
            let _ = tx
                .send(AppMessage::CompletionResponse(id, items, version))
                .await;
        });
    } else {
        // ... local complete fallback ...
        let vocab = editor.vocab_words.iter().cloned().collect::<Vec<_>>();
        let cached = editor.buffer_words.clone();
        let line = editor.get_current_line_text();
        let prefix = editor.get_current_word_prefix();

        tokio::spawn(async move {
            let items = local_complete(vocab, cached, line, prefix).await;
            let _ = tx
                .send(AppMessage::CompletionResponse(id, items, version))
                .await;
        });
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

    // Blocking input reader thread
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

    let mut needs_redraw = true;
    while let Some(msg) = rx.recv().await {
        match msg {
            AppMessage::Input(key) => {
                if key.kind != KeyEventKind::Press {
                    continue;
                }
                if matches!(key.code, crossterm::event::KeyCode::Modifier(_)) {
                    editor.handle_key(key);
                    continue;
                }
                editor.handle_key(key);
                needs_redraw = true;
                if editor.should_quit() {
                    break;
                }
            }

            AppMessage::Paste(data) => {
                editor.handle_paste(&data);
                needs_redraw = true;
            }

            AppMessage::CompletionResponse(id, items, version) => {
                editor.ingest_completion_response(id, items, version);
                needs_redraw = true;
            }

            AppMessage::Tick => {
                // 1. LSP loading indicator (Codeium)
                let server_ready = !editor.config.codeium_enabled
                    || server_cell.read().map(|g| g.is_some()).unwrap_or(false);
                editor.set_lsp_loading(!server_ready);
                editor.tick_spinner();

                // 2. Ask the completion machine if a request should fire (Codeium/Copilot idle-debounce)
                if let Some((id, text, offset, lang, version)) = editor.poll_completion() {
                    spawn_completion(
                        id,
                        text,
                        offset,
                        lang,
                        editor,
                        &server_cell,
                        tx.clone(),
                        version,
                    );
                }

                // 3. Poll debounced LSP completion request (throttle_ms)
                editor.poll_completion_lsp();

                // 4. Poll LSP responses from the background LspManager task
                //    (diagnostics, inlay hints, signature help, completions, formatting)
                editor.poll_lsp_responses();
                editor.poll_copilot_auth();
                editor.poll_copilot_completions();

                // 5. Poll git debounce timer and background diff results
                editor.run_git_tasks();

                // 6. Poll background LLM task responses
                editor.poll_llm_responses();

                // 7. Animate the git commit generation buffer
                editor.tick_git_commit();

                // 8. Animate general LLM prompt spinner
                editor.tick_llm_prompt();

                // 9. Build
                editor.tick_build();

                // 10. Trigger redraw if which-key debounce just elapsed
                if editor.is_whichkey_visible() {
                    needs_redraw = true;
                }

                // 11. Poll MQTT subscriber for incoming messages
                editor.poll_mqtt();
            }
        }

        // Scroll + render after every message
        let viewport = terminal.size()?.height.saturating_sub(3) as usize;
        editor.ensure_cursor_visible(viewport);
        terminal.draw(|f| render::draw(f, editor))?;
    }

    Ok(())
}
