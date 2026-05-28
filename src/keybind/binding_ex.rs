use crate::config::app_config::Config;
use crate::ed::Mode;
use crate::event::KeyEvent;
use crate::keybind::actions::Action;
use crate::keybind::bindings::get_active_bindings;
use crate::keybind::config_keys::{
    find_custom_action, find_custom_prefix_actions, normalize_config_key,
};
use crate::keybind::defaults::get_default_actions;
use crate::keybind::display::action_display_name;
use crate::keybind::resolve_single_key;
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
        parts.push("ctrl".to_string());
    }
    if key.modifiers.contains(KeyModifiers::ALT) {
        parts.push("alt".to_string());
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
        KeyCode::Char(c) => {
            // Crossterm may send Shift+P as either Char('P') or Char('p')+SHIFT.
            // Normalize: always lowercase the char, and emit "shift" prefix
            // when SHIFT is held with ctrl/alt (global shortcuts).
            // BUT: plain Shift+letter (no ctrl/alt) stays uppercase for vim
            // bindings like "G", "V", "I" — do NOT emit "shift" prefix there.
            let has_ctrl_or_alt = key.modifiers.contains(KeyModifiers::CONTROL)
                || key.modifiers.contains(KeyModifiers::ALT);
            if has_ctrl_or_alt && key.modifiers.contains(KeyModifiers::SHIFT) {
                parts.push("shift".to_string());
                c.to_lowercase().to_string()
            } else {
                // Plain letter, uppercase or lowercase — emit as-is.
                // "G" stays "G", "v" stays "v".
                c.to_string()
            }
        }
        KeyCode::F(num) => format!("f{}", num),
        _ => "".to_string(),
    };

    if code_str.is_empty() {
        return "".to_string();
    }

    parts.push(code_str);
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
    // Global chord shortcuts are single-key events handled before this
    // function is ever called — they must never accumulate as a prefix.
    if key_seq.starts_with("ctrl+shift+") {
        return ResolveResult::None;
    }
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
