use crate::config::app_config::Config;
use crate::ed::editing;
use crate::ed::repeat::RepeatExt;
use crate::ed::repeat::RepeatableAction;
use crate::ed::Mode;
use crate::event::KeyEvent;
use crate::keybind::actions::Action;
use crate::keybind::bindings::get_active_bindings;
use crate::keybind::block_ops::delete_block;
use crate::keybind::block_ops::yank_block;
use crate::keybind::config_keys::{
    find_custom_action, find_custom_prefix_actions, normalize_config_key,
};
use crate::keybind::defaults::get_default_actions;
use crate::keybind::display::action_display_name;
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
                    if let Ok(action) = Action::parse(action_str) {
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

// Add this helper function:
pub fn execute_visual_block_edit(editor: &mut Editor, is_append: bool) {
    let anchor_opt = editor.active_window().visual_anchor;
    if let Some(anchor) = anchor_opt {
        let win_row = editor.active_window().row;
        let win_col = editor.active_window().col;

        let r1 = anchor.0.min(win_row);
        let r2 = anchor.0.max(win_row);
        let c1 = anchor.1.min(win_col);
        let c2 = anchor.1.max(win_col);

        let rows: Vec<usize> = (r1..=r2).collect();
        let target_col = if is_append { c2 + 1 } else { c1 };

        editor.visual_block_insert_state = Some(crate::ed::editor::VisualBlockInsertState {
            rows,
            col: target_col,
        });

        let (win, buf) = editor.active_window_and_buf_mut();
        win.row = r1;

        let line_len = buf.line_char_len(r1);
        if target_col > line_len {
            let pad = " ".repeat(target_col - line_len);
            let off = buf.rope.line_to_char(r1) + line_len;
            buf.rope.insert(off, &pad);
            buf.mark_modified();
        }
        win.col = target_col;

        win.visual_anchor = Some((r2, anchor.1));
        editor.insert_buffer = Some(String::new());

        let target_mode = if editor.prev_mode == Mode::Brief {
            Mode::Brief
        } else {
            Mode::Insert
        };
        editor.change_mode(target_mode);
    }
}

#[derive(PartialEq)]
pub enum SelectionOp {
    Yank,
    Delete,
    Change,
}

pub fn execute_selection_op(editor: &mut Editor, register: Option<char>, op: SelectionOp) {
    let mode = editor.mode();

    if mode == Mode::VisualBlock {
        if let Some(text) = yank_block(editor) {
            // Preserve exact original logic: Change uses clipboard directly, others use yank_to_register
            if op == SelectionOp::Change {
                editor.clipboard = Some(text);
                editor.clipboard_is_block = true;
            } else {
                editor.yank_to_register(text, register);
                editor.clipboard_is_block = true;
            }
        }
        if op != SelectionOp::Yank {
            delete_block(editor);
        }
    } else {
        let range = {
            let (win, buf) = editor.active_window_and_buf_mut();
            win.get_selection_range(buf, mode)
        };
        if let Some((start_char, end_char)) = range {
            let text = editor.buf().rope.slice(start_char..end_char).to_string();
            editor.yank_to_register(text, register);
            editor.clipboard_is_block = false;

            if op != SelectionOp::Yank {
                let (win, buf) = editor.active_window_and_buf_mut();
                buf.rope.remove(start_char..end_char);
                buf.mark_modified();
                let new_line = buf.rope.char_to_line(start_char);
                win.row = new_line;
                win.col = start_char.saturating_sub(buf.rope.line_to_char(new_line));
                win.clamp_cursor(buf);
                buf.parse_syntax();
            }
        }
    }

    let next_mode = if op == SelectionOp::Change {
        if editor.prev_mode == Mode::Brief {
            Mode::Brief
        } else {
            Mode::Insert
        }
    } else {
        if editor.prev_mode == Mode::Brief {
            Mode::Brief
        } else {
            Mode::Normal
        }
    };
    editor.change_mode(next_mode);
}

pub fn execute_indent_outdent(editor: &mut Editor, count: usize, is_outdent: bool) {
    let (r1, r2) = if let Some(anchor) = editor.active_window().visual_anchor {
        let row = editor.active_window().row;
        (anchor.0.min(row), anchor.0.max(row))
    } else {
        let row = editor.active_window().row;
        let end = (row + count.saturating_sub(1)).min(editor.buf().len_lines().saturating_sub(1));
        (row, end)
    };
    let line_count = r2.saturating_sub(r1) + 1;

    {
        let (win, buf) = editor.active_window_and_buf_mut();
        for r in r1..=r2 {
            let mut temp_win = win.clone();
            temp_win.row = r;
            if is_outdent {
                editing::outdent_line(&mut temp_win, buf);
            } else {
                editing::indent_line(&mut temp_win, buf);
            }
        }
        win.row = r1;
        win.col = 0;
        win.clamp_cursor(buf);
        win.desired_col = win.col;
    }

    editor.buf_mut().parse_syntax();
    editor.comp.on_edit();

    if matches!(
        editor.mode(),
        Mode::Visual | Mode::VisualLine | Mode::VisualBlock
    ) {
        let target_mode = if editor.prev_mode == Mode::Brief {
            Mode::Brief
        } else {
            Mode::Normal
        };
        editor.change_mode(target_mode);
        editor.clear_status_msg();
    }

    editor.record_action(
        RepeatableAction::Indent {
            count: line_count,
            outdent: is_outdent,
        },
        1,
    );
}
