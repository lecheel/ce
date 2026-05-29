//! Command-line (REPL) execution.
//!
//! The `execute` function parses and runs `:` commands entered by the user.
use crate::ed::mode::MessageKind;
use crate::ed::repeat::{RepeatExt, RepeatableAction};
use crate::Config;
use std::path::Path;

// ---------------------------------------------------------------------------
// Command execution (free function)
// ---------------------------------------------------------------------------

pub fn execute(editor: &mut crate::ed::editor::Editor, cmd: &str) {
    let cmd = cmd.trim();
    if cmd.is_empty() {
        return;
    }

    match cmd {
        // ---- Mode Switching ----
        "vim" | "normal" => {
            editor.enter_normal();
            editor.set_status_msg(
                "Switched to Vim (Normal) mode, gi to brief mode",
                MessageKind::Info,
            );
        }
        "brief" => {
            editor.enter_brief();
            editor.set_status_msg(
                "Switched to Brief mode, F9 :vim to vim mode",
                MessageKind::Info,
            );
        }

        // ---- Info / Diagnostics ----
        "ls" | "buffers" | "files" => {
            editor.trigger_buffer_list_popup();
        }
        "config" => {
            editor.open_config_popup();
        }
        "scankey" => {
            editor.open_scankey_popup();
        }
        "checkhealth" | "health" => {
            editor.open_checkhealth();
        }
        // ---- Guide Commands ----
        "guide" => {
            editor.open_guide_popup();
        }
        s if s.starts_with("guide ") => {
            let arg = s.strip_prefix("guide ").unwrap().trim();
            match arg {
                "update" | "sync" => {
                    let filename = editor.active_filename().map(|s| s.to_string());
                    if let Some(name) = filename {
                        let source = editor.buf().rope.to_string();
                        let mut guide = crate::ed::guide::Guide::load();
                        match guide.sync_from_buffer(std::path::Path::new(&name), &source) {
                            Ok(result) => {
                                editor.set_status_msg(
                                    &format!("Guide synced: +{} ~{}", result.added, result.updated),
                                    MessageKind::Success,
                                );
                            }
                            Err(e) => {
                                editor.set_status_msg(
                                    &format!("Guide sync failed: {}", e),
                                    MessageKind::Error,
                                );
                            }
                        }
                    } else {
                        editor.set_status_msg(
                            "Cannot sync guide: unsaved buffer",
                            MessageKind::Error,
                        );
                    }
                }
                _ => {
                    editor.set_status_msg(
                        &format!("Unknown guide argument: {}", arg),
                        MessageKind::Error,
                    );
                }
            }
        }
        "command_palette" => {
            let entries = crate::popup::command_palette::build_command_entries();
            editor.popup.open_command_palette(entries);
        }
        s if s.starts_with("vocab ") => {
            let word = s.strip_prefix("vocab ").unwrap().trim();
            if !word.is_empty() {
                match editor.add_vocab_word(word) {
                    Ok(_) => {
                        editor.set_status_msg(
                            &format!("Added '{}' to local vocabulary", word),
                            MessageKind::Success,
                        );
                    }
                    Err(e) => {
                        editor.set_status_msg(&format!("Vocab error: {}", e), MessageKind::Error);
                    }
                }
            } else {
                editor.set_status_msg("Usage: :vocab <word>", MessageKind::Error);
            }
        }
        "ff" | "finder" | "filepicker" => {
            let initial = editor
                .popup
                .last_file_picker_dir
                .clone()
                .unwrap_or_else(|| std::path::PathBuf::from("."));
            editor.popup.open_file_picker(&initial, false);
        }
        s if s.starts_with("ff ") || s.starts_with("finder ") || s.starts_with("filepicker ") => {
            let arg = if let Some(p) = s.strip_prefix("ff ") {
                p
            } else if let Some(p) = s.strip_prefix("finder ") {
                p
            } else {
                s.strip_prefix("filepicker ").unwrap()
            }
            .trim();

            let path = std::path::Path::new(arg);

            if path.is_dir() {
                // Argument is a directory path
                editor.popup.open_file_picker(path, false);
            } else {
                // Argument is treated as an initial filter (e.g., `:ff ma`)
                let initial = editor
                    .popup
                    .last_file_picker_dir
                    .clone()
                    .unwrap_or_else(|| std::path::PathBuf::from("."));
                editor.popup.open_file_picker(&initial, false);

                if !arg.is_empty() {
                    if let Some(picker) = editor.popup.file_picker.as_mut() {
                        picker.set_initial_filter(arg);
                    }
                }
            }
        }

        "functions" | "funlist" | "fn" => {
            let entries = crate::popup::function_list::extract_functions(editor.buf());
            editor.popup.function_list =
                Some(crate::popup::function_list::FunctionListPopup::new(entries));
            editor.set_status_msg("Opened function navigation list", MessageKind::Info);
        }
        "mru" => {
            editor.open_mru_popup(true);
        }
        "mru!" | "mruall" => {
            editor.open_mru_popup(false); // Global
        }
        // ---- Buffer commands ----
        "bn" | "bnext" => {
            editor.switch_next_buffer();
            editor.record_action(RepeatableAction::BufferNext, 1);
        }
        "bp" | "bprev" | "bprevious" => {
            editor.switch_prev_buffer();
            editor.record_action(RepeatableAction::BufferPrev, 1);
        }
        "bd" | "bdelete" => {
            editor.close_buffer();
        }
        "e" | "new" => {
            editor.open_buffer(None);
        }
        s if s.starts_with("e ") => {
            let path = s.strip_prefix("e ").unwrap().trim();
            if !path.is_empty() {
                editor.open_buffer(Some(path.to_string()));
            }
        }
        s if s.starts_with("b ") => {
            let idx_str = s.strip_prefix("b ").unwrap().trim();
            if let Ok(idx) = idx_str.parse::<usize>() {
                editor.switch_buffer_by_index(idx);
            }
        }
        "tig" | "gitlog" | "glog" => {
            // No arguments provided, use default (None -> 10)
            editor.open_git_log(None);
        }
        s if s.starts_with("tig ") || s.starts_with("gitlog ") || s.starts_with("glog ") => {
            // Arguments provided (e.g., "tig 0", "tig 20"), parse the limit
            let parts: Vec<&str> = s.split_whitespace().collect();
            let arg = parts.get(1); // Get the optional second part

            // Parse the argument (provide the &&str type hint to fix E0282)
            let limit = arg.and_then(|a: &&str| a.parse::<usize>().ok());

            // None -> default (10)
            // Some(0) -> fetch all
            // Some(n) -> fetch n
            editor.open_git_log(limit);
        }

        // ---- Tag Commands ----
        s if s.starts_with("tag ") || s.starts_with("ta ") => {
            let name = if let Some(p) = s.strip_prefix("tag ") {
                p
            } else {
                s.strip_prefix("ta ").unwrap()
            }
            .trim();

            if name.is_empty() {
                editor.set_status_msg("Usage: :tag <name>", MessageKind::Error);
            } else {
                editor.jump_to_tag(name);
            }
        }
        "retag" => {
            editor.retag();
        }
        "tags" => {
            editor.show_tag_info();
        }

        //-- repl commands (anchor dont removed) --//
        // ---- Window commands ----
        "sp" | "split" => {
            editor.split_horizontal();
        }
        "vs" | "vsplit" => {
            editor.split_vertical();
        }
        "on" | "only" => {
            editor.only_window();
        }

        // ---- Ripgrep Commands ----
        "rg" | "lastrg" => {
            editor.ripgrep_last(); // Direct execution on editor
        }
        s if s.starts_with("rg ") => {
            let pattern = s.strip_prefix("rg ").unwrap().trim();
            editor.ripgrep_search(pattern); // Direct execution on editor
        }

        "cn" => {
            editor.ripgrep_next_result(); // Direct execution on editor
            editor.record_action(RepeatableAction::QuickfixNext, 1);
        }
        "cp" => {
            editor.ripgrep_prev_result(); // Direct execution on editor
            editor.record_action(RepeatableAction::QuickfixPrev, 1);
        }

        // ---- File / Quit commands ----
        "q" => {
            if !editor.active_modified() {
                if editor.buffer_count() > 1 {
                    editor.close_buffer();
                } else {
                    editor.quit_check();
                }
            } else {
                editor.set_status_msg(
                    "No write since last change (use :q! to force)",
                    MessageKind::Error,
                );
            }
        }
        "q!" => {
            if editor.buffer_count() > 1 {
                editor.close_buffer();
            } else {
                editor.force_quit();
            }
        }
        "w" => {
            if let Err(e) = editor.save_active_buffer() {
                editor.set_status_msg(&format!("Save failed: {}", e), MessageKind::Error);
            }
        }
        s if s.starts_with("w ") => {
            let path = s.strip_prefix("w ").unwrap().trim();
            if !path.is_empty() {
                editor.set_active_filename(path.to_string());
                if let Err(e) = editor.save_active_buffer() {
                    editor.set_status_msg(&format!("Save failed: {}", e), MessageKind::Error);
                }
            }
        }
        "wq" | "x" => {
            if let Err(e) = editor.save_active_buffer() {
                editor.set_status_msg(&format!("Save failed: {}", e), MessageKind::Error);
            } else if editor.buffer_count() > 1 {
                editor.close_buffer();
            } else {
                editor.force_quit();
            }
        }
        s if s.starts_with("wq ") || s.starts_with("x ") => {
            let path = if let Some(p) = s.strip_prefix("wq ") {
                p
            } else {
                s.strip_prefix("x ").unwrap()
            }
            .trim();

            if !path.is_empty() {
                editor.set_active_filename(path.to_string());
            }
            if let Err(e) = editor.save_active_buffer() {
                editor.set_status_msg(&format!("Save failed: {}", e), MessageKind::Error);
            } else if editor.buffer_count() > 1 {
                editor.close_buffer();
            } else {
                editor.force_quit();
            }
        }
        "gs" | "gitstatus" => {
            editor.open_git_status();
        }
        "diffthis" | "gd" => {
            editor.open_diffthis();
        }
        "stash" => {
            editor.handle_stash_command("");
        }
        s if s.starts_with("stash ") => {
            let comment = s.strip_prefix("stash ").unwrap().trim();
            editor.handle_stash_command(comment);
        }
        "llm" => {
            editor.open_llm_chat_session();
        }
        s if s.starts_with("prompt ") || s.starts_with("> ") => {
            let prompt_text = if s.starts_with("prompt ") {
                s.strip_prefix("prompt ").unwrap().trim()
            } else {
                s.strip_prefix("> ").unwrap().trim()
            };
            if !prompt_text.is_empty() {
                editor.llm_send_from_prompt(prompt_text.to_string());
            } else {
                editor.set_status_msg("Usage: :prompt <message>", MessageKind::Error);
            }
        }
        "gen_desc" => match crate::popup::command_palette::generate_default_desc_file() {
            Ok(()) => {
                let path = Config::descriptions_path()
                    .map(|p| p.display().to_string())
                    .unwrap_or_else(|_| "~/.config/ce/desc.json".to_string());
                editor.set_status_msg(
                    &format!("Generated default descriptions to {}", path),
                    MessageKind::Success,
                );
            }
            Err(e) => {
                editor.set_status_msg(
                    &format!("Failed to generate desc.json: {}", e),
                    MessageKind::Error,
                );
            }
        },
        "marks" => {
            editor.open_marks_popup();
        }
        "gc" | "gitcommit" => {
            editor.git_commit_generate();
        }
        // ---- Clear Search Highlight ----
        "noh" | "nohl" | "nohlsearch" => {
            editor.last_search_query = None;
            editor.buf_mut().search_pattern = None;
            editor.set_status_msg("Search highlight cleared", MessageKind::Info);
        }

        // ---- Jump to Line Number ----
        s if s.chars().all(|c| c.is_ascii_digit()) => {
            if let Ok(line_num) = s.parse::<usize>() {
                if line_num > 0 {
                    let max_lines = editor.buf().len_lines();
                    let line_idx = (line_num - 1).min(max_lines.saturating_sub(1));
                    let gutter = editor.active_gutter_width();
                    let (win, _buf) = editor.active_window_and_buf_mut();
                    win.row = line_idx;
                    win.col = 0;
                    win.desired_col = 0;
                    let viewport_h = win.position.height;
                    let viewport_w = win.position.width;
                    win.scroll_to_cursor(viewport_h, viewport_w, gutter);
                    editor.set_status_msg(
                        &format!("Jumped to line {}", line_idx + 1),
                        MessageKind::Info,
                    );
                } else {
                    editor.set_status_msg("Line number must be greater than 0", MessageKind::Error);
                }
            }
        }

        // ---- Unknown ----
        _ => {
            editor.set_status_msg(&format!("Unknown command: {}", cmd), MessageKind::Error);
        }
    }
}

// ---------------------------------------------------------------------------
// Command Completion
// ---------------------------------------------------------------------------

/// Auto-complete commands, paths, or matching persistent history strings.
pub fn complete_command(input: &str, history: &[String]) -> Vec<String> {
    #[rustfmt::skip]
    let command_list = vec![
        "q", "q!", "w", "wq", "x", "ls", "buffers", "files",
        "bn", "bnext", "bp", "bprev", "bprevious", "bd", "bdelete",
        "e", "new", "config", "vocab", "sp", "split", "vs", "vsplit",
        "on", "only", "vim", "brief", "scankey", "ff", "finder", "filepicker", 
        "tig", "glog", "rg", "lastrg", "cn", "cp","noh", "nohlsearch", "marks", "bookmarks", 
        "llm", "prompt", ">", "gs", "gitstatus", "stash", "diffthis", "gd", "checkhealth",
        "command_palette","guide","guide sync", "guide update", "gen_desc",
        "tag", "ta", "retag", "tags",
    ];
    //-- complete command (anchor dont removed) --//
    let mut results = Vec::new();

    // 1. Standard commands (only if no arguments are entered yet)
    if !input.contains(' ') {
        for cmd in &command_list {
            if cmd.starts_with(input) {
                results.push(cmd.to_string());
            }
        }
    } else if let Some((cmd_name, path_prefix)) = split_cmd_and_arg(input) {
        // 2. Path autocomplete for e/w/wq commands
        if cmd_name == "e" || cmd_name == "w" || cmd_name == "wq" {
            for path in complete_paths(path_prefix) {
                results.push(format!("{} {}", cmd_name, path));
            }
        }
    }
    // 3. Smart History Completer (Bash/Zsh style)
    // Scan unique history files and append exact historical matches
    for h in history {
        let trimmed = h.trim();
        if trimmed.starts_with(input) && trimmed != input && !results.contains(&trimmed.to_string())
        {
            results.push(trimmed.to_string());
        }
    }

    results
}

fn split_cmd_and_arg(input: &str) -> Option<(&str, &str)> {
    let mut parts = input.splitn(2, ' ');
    let cmd = parts.next()?;
    let arg = parts.next().unwrap_or("");
    Some((cmd, arg))
}

fn complete_paths(prefix: &str) -> Vec<String> {
    // Check if the user used relative prefix constraints
    let starts_with_dot_slash = prefix.starts_with("./");
    let starts_with_dot_dot_slash = prefix.starts_with("../");

    let path = Path::new(prefix);
    let (dir_path, file_prefix) = if prefix.ends_with('/') || prefix.is_empty() {
        (path, "")
    } else {
        (
            path.parent().unwrap_or_else(|| Path::new("")),
            path.file_name().and_then(|s| s.to_str()).unwrap_or(""),
        )
    };

    let search_dir = if dir_path.as_os_str().is_empty() {
        Path::new(".")
    } else {
        dir_path
    };

    let mut results = Vec::new();
    if let Ok(entries) = std::fs::read_dir(search_dir) {
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            if name.starts_with(file_prefix) {
                let mut full_path = dir_path.to_path_buf();
                full_path.push(&name);

                let mut path_str = full_path.to_string_lossy().to_string();

                // Re-apply prefix requirements to standard output paths
                if starts_with_dot_slash && !path_str.starts_with("./") {
                    path_str = format!("./{}", path_str);
                } else if starts_with_dot_dot_slash && !path_str.starts_with("../") {
                    path_str = format!("../{}", path_str);
                }

                if entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
                    path_str.push('/');
                }

                results.push(path_str);
            }
        }
    }
    results
}
