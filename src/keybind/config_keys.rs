// keybind/config_keys.rs
//! Config key normalization and custom binding lookup.

use crate::config::app_config::Config;
use crate::ed::Mode;
use crate::keybind::actions::Action;
use crate::keybind::bindings::get_active_bindings;

/// Normalizes various user-friendly config key notations into the standard internal representation.
/// Examples:
/// - "<alt>+<shift>+q" -> "alt+shift+q"
/// - "<alt> q"          -> "alt+q"
/// - "<insert> <ctrl> p" -> "<insert> ctrl+p"
/// - "<normal> <tab>"   -> "<normal> tab"
pub fn normalize_config_key(bind_key: &str) -> String {
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
