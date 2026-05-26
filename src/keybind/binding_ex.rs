use crate::config::app_config::Config;
use crate::ed::Mode;
use crate::event::KeyEvent;
use crate::keybind::bindings::get_active_bindings;
use crate::keybind::bindings::get_default_actions;
use crate::keybind::bindings::Action;
use crate::keybind::resolve_single_key;
use crate::Editor;
use crossterm::event::KeyCode;
use crossterm::event::KeyModifiers;
use std::collections::HashSet;
// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Format a key event into a string representation.
///
/// - `Shift+G` → `"G"`  (uppercase letter, no shift prefix)
/// - `Ctrl+G`  → `"ctrl+g"`
/// - `Alt+G`   → `"alt+g"`
pub fn format_key(key: KeyEvent) -> String {
    let mut parts = Vec::new();

    if key.modifiers.contains(KeyModifiers::CONTROL) {
        parts.push("ctrl");
    }
    if key.modifiers.contains(KeyModifiers::ALT) {
        parts.push("alt");
    }

    let code_str = match key.code {
        KeyCode::Esc => "esc".to_string(),
        KeyCode::Enter => "enter".to_string(),
        KeyCode::Backspace => "backspace".to_string(),
        KeyCode::Tab => "tab".to_string(),
        KeyCode::BackTab => "backtab".to_string(),
        KeyCode::Delete => "delete".to_string(),
        KeyCode::Insert => "insert".to_string(),
        KeyCode::Up => "up".to_string(),
        KeyCode::Down => "down".to_string(),
        KeyCode::Left => "left".to_string(),
        KeyCode::Right => "right".to_string(),
        KeyCode::Home => "home".to_string(),
        KeyCode::End => "end".to_string(),
        KeyCode::PageUp => "pageup".to_string(),
        KeyCode::PageDown => "pagedown".to_string(),
        KeyCode::Char(' ') => "space".to_string(),
        KeyCode::Char(c) => c.to_string(),
        KeyCode::F(num) => format!("f{}", num),
        _ => "".to_string(),
    };

    if code_str.is_empty() {
        return "".to_string();
    }

    parts.push(&code_str);
    parts.join("+")
}

// ---------------------------------------------------------------------------
// KeySuggestion — richer suggestion entry used by the which-key popup
// ---------------------------------------------------------------------------

/// A single narrowed candidate shown in the which-key popup.
#[derive(Debug, Clone)]
pub struct KeySuggestion {
    /// Keys still to type (e.g. `"v"` when pending is `"space p"`).
    pub suffix: String,
    /// Complete binding string (e.g. `"space p v"`).
    pub full_bind: String,
    /// Human-readable action label (e.g. `"Paste"`).
    pub description: String,
    /// Resolved action — used for auto-execute on last match.
    pub action: Action,
}

// ---------------------------------------------------------------------------
// Suggestion Engine for Which-Key Popups
// ---------------------------------------------------------------------------

pub fn get_sequence_suggestions(config: &Config, pending: &str, mode: Mode) -> Vec<KeySuggestion> {
    // ── Translate Brief Mode F10 pending state to leader character ──
    let mut resolved_pending = pending.to_lowercase();
    if mode == Mode::Brief {
        let brief_leader = config
            .keybindings
            .brief
            .iter()
            .find(|(_, action_str)| {
                let norm = action_str.to_lowercase().replace('_', "");
                norm == "shortcuts" || norm == "shortcutspopup"
            })
            .map(|(key, _)| normalize_config_key(key))
            .unwrap_or_else(|| "f10".to_string());

        if resolved_pending.starts_with(&brief_leader) {
            resolved_pending = resolved_pending.replacen(&brief_leader, &config.leader, 1);
        }
    }

    let prefix = format!("{} ", resolved_pending);
    let mut seen = HashSet::new();
    let mut out = Vec::new();

    // 1. Default bindings (with Leader override)
    for (bind, action) in get_default_actions() {
        // Dynamically replace default "space " leader with the configured leader
        let resolved_bind = if bind.starts_with("space ") {
            bind.replacen("space ", &format!("{} ", config.leader), 1)
        } else {
            bind.to_string()
        };

        let bind_lower = resolved_bind.to_lowercase();
        if bind_lower.starts_with(&prefix) {
            let suffix = resolved_bind[prefix.len()..].to_string();
            if seen.insert(suffix.clone()) {
                out.push(KeySuggestion {
                    suffix,
                    full_bind: resolved_bind.clone(),
                    description: action_display_name(&action),
                    action,
                });
            }
        }
    }

    let mut check_suggestions = |map: &std::collections::HashMap<String, String>| {
        for (bind_key, action_str) in map {
            let normalized_bind = normalize_config_key(bind_key);
            let resolved_bind = normalized_bind.replace("<leader>", &config.leader);

            let norm = resolved_bind.to_lowercase();
            if norm.starts_with(&prefix) {
                let suffix = resolved_bind[prefix.len()..].to_string();
                if seen.insert(suffix.clone()) {
                    if let Ok(action) = action_str.parse::<Action>() {
                        out.push(KeySuggestion {
                            suffix,
                            full_bind: resolved_bind.clone(),
                            description: action_display_name(&action),
                            action,
                        });
                    }
                }
            }
        }
    };

    // 2. Custom active-mode bindings
    check_suggestions(get_active_bindings(config, mode));

    // 2b. For Brief Mode: share Normal Mode leader suggestion rows
    if mode == Mode::Brief && resolved_pending.starts_with(&config.leader) {
        check_suggestions(&config.keybindings.normal);
    }

    // 3. Custom global bindings
    check_suggestions(&config.keybindings.global);

    out.sort_by(|a, b| a.suffix.cmp(&b.suffix));
    out
}

// ---------------------------------------------------------------------------
// Resolve Key Sequences
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResolveResult {
    /// Exact match — execute immediately.
    Action(Action),
    /// Narrowed to exactly one reachable binding — auto-execute immediately.
    AutoAction(Action),
    /// Valid prefix; keep accumulating keys.
    Pending,
    /// No match and no valid prefix.
    None,
}

pub fn resolve_sequence(
    config: &Config,
    key_seq: &str,
    ghost_active: bool,
    mode: Mode,
) -> ResolveResult {
    // Insert and Search modes never use multi-key sequences.
    if mode == Mode::Insert || mode == Mode::Search {
        return ResolveResult::None;
    }

    // ── Translate Brief Mode dynamic leader ───────────────────────────
    let mut resolved_seq = key_seq.to_string();
    if mode == Mode::Brief {
        // Dynamically find the key mapped to "shortcuts" in your config.json brief map
        let brief_leader = config
            .keybindings
            .brief
            .iter()
            .find(|(_, action_str)| {
                let norm = action_str.to_lowercase().replace('_', "");
                norm == "shortcuts"
            })
            .map(|(key, _)| normalize_config_key(key))
            .unwrap_or_else(|| "f12".to_string()); // Fallback to f12 if not defined

        if resolved_seq.starts_with(&brief_leader) {
            // Map the configured brief leader prefix to standard leader character (e.g. ",")
            resolved_seq = resolved_seq.replacen(&brief_leader, &config.leader, 1);
        } else {
            // Any other key combinations in Brief mode do not trigger sequence prefixes
            return ResolveResult::None;
        }
    }

    // ── 1. Exact match — custom config ────────────────────────────
    if let Some(action) = find_custom_action(config, &resolved_seq, mode) {
        return ResolveResult::Action(action);
    }

    // ── 2. Exact match — defaults (with Leader override) ──────────
    if ghost_active && (resolved_seq == "tab" || resolved_seq == "right") {
        return ResolveResult::Action(Action::AcceptCompletion);
    }

    for (bind, action) in get_default_actions() {
        // Dynamically replace default "space " leader with the configured leader
        let resolved_bind = if bind.starts_with("space ") {
            bind.replacen("space ", &format!("{} ", config.leader), 1)
        } else {
            bind.to_string()
        };

        if resolved_bind == resolved_seq {
            return ResolveResult::Action(action);
        }
    }

    // ── 3. Prefix scan — collect all reachable terminal actions ───
    let mut candidates = find_custom_prefix_actions(config, &resolved_seq, mode);

    for (bind, action) in get_default_actions() {
        // Dynamically replace default "space " leader with the configured leader for prefixes
        let resolved_bind = if bind.starts_with("space ") {
            bind.replacen("space ", &format!("{} ", config.leader), 1)
        } else {
            bind.to_string()
        };

        if resolved_bind.starts_with(&resolved_seq) && resolved_bind.len() > resolved_seq.len() {
            if !candidates.contains(&action) {
                candidates.push(action);
            }
        }
    }

    // Do not auto-execute on partial prefix matches
    if candidates.is_empty() {
        ResolveResult::None
    } else {
        ResolveResult::Pending
    }
}

// ---------------------------------------------------------------------------
// action_display_name
// ---------------------------------------------------------------------------

/// Convert an `Action` into a human-readable label.
///
/// `CamelCase` → `"Camel Case"`, tuple payload preserved:
/// `SwitchBuffer(1)` → `"Switch Buffer(1)"`.
pub fn action_display_name(action: &Action) -> String {
    let raw = format!("{:?}", action);

    // Split off any tuple payload  "SwitchBuffer(1)" → "SwitchBuffer" + "(1)"
    let (main, suffix) = match raw.find('(') {
        Some(pos) => (&raw[..pos], Some(&raw[pos..])),
        None => (raw.as_str(), None),
    };

    let mut out = String::with_capacity(main.len() + 8);
    for (i, ch) in main.chars().enumerate() {
        if ch.is_uppercase() && i > 0 {
            out.push(' ');
        }
        out.push(ch);
    }
    if let Some(suf) = suffix {
        out.push_str(suf);
    }
    out
}

// ---------------------------------------------------------------------------
// lookup_key_action  (scankey overlay helper)
// ---------------------------------------------------------------------------

/// Return a human-readable description of what `key_str` does in `mode`.
/// Used by the scankey overlay to display a binding without executing it.
pub fn lookup_key_action(config: &Config, key_str: &str, mode: Mode, raw_key: KeyEvent) -> String {
    // Insert / Brief / Command — resolve_single_key knows the answers.
    if mode != Mode::Normal {
        if let Some(action) = resolve_single_key(config, key_str, mode, false, raw_key) {
            return action_display_name(&action);
        }
    }

    // Normal mode (and fallback) — resolve_sequence handles multi-key bindings.
    match resolve_sequence(config, key_str, false, mode) {
        ResolveResult::Action(action) | ResolveResult::AutoAction(action) => {
            action_display_name(&action)
        }
        ResolveResult::Pending => {
            // Show what sequences this key is a prefix for.
            let suggestions = get_sequence_suggestions(config, key_str, mode);
            if suggestions.is_empty() {
                "Partial sequence…".to_string()
            } else {
                let items: Vec<String> = suggestions
                    .iter()
                    .take(4)
                    .map(|s| format!("{}→{}", s.suffix, s.description))
                    .collect();
                format!("Prefix: {}", items.join(", "))
            }
        }
        ResolveResult::None => "No binding".to_string(),
    }
}

/// Normalizes various user-friendly config key notations into the standard internal representation.
/// Examples:
/// - "<alt>+<shift>+q" -> "alt+shift+q"
/// - "<alt> q"          -> "alt+q"
/// - "<insert> <ctrl> p" -> "<insert> ctrl+p"
/// - "<normal> <tab>"   -> "<normal> tab"
fn normalize_config_key(bind_key: &str) -> String {
    let mut key = bind_key.trim().to_string();

    // 1. Extract and preserve the mode prefix
    let mut mode_prefix = "";
    if key.starts_with("<normal> ") {
        mode_prefix = "<normal> ";
        key = key["<normal> ".len()..].to_string();
    } else if key.starts_with("<insert> ") {
        mode_prefix = "<insert> ";
        key = key["<insert> ".len()..].to_string();
    } else if key.starts_with("<brief> ") {
        mode_prefix = "<brief> ";
        key = key["<brief> ".len()..].to_string();
    } else if key.starts_with("<command> ") {
        mode_prefix = "<command> ";
        key = key["<command> ".len()..].to_string();
    } else if key.starts_with("normal+") {
        mode_prefix = "normal+";
        key = key["normal+".len()..].to_string();
    } else if key.starts_with("insert+") {
        mode_prefix = "insert+";
        key = key["insert+".len()..].to_string();
    } else if key.starts_with("brief+") {
        mode_prefix = "brief+";
        key = key["brief+".len()..].to_string();
    } else if key.starts_with("command+") {
        mode_prefix = "command+";
        key = key["command+".len()..].to_string();
    }

    // 2. Normalize modifier chains
    // Convert e.g., "<alt>+<shift>+q" -> "<alt+shift+q>"
    let mut normalized = key
        .replace(">+<", "+")
        .replace("><", "+")
        .replace("> + <", "+")
        .replace("> <", "+");

    // Preserve the `<leader>` token dynamically by replacing it during stripping
    normalized = normalized.replace("<leader>", "__LEADER__");

    // 3. Strip outer brackets from valid modifiers and special keys
    let mut final_key = String::new();
    let mut in_bracket = false;
    let mut bracket_content = String::new();

    for ch in normalized.chars() {
        if ch == '<' {
            in_bracket = true;
            bracket_content.clear();
        } else if ch == '>' {
            in_bracket = false;
            let content = bracket_content.to_lowercase();
            // List of valid modifiers and special keys we strip brackets from
            if matches!(
                content.as_str(),
                "alt"
                    | "ctrl"
                    | "shift"
                    | "tab"
                    | "backspace"
                    | "enter"
                    | "esc"
                    | "space"
                    | "up"
                    | "down"
                    | "left"
                    | "right"
                    | "pageup"
                    | "pagedown"
                    | "home"
                    | "end"
                    | "delete"
                    | "insert"
            ) || content.contains('+')
            {
                final_key.push_str(&bracket_content);
            } else {
                final_key.push('<');
                final_key.push_str(&bracket_content);
                final_key.push('>');
            }
        } else if in_bracket {
            bracket_content.push(ch);
        } else {
            final_key.push(ch);
        }
    }

    // Restore the `<leader>` token
    final_key = final_key.replace("__LEADER__", "<leader>");

    // 4. Convert remaining space-separated modifiers, e.g. "alt q" -> "alt+q"
    let modifiers = ["alt", "ctrl", "shift"];
    for m in &modifiers {
        let pattern_space = format!("{} ", m);
        let pattern_plus = format!("{}+", m);
        final_key = final_key.replace(&pattern_space, &pattern_plus);
    }

    format!("{}{}", mode_prefix, final_key.trim())
}

pub fn find_custom_action(config: &Config, key_seq: &str, mode: Mode) -> Option<Action> {
    // 1. Try active mode-specific bindings first
    let active_bindings = get_active_bindings(config, mode);
    for (bind, action_str) in active_bindings {
        let normalized_bind = normalize_config_key(bind);
        let resolved_bind = normalized_bind.replace("<leader>", &config.leader);
        if resolved_bind == key_seq {
            return match action_str.parse::<Action>() {
                Ok(a) => Some(a),
                Err(_) => None,
            };
        }
    }

    // 1b. For Brief Mode: check Normal Mode bindings if starting with the leader
    if mode == Mode::Brief && key_seq.starts_with(&config.leader) {
        for (bind, action_str) in &config.keybindings.normal {
            let normalized_bind = normalize_config_key(bind);
            let resolved_bind = normalized_bind.replace("<leader>", &config.leader);
            if resolved_bind == key_seq {
                return action_str.parse::<Action>().ok();
            }
        }
    }

    // 2. Try global bindings as a fallback
    for (bind, action_str) in &config.keybindings.global {
        let normalized_bind = normalize_config_key(bind);
        let resolved_bind = normalized_bind.replace("<leader>", &config.leader);
        if resolved_bind == key_seq {
            return action_str.parse::<Action>().ok();
        }
    }

    None
}

pub fn find_custom_prefix_actions(config: &Config, key_seq: &str, mode: Mode) -> Vec<Action> {
    let active_bindings = get_active_bindings(config, mode);
    let key_lower = key_seq.to_lowercase();
    let mut actions = Vec::new();

    let mut check_prefix = |map: &std::collections::HashMap<String, String>| {
        for (bind_key, action_str) in map {
            let normalized_bind = normalize_config_key(bind_key);
            let resolved_bind = normalized_bind.replace("<leader>", &config.leader);
            let norm = resolved_bind.to_lowercase();

            if norm.starts_with(&key_lower) && norm.len() > key_lower.len() {
                if let Ok(action) = action_str.parse::<Action>() {
                    if !actions.contains(&action) {
                        actions.push(action);
                    }
                }
            }
        }
    };

    check_prefix(active_bindings);

    // 1b. For Brief Mode: search Normal Mode prefix candidates if starting with the leader
    if mode == Mode::Brief && key_lower.starts_with(&config.leader) {
        check_prefix(&config.keybindings.normal);
    }

    check_prefix(&config.keybindings.global);

    actions
}

pub fn yank_block(editor: &mut Editor) -> Option<String> {
    let win = editor.active_window();
    let buf = editor.buf();
    let anchor = win.visual_anchor?;

    let r1 = anchor.0.min(win.row);
    let r2 = anchor.0.max(win.row);
    let c1 = anchor.1.min(win.col);
    let c2 = anchor.1.max(win.col);

    let mut lines = Vec::new();
    for r in r1..=r2 {
        if r >= buf.len_lines() {
            continue;
        }
        let line_text = buf.line_text(r);
        let chars: Vec<char> = line_text.chars().collect();

        let start = c1.min(chars.len());
        let end = (c2 + 1).min(chars.len());

        if start < end {
            let chunk: String = chars[start..end].iter().collect();
            lines.push(chunk);
        } else {
            lines.push(String::new());
        }
    }

    Some(lines.join("\n"))
}

pub fn delete_block(editor: &mut Editor) {
    let anchor = match editor.active_window().visual_anchor {
        Some(a) => a,
        None => return,
    };

    let (win_row, win_col) = {
        let win = editor.active_window();
        (win.row, win.col)
    };

    let r1 = anchor.0.min(win_row);
    let r2 = anchor.0.max(win_row);
    let c1 = anchor.1.min(win_col);
    let c2 = anchor.1.max(win_col);

    let buf = editor.buf_mut();
    for r in r1..=r2 {
        if r >= buf.len_lines() {
            continue;
        }

        let line_char_offset = buf.rope.line_to_char(r);
        let line_len = buf.line_char_len(r);

        let start_col = c1.min(line_len);
        let end_col = (c2 + 1).min(line_len);

        if start_col < end_col {
            let remove_start = line_char_offset + start_col;
            let remove_end = line_char_offset + end_col;
            buf.rope.remove(remove_start..remove_end);
        }
    }

    buf.modified = true;
    buf.parse_syntax();

    // disjoint mutable borrow to safely position and clamp cursor
    let (win, buf) = editor.active_window_and_buf_mut();
    win.row = r1;
    win.col = c1.min(buf.line_char_len(r1));
    win.clamp_cursor(buf);
}

pub fn paste_block(editor: &mut Editor, text: &str) {
    let (win, buf) = editor.active_window_and_buf_mut();
    let (row, col) = (win.row, win.col);

    let lines: Vec<&str> = if text.contains("\r\n") {
        text.split("\r\n").collect()
    } else {
        text.split('\n').collect()
    };

    for (i, line_text) in lines.iter().enumerate() {
        let target_row = row + i;

        while target_row >= buf.len_lines() {
            buf.rope.insert(buf.rope.len_chars(), "\n");
        }

        let line_len = buf.line_char_len(target_row);

        if col > line_len {
            let pad_spaces = " ".repeat(col - line_len);
            let insert_offset = buf.rope.line_to_char(target_row) + line_len;
            buf.rope.insert(insert_offset, &pad_spaces);
        }

        let insert_offset = buf.rope.line_to_char(target_row) + col;
        buf.rope.insert(insert_offset, line_text);
    }

    buf.modified = true;
    buf.parse_syntax();
}
