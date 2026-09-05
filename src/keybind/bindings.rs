//--+ keybind/bindings.rs
//! Configurable keybindings and Actions.
//!
//! Maps physical Crossterm key events to logical Editor `Action` items,
//! supporting custom user mappings parsed from the configuration file.

use crate::config::app_config::Config;
use crate::ed::buffer::Buffer;
use crate::ed::editor::Editor;
use crate::ed::editor::PendingInput;
use crate::ed::mode::{MessageKind, Mode};
use crate::ed::repeat::{DeleteDirection, RepeatExt, RepeatableAction};
use crate::ed::window::Window;
use crate::ed::{editing, movement};
pub use crate::keybind::actions::Action;
use crate::keybind::binding_ex::execute_indent_outdent;
use crate::keybind::binding_ex::execute_selection_op;
use crate::keybind::binding_ex::execute_visual_block_edit;
use crate::keybind::binding_ex::SelectionOp;
use crate::keybind::block_ops::paste_block;
use crate::keybind::config_keys::find_custom_action;
use crate::keybind::defaults::get_default_actions;
use crate::keybind::display::action_display_name;
use crate::keybind::safetynet::check_around_function_safetynet;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use std::time::Instant;
use unicode_segmentation::UnicodeSegmentation;

fn apply_to_all_cursors(editor: &mut Editor, mut f: impl FnMut(&mut Window, &mut Buffer)) {
    let win = editor.active_window();
    let primary_pos = (win.row, win.col);
    let mut all_cursors = win.extra_cursors.clone();
    all_cursors.push(primary_pos);
    // Sort descending by row, then descending by column
    all_cursors.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| b.1.cmp(&a.1)));
    all_cursors.dedup();

    let mut new_cursors: Vec<(usize, usize)> = Vec::with_capacity(all_cursors.len());
    let mut new_primary = primary_pos;
    let mut primary_processed = false;

    for (r, c) in all_cursors {
        let (win, buf) = editor.active_window_and_buf_mut();
        let lines_before = buf.len_lines();
        let chars_before = buf.rope.len_chars();

        win.row = r.min(lines_before.saturating_sub(1));
        win.col = c.min(buf.line_char_len(win.row));
        f(win, buf);

        let lines_after = buf.len_lines();
        let chars_after = buf.rope.len_chars();
        let lines_diff = lines_after as isize - lines_before as isize;
        let chars_diff = chars_after as isize - chars_before as isize;

        // Shift already processed cursors
        for nc in &mut new_cursors {
            if lines_diff != 0 && nc.0 > r {
                nc.0 = (nc.0 as isize + lines_diff) as usize;
            } else if lines_diff == 0 && nc.0 == r {
                let new_col = nc.1 as isize + chars_diff;
                nc.1 = new_col.max(0) as usize;
            }
        }
        if primary_processed {
            if lines_diff != 0 && new_primary.0 > r {
                new_primary.0 = (new_primary.0 as isize + lines_diff) as usize;
            } else if lines_diff == 0 && new_primary.0 == r {
                let new_col = new_primary.1 as isize + chars_diff;
                new_primary.1 = new_col.max(0) as usize;
            }
        }

        let new_pos = (win.row, win.col);
        if (r, c) == primary_pos {
            new_primary = new_pos;
            primary_processed = true;
        } else {
            new_cursors.push(new_pos);
        }
    }

    let (win, _) = editor.active_window_and_buf_mut();
    win.row = new_primary.0;
    win.col = new_primary.1;
    win.extra_cursors = new_cursors;
}

fn apply_movement_to_all_cursors(editor: &mut Editor, mut f: impl FnMut(&mut Window, &Buffer)) {
    let (win, buf) = editor.active_window_and_buf_mut();
    f(win, buf);
    // Capture the primary cursor's result BEFORE touching extra cursors —
    // the loop below reuses win.row/win.col as scratch space for each
    // extra cursor, so without this the primary position gets silently
    // overwritten by whichever extra cursor is processed last. That was
    // the cause of a 5-cursor multicursor selection collapsing to 4
    // effective cursors after entering Insert mode (e.g. via `a`).
    let primary_result = (win.row, win.col);
    let cursors = std::mem::take(&mut win.extra_cursors);
    let mut new_cursors = Vec::with_capacity(cursors.len());
    for (r, c) in cursors {
        win.row = r;
        win.col = c;
        f(win, buf);
        new_cursors.push((win.row, win.col));
    }
    win.extra_cursors = new_cursors;
    win.row = primary_result.0;
    win.col = primary_result.1;
}
// ========== BRIEF MODE HOME/END TRACKERS ==========
struct BriefHomeTracker {
    last_press: Option<Instant>,
    count: usize,
}

struct BriefEndTracker {
    last_press: Option<Instant>,
    count: usize,
}

// ========== AROUND-FUNCTION SAFETYNET ==========

/// Maximum number of lines a function may span before the safetynet rejects
/// a delete/change-around operation.
const AROUND_FN_MAX_LINES: usize = 500;

/// Information returned by `Editor::function_around_span_info`.
#[derive(Debug, Clone)]
pub struct FunctionSpanInfo {
    /// Inclusive start row of the function node.
    pub start_row: usize,
    /// Exclusive end row (the row *after* the closing brace).
    pub end_row: usize,
    /// Number of lines the function spans (`end_row - start_row`).
    pub line_count: usize,
    /// How many nested function-like nodes live inside this one.
    /// E.g. a function that contains `pub fn helper() { … }` counts as 1.
    pub nested_fn_count: usize,
}

// ---------------------------------------------------------------------------
// Resolve Single Key
// ---------------------------------------------------------------------------
pub fn resolve_single_key(
    config: &Config,
    key_str: &str,
    mode: Mode,
    ghost_active: bool,
    raw_key: KeyEvent,
) -> Option<Action> {
    if let Some(action) = find_custom_action(config, key_str, mode) {
        return Some(action);
    }

    // ── 2. Default bare-char insertion for Insert / Brief ─────────
    // (Checked after config so users can override e.g. "insert+j" if they really want to)
    if mode == Mode::Insert || mode == Mode::Brief {
        if let KeyCode::Char(ch) = raw_key.code {
            if raw_key.modifiers == KeyModifiers::NONE || raw_key.modifiers == KeyModifiers::SHIFT {
                return Some(Action::InsertChar(ch));
            }
        }
    }

    // ── 2b. Shift+Navigation for Selection (Insert / Brief) ──────
    if mode == Mode::Insert || mode == Mode::Brief {
        if raw_key.modifiers.contains(KeyModifiers::SHIFT) {
            match raw_key.code {
                KeyCode::Left => return Some(Action::ExtendSelectionLeft),
                KeyCode::Right => return Some(Action::ExtendSelectionRight),
                KeyCode::Up => return Some(Action::ExtendSelectionUp),
                KeyCode::Down => return Some(Action::ExtendSelectionDown),
                KeyCode::Home => return Some(Action::ExtendSelectionLineStart),
                KeyCode::End => return Some(Action::ExtendSelectionLineEnd),
                KeyCode::PageUp => return Some(Action::ExtendSelectionPageUp),
                KeyCode::PageDown => return Some(Action::ExtendSelectionPageDown),
                _ => {}
            }
        }
    }

    // ── 3. Mode-specific hardcoded fallback ───────────────────────
    match mode {
        Mode::Brief => {
            // Alt+key combinations
            if raw_key.modifiers.contains(KeyModifiers::ALT) {
                if let Some(action) = resolve_brief_alt_key(raw_key) {
                    return Some(action);
                }
            }

            // Ctrl+key combinations
            if raw_key.modifiers.contains(KeyModifiers::CONTROL) {
                return match raw_key.code {
                    KeyCode::Char('n') | KeyCode::Char('N') => Some(Action::CycleCompletionNext),
                    KeyCode::Char('p') | KeyCode::Char('P') => Some(Action::CycleCompletionPrev),
                    KeyCode::Char('s') => Some(Action::Save),
                    KeyCode::Char('c') | KeyCode::Char('C') => Some(Action::BriefCopySelection),
                    KeyCode::Char('x') | KeyCode::Char('X') => Some(Action::BriefCutSelection),
                    KeyCode::Char('v') | KeyCode::Char('V') => Some(Action::Paste),
                    KeyCode::Char('h') => Some(Action::ExitMode), // ← optional: Ctrl+H backspace compat
                    _ => None,
                };
            }

            // Special keys
            match raw_key.code {
                KeyCode::Esc => Some(Action::ExitMode),
                KeyCode::Backspace => Some(Action::Backspace),
                KeyCode::Delete => Some(Action::DeleteCharForward),
                KeyCode::Enter => Some(Action::InsertNewline),
                KeyCode::F(1) => Some(Action::EnterWindowNav),
                KeyCode::F(2) => Some(Action::EnterCloseWindowNav),
                KeyCode::F(9) => Some(Action::EnterCommand),
                KeyCode::Tab => {
                    if ghost_active {
                        Some(Action::AcceptCompletion)
                    } else {
                        Some(Action::InsertTab)
                    }
                }
                KeyCode::Home => Some(Action::MoveLineStart),
                KeyCode::End => Some(Action::MoveLineEnd),
                KeyCode::Left => Some(Action::MoveLeft),
                KeyCode::PageUp => Some(Action::PageUp),
                KeyCode::PageDown => Some(Action::PageDown),
                KeyCode::Right => {
                    if ghost_active {
                        Some(Action::AcceptCompletion)
                    } else {
                        Some(Action::MoveRight)
                    }
                }
                KeyCode::Up => {
                    if ghost_active {
                        Some(Action::CycleCompletionPrev)
                    } else {
                        Some(Action::MoveUp)
                    }
                }
                KeyCode::Down => {
                    if ghost_active {
                        Some(Action::CycleCompletionNext)
                    } else {
                        Some(Action::MoveDown)
                    }
                }
                _ => None,
            }
        }
        Mode::Insert => {
            // Tab / Right with ghost text
            if raw_key.code == KeyCode::Tab {
                return if ghost_active {
                    Some(Action::AcceptCompletion)
                } else {
                    Some(Action::InsertTab)
                };
            }
            if raw_key.code == KeyCode::Right && ghost_active {
                return Some(Action::AcceptCompletion);
            }

            // Ctrl+n / Ctrl+p completion cycling
            if raw_key.modifiers.contains(KeyModifiers::CONTROL) {
                return match raw_key.code {
                    KeyCode::Char('n') | KeyCode::Char('N') => Some(Action::CycleCompletionNext),
                    KeyCode::Char('p') | KeyCode::Char('P') => Some(Action::CycleCompletionPrev),
                    _ => None,
                };
            }

            // Alt+key shortcuts
            if raw_key.modifiers.contains(KeyModifiers::ALT) {
                if let Some(action) = resolve_insert_alt_key(raw_key) {
                    return Some(action);
                }
            }

            // Special keys
            match raw_key.code {
                KeyCode::Esc => Some(Action::ExitMode),
                KeyCode::Backspace => Some(Action::Backspace),
                KeyCode::Delete => Some(Action::DeleteCharForward),
                KeyCode::Enter => Some(Action::InsertNewline),
                KeyCode::Home => Some(Action::MoveLineStart),
                KeyCode::End => Some(Action::MoveLineEnd),
                KeyCode::Left => Some(Action::MoveLeft),
                KeyCode::Right => Some(Action::MoveRight),
                KeyCode::Up => {
                    if ghost_active {
                        Some(Action::CycleCompletionPrev)
                    } else {
                        Some(Action::MoveUp)
                    }
                }
                KeyCode::Down => {
                    if ghost_active {
                        Some(Action::CycleCompletionNext)
                    } else {
                        Some(Action::MoveDown)
                    }
                }
                KeyCode::PageUp => Some(Action::PageUp),
                KeyCode::PageDown => Some(Action::PageDown),
                _ => None,
            }
        }

        Mode::Command => {
            if raw_key.modifiers.contains(KeyModifiers::CONTROL) {
                return match raw_key.code {
                    KeyCode::Char('a') => Some(Action::CommandLineStart),
                    KeyCode::Char('e') => Some(Action::CommandLineEnd),
                    KeyCode::Char('b') => Some(Action::CommandLineLeft),
                    KeyCode::Char('f') => Some(Action::CommandLineRight),
                    KeyCode::Char('d') => Some(Action::CommandDeleteChar),
                    KeyCode::Char('k') => Some(Action::CommandLineKillToEnd),
                    KeyCode::Char('r') => Some(Action::CommandEnterRegisterMode),
                    KeyCode::Char('n') | KeyCode::Char('N') => Some(Action::CommandHistoryNext),
                    KeyCode::Char('p') | KeyCode::Char('P') => Some(Action::CommandHistoryPrev),
                    _ => None,
                };
            }
            if raw_key.modifiers.contains(KeyModifiers::ALT) {
                return match raw_key.code {
                    KeyCode::Char('d') | KeyCode::Char('D') => Some(Action::CommandClear),
                    _ => None,
                };
            }

            match raw_key.code {
                KeyCode::Esc => Some(Action::ExitMode),
                KeyCode::Enter => Some(Action::ExecuteCommand),
                KeyCode::Tab => Some(Action::CompleteCommand),
                KeyCode::Backspace => Some(Action::CommandBackspace),
                KeyCode::Delete => Some(Action::CommandDeleteChar),
                KeyCode::Left => Some(Action::CommandLineLeft),
                KeyCode::Right => Some(Action::CommandLineRight),
                KeyCode::Home => Some(Action::CommandLineStart),
                KeyCode::End => Some(Action::CommandLineEnd),
                KeyCode::Char(ch) => Some(Action::CommandChar(ch)),
                KeyCode::Up => Some(Action::CommandHistoryPrev),
                KeyCode::Down => Some(Action::CommandHistoryNext),
                _ => None,
            }
        }

        Mode::Search => {
            if raw_key.modifiers.contains(KeyModifiers::CONTROL) {
                return match raw_key.code {
                    KeyCode::Char('a') => Some(Action::CommandLineStart),
                    KeyCode::Char('e') => Some(Action::CommandLineEnd),
                    KeyCode::Char('b') => Some(Action::CommandLineLeft),
                    KeyCode::Char('f') => Some(Action::CommandLineRight),
                    KeyCode::Char('d') => Some(Action::CommandDeleteChar),
                    KeyCode::Char('k') => Some(Action::CommandLineKillToEnd),
                    KeyCode::Char('r') => Some(Action::CommandEnterRegisterMode),
                    KeyCode::Char('n') | KeyCode::Char('N') => Some(Action::CommandHistoryNext),
                    KeyCode::Char('p') | KeyCode::Char('P') => Some(Action::CommandHistoryPrev),
                    _ => None,
                };
            }
            if raw_key.modifiers.contains(KeyModifiers::ALT) {
                return match raw_key.code {
                    KeyCode::Char('d') | KeyCode::Char('D') => Some(Action::CommandClear),
                    _ => None,
                };
            }

            match raw_key.code {
                KeyCode::Esc => Some(Action::ExitMode),
                KeyCode::Enter => Some(Action::ExecuteSearch),
                KeyCode::Backspace => Some(Action::CommandBackspace),
                KeyCode::Delete => Some(Action::CommandDeleteChar),
                KeyCode::Left => Some(Action::CommandLineLeft),
                KeyCode::Right => Some(Action::CommandLineRight),
                KeyCode::Home => Some(Action::CommandLineStart),
                KeyCode::End => Some(Action::CommandLineEnd),
                KeyCode::Char(ch) => Some(Action::CommandChar(ch)),
                KeyCode::Up => Some(Action::CommandHistoryPrev),
                KeyCode::Down => Some(Action::CommandHistoryNext),
                _ => None,
            }
        }

        Mode::LlmPrompt => None,
        Mode::Normal => match raw_key.code {
            // Esc must always be able to clear multicursor state, even
            // when no config/default binding maps "esc" to anything in
            // Normal mode.
            KeyCode::Esc => Some(Action::ExitMode),
            _ => None,
        },
        Mode::Visual | Mode::VisualLine | Mode::VisualBlock => match key_str {
            "y" | "ctrl+c" => Some(Action::YankSelection),
            // "+" => Some(Action::YankToSystemClipboard),
            "d" | "x" | "delete" | "ctrl-x" => Some(Action::DeleteSelection),
            "c" => Some(Action::ChangeSelection),
            "I" => {
                if mode == Mode::VisualBlock {
                    Some(Action::VisualBlockInsert) // <- Shift-I triggers column-insert
                } else {
                    Some(Action::EnterInsertLineStart)
                }
            }
            "A" => {
                if mode == Mode::VisualBlock {
                    Some(Action::VisualBlockAppend) // <- Shift-A triggers column-append
                } else {
                    Some(Action::EnterInsertLineEnd)
                }
            }
            ">" => Some(Action::IndentSelection),
            "<" => Some(Action::OutdentSelection),
            "esc" => Some(Action::ExitMode),
            _ => None,
        },
    }
}

// ---------------------------------------------------------------------------
// Alt key helpers
// ---------------------------------------------------------------------------

/// Default Alt+key shortcuts in Insert mode.
/// Config bindings (e.g. `"insert+alt+d"`) take precedence — checked first
/// in `resolve_single_key`.
fn resolve_insert_alt_key(key: KeyEvent) -> Option<Action> {
    if key.modifiers.contains(KeyModifiers::CONTROL) {
        return None;
    }

    match key.code {
        KeyCode::Char('d') | KeyCode::Char('D') => Some(Action::DeleteCurrentLine),
        _ => None,
    }
}

/// Resolve an Alt+key combination in Brief mode.
fn resolve_brief_alt_key(key: KeyEvent) -> Option<Action> {
    if key.modifiers.contains(KeyModifiers::CONTROL) {
        return None;
    }

    match key.code {
        // ── File ────────────────────────────────
        KeyCode::Char('w') | KeyCode::Char('W') => Some(Action::Save),
        KeyCode::Char('q') | KeyCode::Char('Q') => Some(Action::Quit),
        KeyCode::Char('e') | KeyCode::Char('E') => Some(Action::FilePicker),
        KeyCode::Char('o') | KeyCode::Char('O') => Some(Action::SaveAs),

        // ── Movement ────────────────────────────
        KeyCode::Char('f') | KeyCode::Char('F') => Some(Action::MoveWordForward),
        KeyCode::Char('g') | KeyCode::Char('G') => Some(Action::EnterCommand),
        KeyCode::Char('j') | KeyCode::Char('J') => Some(Action::BookmarkGoto),
        KeyCode::Char('<') => Some(Action::MoveToFirstLine),
        KeyCode::Char('>') => Some(Action::MoveToLastLine),

        KeyCode::Char('s') | KeyCode::Char('S') => Some(Action::EnterSearch),

        // ── Editing ─────────────────────────────
        KeyCode::Char('d') | KeyCode::Char('D') => Some(Action::DeleteCurrentLine),
        KeyCode::Char('k') | KeyCode::Char('K') => Some(Action::DeleteToEndOfLine),
        KeyCode::Char('u') | KeyCode::Char('U') => Some(Action::Undo),
        KeyCode::Char('y') | KeyCode::Char('Y') => Some(Action::YankCurrentWord),
        KeyCode::Char('l') | KeyCode::Char('L') => Some(Action::BriefSelectionToggle),
        KeyCode::Char('c') | KeyCode::Char('C') => Some(Action::EnterVisualBlock),
        KeyCode::Char('m') | KeyCode::Char('M') => Some(Action::ToggleBookmarkAtCursor),

        // ── Window / Buffer ─────────────────────
        KeyCode::Char('b') | KeyCode::Char('B') => Some(Action::BufferList),
        KeyCode::Char('n') | KeyCode::Char('N') => Some(Action::FocusNextWindow),
        KeyCode::Char('1') => Some(Action::SwitchBuffer(0)),
        KeyCode::Char('2') => Some(Action::SwitchBuffer(1)),
        KeyCode::Char('3') => Some(Action::SwitchBuffer(2)),
        KeyCode::Char('4') => Some(Action::SwitchBuffer(3)),

        _ => None,
    }
}

// ---------------------------------------------------------------------------
// Recording Engine Helper Methods
// ---------------------------------------------------------------------------
fn enters_insert_mode(action: &Action) -> bool {
    if let Action::Chain(ref actions) = action {
        return actions.iter().any(|a| enters_insert_mode(a));
    }
    matches!(
        action,
        Action::EnterInsert
            | Action::EnterAppend
            | Action::EnterInsertLineStart
            | Action::EnterInsertLineEnd
            | Action::InsertNewlineBelow
            | Action::InsertNewlineAbove
            | Action::ChangeInsideWord
            | Action::ChangeInsideQuotes
            | Action::ChangeInsideParens
            | Action::ChangeInsideFunction
            | Action::ChangeInsideBraces
            | Action::ChangeInsideBrackets
            | Action::EnterBrief
    )
}

fn exits_insert_mode(action: &Action) -> bool {
    if let Action::Chain(ref actions) = action {
        return actions.iter().any(|a| exits_insert_mode(a));
    }
    matches!(action, Action::EnterNormal | Action::ExitMode)
}

// ---------------------------------------------------------------------------
// Execute Action
// ---------------------------------------------------------------------------

pub fn execute_action(editor: &mut Editor, action: Action) {
    log::debug!("execute_action: {:?}", action);

    // Reset failure flag for this action
    editor.action_failed = false;

    let register = editor.normal_register_prefix.take();

    // ── Consume count prefix ─────────────────────────────────────────
    let count = editor.current_count.max(1);
    editor.current_count = 0;
    if count > 1 {
        editor.clear_status_msg(); // Clear the "3" from the status bar
    }

    // ── Save jump position for big movements ─────────────────────────
    // Records where the cursor was *before* the jump so `` can ping-pong back.
    if action.is_jump() {
        editor.active_window_mut().save_jump_position();
    }

    let pre_captured_insert: Option<String> =
        if action == Action::ExitMode || action == Action::EnterNormal {
            editor.insert_buffer.clone()
        } else {
            None
        };

    // ── MASTER UNDO GUARD (FIXED) ─────────────────────────────────────
    let is_typing_mode = matches!(editor.mode(), Mode::Insert);

    // Expanded: cover ALL typing-like actions in Brief mode
    let is_brief_typing_action = editor.mode() == Mode::Brief
        && matches!(
            action,
            Action::InsertChar(_)
                | Action::InsertNewline
                | Action::InsertTab
                | Action::Backspace
                | Action::DeleteCharForward
        );

    // Only skip snapshots if we're CONTINUING an existing typing session.
    // The FIRST typing action after a destructive command (or after entering
    // Brief mode) must take a snapshot to anchor the undo block.
    let is_brief_typing_continuation = is_brief_typing_action && editor.insert_buffer.is_some();

    let is_entering_insert = if let Action::Chain(ref actions) = action {
        actions.iter().any(|a| enters_insert_mode(a))
    } else {
        matches!(
            action,
            Action::EnterInsert
                | Action::EnterAppend
                | Action::EnterInsertLineStart
                | Action::EnterInsertLineEnd
                | Action::InsertNewlineBelow
                | Action::InsertNewlineAbove
                | Action::VisualBlockInsert
                | Action::VisualBlockAppend
                | Action::ChangeSelection
                | Action::ChangeInsideWord
                | Action::ChangeInsideQuotes
                | Action::ChangeInsideParens
                | Action::ChangeInsideFunction
                | Action::ChangeInsideBraces
                | Action::ChangeInsideBrackets
                | Action::ChangeWordForward
                | Action::ChangeCurrentLine
                | Action::ChangeToEndOfLine
                | Action::EnterBrief
        )
    };

    let is_block_finalize = matches!(action, Action::ExitMode | Action::EnterNormal);
    let is_undo_or_repeat = matches!(action, Action::Undo | Action::RepeatLastChange);

    if !is_undo_or_repeat
        && !is_block_finalize
        && !is_brief_typing_continuation
        && (is_entering_insert || (!is_typing_mode && action.modifies_buffer()))
    {
        let (win, buf) = editor.active_window_and_buf_mut();
        buf.push_undo(win.row, win.col);
    }

    // ─────────────────────────────────────────────────────────────────

    // ── Brief Mode: Cancel selection on non-navigation actions ────────────
    if editor.mode() == Mode::Brief
        && editor.active_window().visual_anchor.is_some()
        && editor.visual_block_insert_state.is_none()
    {
        if !action_keeps_brief_selection(&action) {
            editor.active_window_mut().visual_anchor = None;
        }
    }

    // ── SAFETY: Prevent mutating actions on read-only special buffers ──────
    if action.modifies_buffer() && editor.buf().is_readonly() {
        editor.set_status_msg("Buffer is read-only", MessageKind::Error);
        editor.action_failed = true;
        return;
    }

    // Notify Git debounce engine of any buffer modification
    if action.modifies_buffer() {
        let buf_id = editor.buf().id;
        editor.git_debounce.notify_edit(buf_id);
        editor.invalidate_hunk_cache();
    }

    // ── ② Recording State Engine (FIXED) ──────────────────────────────
    if action != Action::RepeatLastChange {
        let mode = editor.mode();
        let modifies_buffer = action.modifies_buffer();

        // ① Auto-initialize insert_buffer for Brief mode typing
        if editor.insert_buffer.is_none() {
            let is_brief_typing_action = mode == Mode::Brief
                && matches!(
                    action,
                    Action::InsertChar(_)
                        | Action::InsertNewline
                        | Action::InsertTab
                        | Action::Backspace
                        | Action::DeleteCharForward
                );
            if enters_insert_mode(&action) || is_brief_typing_action {
                editor.insert_buffer = Some(String::new());
            }
        }

        if let Some(ref mut text) = editor.insert_buffer {
            match action {
                Action::InsertChar(ch) => text.push(ch),
                Action::InsertNewline => text.push('\n'),
                Action::InsertTab => text.push_str("    "),
                Action::Backspace => {
                    text.pop();
                }
                Action::EnterNormal | Action::ExitMode => {
                    let final_text = text.clone();
                    editor.record_action(RepeatableAction::Insert(final_text), 1);
                    editor.insert_buffer = None;
                }
                _ if mode == Mode::Brief && modifies_buffer => {
                    let final_text = text.clone();
                    if !final_text.is_empty() {
                        editor.record_action(RepeatableAction::Insert(final_text), 1);
                    }
                    editor.insert_buffer = None;
                }
                _ if enters_insert_mode(&action) => {
                    let final_text = text.clone();
                    if !final_text.is_empty() {
                        editor.record_action(RepeatableAction::Insert(final_text), 1);
                    }
                    editor.insert_buffer = None;
                }
                _ => {}
            }
        }

        // ── RECORD NON-INSERT MODIFICATIONS FOR DOT REPEAT ─────────────────
        // When the editor is NOT in an active insert session (no text buffered),
        // and the action is a non‑insert‑mode editing command that modifies the buffer,
        // we record it as a repeatable action. This allows the dot ('.') command
        // to replay deletions, line operations, indentation, etc. without needing
        // to emulate a full Insert‑mode sequence.
        //
        // Actions that *enter* insert mode (e.g., 'i', 'a', 'ciw') are handled
        // separately by the insert_buffer tracking logic above. This block only
        // captures atomic editing commands that are executed directly in Normal mode.
        if editor.insert_buffer.is_none() && !enters_insert_mode(&action) && modifies_buffer {
            match action {
                Action::Backspace => {
                    editor.record_action(
                        RepeatableAction::DeleteChars {
                            count: 1,
                            direction: DeleteDirection::Left,
                        },
                        count,
                    );
                }
                Action::DeleteCharForward => {
                    editor.record_action(
                        RepeatableAction::DeleteChars {
                            count: 1,
                            direction: DeleteDirection::Right,
                        },
                        count,
                    );
                }
                Action::DeleteCurrentLine => {
                    editor.record_action(RepeatableAction::DeleteLine, count);
                }
                Action::DeleteToEndOfLine => {
                    editor.record_action(RepeatableAction::DeleteToLineEnd, 1);
                }
                Action::DeleteInsideWord => {
                    editor.record_action(RepeatableAction::DeleteWordForward, count);
                }
                Action::DeleteWordForward => {
                    editor.record_action(RepeatableAction::DeleteWordForward, count);
                }
                Action::Paste => {
                    editor.record_action(
                        RepeatableAction::Paste {
                            register: '"',
                            after_cursor: true,
                        },
                        count,
                    );
                }
                _ => {}
            }
        }
    }

    // ── ③ Action Execution ─────────────────────────────────────────────────
    // When inside a visual-block insert session, suppress per-keystroke undo
    // snapshots. A single snapshot is taken during ExitMode replication.
    let in_block_insert = editor.visual_block_insert_state.is_some();
    match action {
        Action::CodeLlmChat => editor.open_codellm_chat_session(),
        Action::CodeLlmSend => editor.codellm_send(),
        // ── Chain: execute each sub-action unconditionally ────────
        Action::Chain(ref actions) => {
            for sub_action in actions.clone() {
                execute_action(editor, sub_action);
            }
        }

        // ── Then: execute each sub-action, stop on first failure ──
        Action::Then(ref actions) => {
            for sub_action in actions.clone() {
                editor.action_failed = false;
                let prev_kind = editor.status_kind;
                execute_action(editor, sub_action.clone());
                if editor.action_failed || editor.status_kind == MessageKind::Error {
                    break;
                }
            }
        }

        Action::EnterBrief => {
            editor.active_window_mut().extra_cursors.clear();
            editor.enter_brief();
        }
        Action::BriefSelectionToggle => {
            let win = editor.active_window_mut();
            if win.visual_anchor.is_some() {
                win.visual_anchor = None;
                editor.set_status_msg("Selection cancelled", MessageKind::Info);
            } else {
                win.visual_anchor = Some((win.row, win.col));
                editor.set_status_msg(
                    "Selection started. Navigate to extend, Ctrl+C to copy, Esc to cancel.",
                    MessageKind::Info,
                );
            }
        }

        // ---------------------------------------------------------------
        // Shift+Nav Selection Extenders
        // ---------------------------------------------------------------
        Action::ExtendSelectionLeft => {
            if editor.active_window().visual_anchor.is_none() {
                editor.active_window_mut().visual_anchor =
                    Some((editor.active_window().row, editor.active_window().col));
            }
            execute_action(editor, Action::MoveLeft);
        }
        Action::ExtendSelectionRight => {
            if editor.active_window().visual_anchor.is_none() {
                editor.active_window_mut().visual_anchor =
                    Some((editor.active_window().row, editor.active_window().col));
            }
            execute_action(editor, Action::MoveRight);
        }
        Action::ExtendSelectionUp => {
            if editor.active_window().visual_anchor.is_none() {
                editor.active_window_mut().visual_anchor =
                    Some((editor.active_window().row, editor.active_window().col));
            }
            execute_action(editor, Action::MoveUp);
        }
        Action::ExtendSelectionDown => {
            if editor.active_window().visual_anchor.is_none() {
                editor.active_window_mut().visual_anchor =
                    Some((editor.active_window().row, editor.active_window().col));
            }
            execute_action(editor, Action::MoveDown);
        }
        Action::ExtendSelectionWordForward => {
            if editor.active_window().visual_anchor.is_none() {
                editor.active_window_mut().visual_anchor =
                    Some((editor.active_window().row, editor.active_window().col));
            }
            execute_action(editor, Action::MoveWordForward);
        }
        Action::ExtendSelectionWordBackward => {
            if editor.active_window().visual_anchor.is_none() {
                editor.active_window_mut().visual_anchor =
                    Some((editor.active_window().row, editor.active_window().col));
            }
            execute_action(editor, Action::MoveWordBackward);
        }
        Action::ExtendSelectionLineStart => {
            if editor.active_window().visual_anchor.is_none() {
                editor.active_window_mut().visual_anchor =
                    Some((editor.active_window().row, editor.active_window().col));
            }
            execute_action(editor, Action::MoveLineStart);
        }
        Action::ExtendSelectionLineEnd => {
            if editor.active_window().visual_anchor.is_none() {
                editor.active_window_mut().visual_anchor =
                    Some((editor.active_window().row, editor.active_window().col));
            }
            execute_action(editor, Action::MoveLineEnd);
        }
        Action::ExtendSelectionToFirstLine => {
            if editor.active_window().visual_anchor.is_none() {
                editor.active_window_mut().visual_anchor =
                    Some((editor.active_window().row, editor.active_window().col));
            }
            execute_action(editor, Action::MoveToFirstLine);
        }
        Action::ExtendSelectionToLastLine => {
            if editor.active_window().visual_anchor.is_none() {
                editor.active_window_mut().visual_anchor =
                    Some((editor.active_window().row, editor.active_window().col));
            }
            execute_action(editor, Action::MoveToLastLine);
        }
        Action::ExtendSelectionPageUp => {
            if editor.active_window().visual_anchor.is_none() {
                editor.active_window_mut().visual_anchor =
                    Some((editor.active_window().row, editor.active_window().col));
            }
            execute_action(editor, Action::PageUp);
        }
        Action::ExtendSelectionPageDown => {
            if editor.active_window().visual_anchor.is_none() {
                editor.active_window_mut().visual_anchor =
                    Some((editor.active_window().row, editor.active_window().col));
            }
            execute_action(editor, Action::PageDown);
        }

        // ---------------------------------------------------------------
        // Visual Selection Operations
        // ---------------------------------------------------------------
        Action::AddCursorDown => {
            let (win, buf) = editor.active_window_and_buf_mut();
            let max_row = buf.len_lines().saturating_sub(1);
            let tab_size = buf.tab_size.max(1);
            // Respect the count prefix ("4C" => 4 new cursors on the 4
            // lines below, 5 total including the original). Stop instead
            // of stacking a duplicate cursor once we run out of lines —
            // that duplicate was causing double-typed text on the last line.
            //
            // Convert the primary cursor's char index to a visual column
            // so that cursors added on lines below stay visually aligned.
            // Using the raw char index directly causes the 'C' multicursor
            // to drift when lines contain tabs (a tab expands to a variable
            // width depending on the current column).
            let visual_col = {
                let line_text = buf.line_text(win.row);
                crate::ed::editing::visual_col_from_char_idx(&line_text, win.col, tab_size)
            };
            for _ in 0..count {
                let r = win.row;
                let c = win.col;
                if r >= max_row {
                    break;
                }
                win.extra_cursors.push((r, c));
                win.row = r + 1;
                // Convert visual column back to char index on the new line
                // so the cursor lands at the same screen column.
                let new_line = buf.line_text(win.row);
                let mut col = 0;
                let mut new_char_idx = new_line.chars().count();
                for (i, ch) in new_line.chars().enumerate() {
                    if col >= visual_col {
                        new_char_idx = i;
                        break;
                    }
                    match ch {
                        '\t' => col += tab_size - (col % tab_size),
                        _ => col += 1,
                    }
                }
                win.col = new_char_idx.min(buf.line_char_len(win.row));
            }
            win.desired_col = visual_col;
            editor.snap_cursor_to_viewport();
        }
        Action::EnterVisual => {
            editor.active_window_mut().extra_cursors.clear();
            if editor.mode() == Mode::Visual {
                editor.change_mode(Mode::Normal);
            } else {
                editor.change_mode(Mode::Visual);
            }
        }
        Action::EnterVisualLine => {
            editor.active_window_mut().extra_cursors.clear();
            if editor.mode() == Mode::VisualLine {
                editor.change_mode(Mode::Normal);
            } else {
                editor.change_mode(Mode::VisualLine);
            }
        }
        Action::EnterVisualBlock => {
            editor.active_window_mut().extra_cursors.clear();
            if editor.mode() == Mode::VisualBlock {
                editor.change_mode(Mode::Normal);
            } else {
                editor.change_mode(Mode::VisualBlock);
                editor.set_status_msg(
                    "Column selection started. Navigate to extend, Ctrl+C to copy, Esc to cancel.",
                    MessageKind::Info,
                );
            }
        }
        Action::VisualBlockInsert => execute_visual_block_edit(editor, false),
        Action::VisualBlockAppend => execute_visual_block_edit(editor, true),
        Action::YankSelection => execute_selection_op(editor, register, SelectionOp::Yank),
        Action::DeleteSelection => execute_selection_op(editor, register, SelectionOp::Delete),
        Action::ChangeSelection => execute_selection_op(editor, register, SelectionOp::Change),
        Action::IndentSelection => execute_indent_outdent(editor, count, false),
        Action::OutdentSelection => execute_indent_outdent(editor, count, true),

        // ---------------------------------------------------------------
        // Movement
        // ---------------------------------------------------------------
        Action::MoveLeft => {
            {
                apply_movement_to_all_cursors(editor, |w, b| {
                    for _ in 0..count {
                        movement::move_left(w, b);
                    }
                });
            }
            editor.snap_cursor_to_viewport();
            editor.comp.on_leave_insert();
        }
        Action::MoveRight => {
            {
                apply_movement_to_all_cursors(editor, |w, b| {
                    for _ in 0..count {
                        movement::move_right(w, b);
                    }
                });
            }
            editor.snap_cursor_to_viewport();
            editor.comp.on_leave_insert();
        }
        Action::MoveUp => {
            {
                apply_movement_to_all_cursors(editor, |w, b| {
                    for _ in 0..count {
                        movement::move_up(w, b);
                    }
                });
            }
            editor.snap_cursor_to_viewport();
            editor.comp.on_leave_insert();
        }
        Action::MoveDown => {
            {
                apply_movement_to_all_cursors(editor, |w, b| {
                    for _ in 0..count {
                        movement::move_down(w, b);
                    }
                });
            }
            editor.snap_cursor_to_viewport();
            editor.comp.on_leave_insert();
        }
        Action::MoveWordForward => {
            {
                apply_movement_to_all_cursors(editor, |w, b| {
                    for _ in 0..count {
                        movement::move_word_forward(w, b);
                    }
                });
            }
            editor.snap_cursor_to_viewport();
            editor.comp.on_leave_insert();
        }
        Action::MoveWordBackward => {
            {
                apply_movement_to_all_cursors(editor, |w, b| {
                    for _ in 0..count {
                        movement::move_word_backward(w, b);
                    }
                });
            }
            editor.snap_cursor_to_viewport();
            editor.comp.on_leave_insert();
        }
        Action::MoveLineStart => {
            let is_brief = editor.mode() == Mode::Brief;
            let tap_count = if is_brief {
                crate::keybind::brief_trackers::home_tap()
            } else {
                0
            };
            {
                apply_movement_to_all_cursors(editor, |w, b| {
                    if is_brief {
                        match tap_count {
                            0 => movement::move_line_start(w, b),
                            1 => {
                                w.row = w.scroll_line;
                                w.col = 0;
                                w.clamp_cursor(b);
                            }
                            _ => movement::move_to_first_line(w, b),
                        }
                    } else {
                        movement::move_line_start(w, b);
                    }
                });
            }
            editor.snap_cursor_to_viewport();
            editor.comp.on_leave_insert();
        }
        Action::MoveLineEnd => {
            let current_mode = editor.mode();
            let is_brief = current_mode == Mode::Brief;
            let tap_count = if is_brief {
                crate::keybind::brief_trackers::end_tap()
            } else {
                0
            };
            {
                apply_movement_to_all_cursors(editor, |w, b| {
                    if is_brief {
                        match tap_count {
                            0 => movement::move_line_end(w, b, current_mode),
                            1 => {
                                let last_visible = (w.scroll_line
                                    + w.position.height.saturating_sub(1))
                                .min(b.len_lines().saturating_sub(1));
                                w.row = last_visible;
                                w.col = editing::line_display_width(b, w.row).saturating_sub(1);
                                w.desired_col = w.col;
                                w.clamp_cursor(b);
                            }
                            _ => movement::move_to_last_line(w, b),
                        }
                    } else {
                        movement::move_line_end(w, b, current_mode);
                    }
                });
            }
            editor.snap_cursor_to_viewport();
            editor.comp.on_leave_insert();
        }
        Action::MoveToFirstLine => {
            {
                apply_movement_to_all_cursors(editor, |w, b| {
                    movement::move_to_first_line(w, b);
                });
            }
            editor.snap_cursor_to_viewport();
            editor.comp.on_leave_insert();
        }
        Action::MoveToLastLine => {
            {
                apply_movement_to_all_cursors(editor, |w, b| {
                    movement::move_to_last_line(w, b);
                });
            }
            editor.snap_cursor_to_viewport();
            editor.comp.on_leave_insert();
        }
        Action::PageUp => {
            {
                apply_movement_to_all_cursors(editor, |w, b| {
                    let jump = w.position.height.saturating_sub(2).max(1);
                    movement::page_up(w, b, jump);
                });
            }
            editor.snap_cursor_to_viewport();
            editor.comp.on_leave_insert();
        }
        Action::PageDown => {
            {
                apply_movement_to_all_cursors(editor, |w, b| {
                    let jump = w.position.height.saturating_sub(2).max(1);
                    movement::page_down(w, b, jump);
                });
            }
            editor.snap_cursor_to_viewport();
            editor.comp.on_leave_insert();
        }
        Action::ScrollCenter => {
            editor.center_viewport_on_cursor();
        }
        Action::CommandLineStart => {
            editor.set_command_cursor(0);
        }
        Action::CommandLineEnd => {
            editor.set_command_cursor(editor.command().len());
        }
        Action::CommandLineLeft => {
            if editor.command_cursor > 0 {
                let prev = editor.command()[..editor.command_cursor]
                    .char_indices()
                    .rev()
                    .next()
                    .map(|(b, _)| b)
                    .unwrap_or(0);
                editor.command_cursor = prev;
            }
        }
        Action::CommandLineRight => {
            let s = editor.command();
            if editor.command_cursor < s.len() {
                // advance by the byte length of the char under the cursor
                let n = s[editor.command_cursor..]
                    .chars()
                    .next()
                    .map(|c| c.len_utf8())
                    .unwrap_or(0);
                editor.command_cursor += n;
            }
        }
        Action::CommandDeleteChar => {
            if editor.command_cursor < editor.command().len() {
                editor.command.remove(editor.command_cursor);
            }
        }
        Action::CommandLineKillToEnd => {
            if editor.command_cursor < editor.command().len() {
                editor.command.truncate(editor.command_cursor);
            }
        }
        Action::CommandClear => {
            editor.clear_command();
        }

        // ---------------------------------------------------------------
        // Text Objects (Multi-cursor aware)
        // ---------------------------------------------------------------
        Action::DeleteInsideWord => {
            apply_to_all_cursors(editor, |win, buf| {
                for _ in 0..count {
                    let row = win.row;
                    let col = win.col;
                    let line_text = buf.line_text(row);
                    let chars: Vec<char> = line_text.chars().collect();
                    if chars.is_empty() {
                        break;
                    }
                    let c = col.min(chars.len().saturating_sub(1));
                    let is_word_char = |ch: char| ch.is_alphanumeric() || ch == '_';
                    let (start, end) = if is_word_char(chars[c]) {
                        let mut s = c;
                        while s > 0 && is_word_char(chars[s - 1]) {
                            s -= 1;
                        }
                        let mut e = c + 1;
                        while e < chars.len() && is_word_char(chars[e]) {
                            e += 1;
                        }
                        (s, e)
                    } else if chars[c].is_whitespace() {
                        let mut s = c;
                        while s > 0 && chars[s - 1].is_whitespace() {
                            s -= 1;
                        }
                        let mut e = c + 1;
                        while e < chars.len() && chars[e].is_whitespace() {
                            e += 1;
                        }
                        (s, e)
                    } else {
                        let ch = chars[c];
                        let mut s = c;
                        while s > 0 && chars[s - 1] == ch {
                            s -= 1;
                        }
                        let mut e = c + 1;
                        while e < chars.len() && chars[e] == ch {
                            e += 1;
                        }
                        (s, e)
                    };
                    let line_start = buf.rope.line_to_char(row);
                    let start_offset = line_start + start;
                    let end_offset = line_start + end;
                    if end_offset > start_offset && end_offset <= buf.rope.len_chars() {
                        buf.rope.remove(start_offset..end_offset);
                        win.row = row;
                        win.col = start;
                        win.col = win.col.min(buf.line_char_len(win.row));
                        buf.mark_modified();
                    }
                }
            });
        }
        Action::DeleteWordForward => {
            apply_to_all_cursors(editor, |win, buf| {
                for _ in 0..count {
                    let row = win.row;
                    let col = win.col;
                    let line_text = buf.line_text(row);
                    let chars: Vec<char> = line_text.chars().collect();
                    if chars.is_empty() {
                        break;
                    }
                    let c = col.min(chars.len().saturating_sub(1));
                    let is_word_char = |ch: char| ch.is_alphanumeric() || ch == '_';
                    let (start, end) = if is_word_char(chars[c]) {
                        let mut s = c;
                        while s > 0 && is_word_char(chars[s - 1]) {
                            s -= 1;
                        }
                        let mut e = c + 1;
                        while e < chars.len() && is_word_char(chars[e]) {
                            e += 1;
                        }
                        while e < chars.len() && chars[e].is_whitespace() {
                            e += 1;
                        }
                        (s, e)
                    } else if chars[c].is_whitespace() {
                        let mut s = c;
                        while s > 0 && chars[s - 1].is_whitespace() {
                            s -= 1;
                        }
                        let mut e = c + 1;
                        while e < chars.len() && chars[e].is_whitespace() {
                            e += 1;
                        }
                        (s, e)
                    } else {
                        let ch = chars[c];
                        let mut s = c;
                        while s > 0 && chars[s - 1] == ch {
                            s -= 1;
                        }
                        let mut e = c + 1;
                        while e < chars.len() && chars[e] == ch {
                            e += 1;
                        }
                        while e < chars.len() && chars[e].is_whitespace() {
                            e += 1;
                        }
                        (s, e)
                    };
                    let line_start = buf.rope.line_to_char(row);
                    let start_offset = line_start + start;
                    let end_offset = line_start + end;
                    if end_offset > start_offset && end_offset <= buf.rope.len_chars() {
                        buf.rope.remove(start_offset..end_offset);
                        win.row = row;
                        win.col = start;
                        win.col = win.col.min(buf.line_char_len(win.row));
                        buf.mark_modified();
                    }
                }
            });
        }
        Action::ChangeWordForward => {
            apply_to_all_cursors(editor, |win, buf| {
                let row = win.row;
                let col = win.col;
                let line_text = buf.line_text(row);
                let chars: Vec<char> = line_text.chars().collect();
                if chars.is_empty() {
                    return;
                }
                let c = col.min(chars.len().saturating_sub(1));
                let is_word_char = |ch: char| ch.is_alphanumeric() || ch == '_';
                let (start, end) = if is_word_char(chars[c]) {
                    let mut s = c;
                    while s > 0 && is_word_char(chars[s - 1]) {
                        s -= 1;
                    }
                    let mut e = c + 1;
                    while e < chars.len() && is_word_char(chars[e]) {
                        e += 1;
                    }
                    while e < chars.len() && chars[e].is_whitespace() {
                        e += 1;
                    }
                    (s, e)
                } else if chars[c].is_whitespace() {
                    let mut s = c;
                    while s > 0 && chars[s - 1].is_whitespace() {
                        s -= 1;
                    }
                    let mut e = c + 1;
                    while e < chars.len() && chars[e].is_whitespace() {
                        e += 1;
                    }
                    (s, e)
                } else {
                    let ch = chars[c];
                    let mut s = c;
                    while s > 0 && chars[s - 1] == ch {
                        s -= 1;
                    }
                    let mut e = c + 1;
                    while e < chars.len() && chars[e] == ch {
                        e += 1;
                    }
                    while e < chars.len() && chars[e].is_whitespace() {
                        e += 1;
                    }
                    (s, e)
                };
                let line_start = buf.rope.line_to_char(row);
                let start_offset = line_start + start;
                let end_offset = line_start + end;
                if end_offset > start_offset && end_offset <= buf.rope.len_chars() {
                    buf.rope.remove(start_offset..end_offset);
                    win.row = row;
                    win.col = start;
                    win.col = win.col.min(buf.line_char_len(win.row));
                    buf.mark_modified();
                }
            });
            editor.enter_insert();
        }
        Action::ChangeCurrentLine => {
            apply_to_all_cursors(editor, |win, buf| {
                if buf.len_lines() <= 1 {
                    buf.rope.remove(..buf.rope.len_chars());
                    buf.rope.insert(0, "\n");
                    win.col = 0;
                    win.desired_col = 0;
                    buf.mark_modified();
                    return;
                }
                let line_start = buf.rope.line_to_char(win.row);
                let next_line_start = if win.row + 1 < buf.len_lines() {
                    buf.rope.line_to_char(win.row + 1)
                } else {
                    buf.rope.len_chars()
                };
                if next_line_start > line_start {
                    buf.rope.remove(line_start..next_line_start);
                    buf.rope.insert(line_start, "\n");
                }
                win.col = 0;
                win.desired_col = 0;
                buf.mark_modified();
            });
            editor.enter_insert();
        }
        Action::ChangeToEndOfLine => {
            apply_to_all_cursors(editor, |win, buf| {
                let line_start = buf.rope.line_to_char(win.row);
                let line_char_len = buf.line_char_len(win.row);
                let del_start = line_start + win.col;
                let del_end = line_start + line_char_len;
                if del_start < del_end {
                    buf.rope.remove(del_start..del_end);
                }
                buf.mark_modified();
            });
            editor.enter_insert();
        }
        Action::ChangeInsideWord => {
            apply_to_all_cursors(editor, |win, buf| {
                for _ in 0..count {
                    let row = win.row;
                    let col = win.col;
                    let line_text = buf.line_text(row);
                    let chars: Vec<char> = line_text.chars().collect();
                    if chars.is_empty() {
                        break;
                    }
                    let c = col.min(chars.len().saturating_sub(1));
                    let is_word_char = |ch: char| ch.is_alphanumeric() || ch == '_';
                    let (start, end) = if is_word_char(chars[c]) {
                        let mut s = c;
                        while s > 0 && is_word_char(chars[s - 1]) {
                            s -= 1;
                        }
                        let mut e = c + 1;
                        while e < chars.len() && is_word_char(chars[e]) {
                            e += 1;
                        }
                        (s, e)
                    } else if chars[c].is_whitespace() {
                        let mut s = c;
                        while s > 0 && chars[s - 1].is_whitespace() {
                            s -= 1;
                        }
                        let mut e = c + 1;
                        while e < chars.len() && chars[e].is_whitespace() {
                            e += 1;
                        }
                        (s, e)
                    } else {
                        let ch = chars[c];
                        let mut s = c;
                        while s > 0 && chars[s - 1] == ch {
                            s -= 1;
                        }
                        let mut e = c + 1;
                        while e < chars.len() && chars[e] == ch {
                            e += 1;
                        }
                        (s, e)
                    };
                    let line_start = buf.rope.line_to_char(row);
                    let start_offset = line_start + start;
                    let end_offset = line_start + end;
                    if end_offset > start_offset && end_offset <= buf.rope.len_chars() {
                        buf.rope.remove(start_offset..end_offset);
                        win.row = row;
                        win.col = start;
                        win.col = win.col.min(buf.line_char_len(win.row));
                        buf.mark_modified();
                    }
                }
            });
            editor.enter_insert();
        }
        Action::DeleteInsideQuotes => {
            apply_to_all_cursors(editor, |win, buf| {
                let row = win.row;
                let col = win.col;
                if let Some((sr, sc, er, ec)) = buf.syntax.text_object_range(
                    row,
                    col,
                    crate::ed::syntax::TextObject::Quotes,
                    true,
                ) {
                    if sr >= buf.len_lines() || er >= buf.len_lines() {
                        return;
                    }
                    let start_offset = buf.rope.line_to_char(sr).saturating_add(sc);
                    let end_offset = buf.rope.line_to_char(er).saturating_add(ec);
                    if end_offset <= start_offset || end_offset > buf.rope.len_chars() {
                        return;
                    }
                    buf.rope.remove(start_offset..end_offset);
                    win.row = sr;
                    win.col = sc;
                    win.col = win.col.min(buf.line_char_len(win.row));
                    buf.mark_modified();
                }
            });
        }
        Action::ChangeInsideQuotes => {
            apply_to_all_cursors(editor, |win, buf| {
                let row = win.row;
                let col = win.col;
                if let Some((sr, sc, er, ec)) = buf.syntax.text_object_range(
                    row,
                    col,
                    crate::ed::syntax::TextObject::Quotes,
                    true,
                ) {
                    if sr >= buf.len_lines() || er >= buf.len_lines() {
                        return;
                    }
                    let start_offset = buf.rope.line_to_char(sr).saturating_add(sc);
                    let end_offset = buf.rope.line_to_char(er).saturating_add(ec);
                    if end_offset <= start_offset || end_offset > buf.rope.len_chars() {
                        return;
                    }
                    buf.rope.remove(start_offset..end_offset);
                    win.row = sr;
                    win.col = sc;
                    win.col = win.col.min(buf.line_char_len(win.row));
                    buf.mark_modified();
                }
            });
            editor.enter_insert();
        }
        Action::DeleteInsideParens => {
            apply_to_all_cursors(editor, |win, buf| {
                let row = win.row;
                let col = win.col;
                if let Some((sr, sc, er, ec)) = buf.syntax.text_object_range(
                    row,
                    col,
                    crate::ed::syntax::TextObject::Parens,
                    true,
                ) {
                    if sr >= buf.len_lines() || er >= buf.len_lines() {
                        return;
                    }
                    let start_offset = buf.rope.line_to_char(sr).saturating_add(sc);
                    let end_offset = buf.rope.line_to_char(er).saturating_add(ec);
                    if end_offset <= start_offset || end_offset > buf.rope.len_chars() {
                        return;
                    }
                    buf.rope.remove(start_offset..end_offset);
                    win.row = sr;
                    win.col = sc;
                    win.col = win.col.min(buf.line_char_len(win.row));
                    buf.mark_modified();
                }
            });
        }
        Action::ChangeInsideParens => {
            apply_to_all_cursors(editor, |win, buf| {
                let row = win.row;
                let col = win.col;
                if let Some((sr, sc, er, ec)) = buf.syntax.text_object_range(
                    row,
                    col,
                    crate::ed::syntax::TextObject::Parens,
                    true,
                ) {
                    if sr >= buf.len_lines() || er >= buf.len_lines() {
                        return;
                    }
                    let start_offset = buf.rope.line_to_char(sr).saturating_add(sc);
                    let end_offset = buf.rope.line_to_char(er).saturating_add(ec);
                    if end_offset <= start_offset || end_offset > buf.rope.len_chars() {
                        return;
                    }
                    buf.rope.remove(start_offset..end_offset);
                    win.row = sr;
                    win.col = sc;
                    win.col = win.col.min(buf.line_char_len(win.row));
                    buf.mark_modified();
                }
            });
            editor.enter_insert();
        }
        Action::DeleteInsideFunction => {
            apply_to_all_cursors(editor, |win, buf| {
                let row = win.row;
                let col = win.col;
                if let Some((sr, sc, er, ec)) = buf.syntax.text_object_range(
                    row,
                    col,
                    crate::ed::syntax::TextObject::Function,
                    true,
                ) {
                    if sr >= buf.len_lines() || er >= buf.len_lines() {
                        return;
                    }
                    let start_offset = buf.rope.line_to_char(sr).saturating_add(sc);
                    let end_offset = buf.rope.line_to_char(er).saturating_add(ec);
                    if end_offset <= start_offset || end_offset > buf.rope.len_chars() {
                        return;
                    }
                    buf.rope.remove(start_offset..end_offset);
                    win.row = sr;
                    win.col = sc;
                    win.col = win.col.min(buf.line_char_len(win.row));
                    buf.mark_modified();
                }
            });
        }
        Action::ChangeInsideFunction => {
            apply_to_all_cursors(editor, |win, buf| {
                let row = win.row;
                let col = win.col;
                if let Some((sr, sc, er, ec)) = buf.syntax.text_object_range(
                    row,
                    col,
                    crate::ed::syntax::TextObject::Function,
                    true,
                ) {
                    if sr >= buf.len_lines() || er >= buf.len_lines() {
                        return;
                    }
                    let start_offset = buf.rope.line_to_char(sr).saturating_add(sc);
                    let end_offset = buf.rope.line_to_char(er).saturating_add(ec);
                    if end_offset <= start_offset || end_offset > buf.rope.len_chars() {
                        return;
                    }
                    buf.rope.remove(start_offset..end_offset);
                    win.row = sr;
                    win.col = sc;
                    win.col = win.col.min(buf.line_char_len(win.row));
                    buf.mark_modified();
                }
            });
            editor.enter_insert();
        }
        Action::DeleteInsideBraces => {
            apply_to_all_cursors(editor, |win, buf| {
                let row = win.row;
                let col = win.col;
                if let Some((sr, sc, er, ec)) = buf.syntax.text_object_range(
                    row,
                    col,
                    crate::ed::syntax::TextObject::Braces,
                    true,
                ) {
                    if sr >= buf.len_lines() || er >= buf.len_lines() {
                        return;
                    }
                    let start_offset = buf.rope.line_to_char(sr).saturating_add(sc);
                    let end_offset = buf.rope.line_to_char(er).saturating_add(ec);
                    if end_offset <= start_offset || end_offset > buf.rope.len_chars() {
                        return;
                    }
                    buf.rope.remove(start_offset..end_offset);
                    win.row = sr;
                    win.col = sc;
                    win.col = win.col.min(buf.line_char_len(win.row));
                    buf.mark_modified();
                }
            });
        }
        Action::ChangeInsideBraces => {
            apply_to_all_cursors(editor, |win, buf| {
                let row = win.row;
                let col = win.col;
                if let Some((sr, sc, er, ec)) = buf.syntax.text_object_range(
                    row,
                    col,
                    crate::ed::syntax::TextObject::Braces,
                    true,
                ) {
                    if sr >= buf.len_lines() || er >= buf.len_lines() {
                        return;
                    }
                    let start_offset = buf.rope.line_to_char(sr).saturating_add(sc);
                    let end_offset = buf.rope.line_to_char(er).saturating_add(ec);
                    if end_offset <= start_offset || end_offset > buf.rope.len_chars() {
                        return;
                    }
                    buf.rope.remove(start_offset..end_offset);
                    win.row = sr;
                    win.col = sc;
                    win.col = win.col.min(buf.line_char_len(win.row));
                    buf.mark_modified();
                }
            });
            editor.enter_insert();
        }
        Action::DeleteInsideBrackets => {
            apply_to_all_cursors(editor, |win, buf| {
                let row = win.row;
                let col = win.col;
                if let Some((sr, sc, er, ec)) = buf.syntax.text_object_range(
                    row,
                    col,
                    crate::ed::syntax::TextObject::Brackets,
                    true,
                ) {
                    if sr >= buf.len_lines() || er >= buf.len_lines() {
                        return;
                    }
                    let start_offset = buf.rope.line_to_char(sr).saturating_add(sc);
                    let end_offset = buf.rope.line_to_char(er).saturating_add(ec);
                    if end_offset <= start_offset || end_offset > buf.rope.len_chars() {
                        return;
                    }
                    buf.rope.remove(start_offset..end_offset);
                    win.row = sr;
                    win.col = sc;
                    win.col = win.col.min(buf.line_char_len(win.row));
                    buf.mark_modified();
                }
            });
        }
        Action::ChangeInsideBrackets => {
            apply_to_all_cursors(editor, |win, buf| {
                let row = win.row;
                let col = win.col;
                if let Some((sr, sc, er, ec)) = buf.syntax.text_object_range(
                    row,
                    col,
                    crate::ed::syntax::TextObject::Brackets,
                    true,
                ) {
                    if sr >= buf.len_lines() || er >= buf.len_lines() {
                        return;
                    }
                    let start_offset = buf.rope.line_to_char(sr).saturating_add(sc);
                    let end_offset = buf.rope.line_to_char(er).saturating_add(ec);
                    if end_offset <= start_offset || end_offset > buf.rope.len_chars() {
                        return;
                    }
                    buf.rope.remove(start_offset..end_offset);
                    win.row = sr;
                    win.col = sc;
                    win.col = win.col.min(buf.line_char_len(win.row));
                    buf.mark_modified();
                }
            });
            editor.enter_insert();
        }
        Action::YankAroundFunction => {
            if let Err(msg) = check_around_function_safetynet(editor) {
                editor.set_status_msg(&msg, MessageKind::Error);
                return;
            }

            let (orig_row, orig_col) = {
                let win = editor.active_window();
                (win.row, win.col)
            };

            let info = editor.function_around_span_info();

            let info = if info.is_none() {
                let new_col = {
                    let buf = editor.buf();
                    if orig_row < buf.len_lines() {
                        let line = buf.line_text(orig_row);
                        line.chars().position(|c| !c.is_whitespace())
                    } else {
                        None
                    }
                };
                if let Some(col) = new_col {
                    if col != orig_col {
                        editor.active_window_mut().col = col;
                        let i = editor.function_around_span_info();
                        editor.active_window_mut().col = orig_col;
                        i
                    } else {
                        None
                    }
                } else {
                    None
                }
            } else {
                info
            };

            let info = if info.is_none() && orig_col > 0 {
                editor.active_window_mut().col = orig_col - 1;
                let i = editor.function_around_span_info();
                editor.active_window_mut().col = orig_col;
                i
            } else {
                info
            };

            {
                let win = editor.active_window_mut();
                win.row = orig_row;
                win.col = orig_col;
            }

            if let Some(info) = info {
                let text = {
                    let buf = editor.buf();
                    let start_char = buf.rope.line_to_char(info.start_row);
                    let end_row_exclusive = (info.end_row + 1).min(buf.len_lines());
                    let end_char = if end_row_exclusive < buf.len_lines() {
                        buf.rope.line_to_char(end_row_exclusive)
                    } else {
                        buf.rope.len_chars()
                    };
                    buf.rope.slice(start_char..end_char).to_string()
                };

                editor.yank_to_register(text, register);
            } else {
                editor.set_status_msg("No function found around cursor", MessageKind::Error);
            }
        }

        Action::YankInsideFunction => {
            let (row, col) = {
                let win = editor.active_window();
                (win.row, win.col)
            };

            if let Some((sr, sc, er, ec)) = editor.buf().syntax.text_object_range(
                row,
                col,
                crate::ed::syntax::TextObject::Function,
                true,
            ) {
                let buf = editor.buf();
                if sr >= buf.len_lines() || er >= buf.len_lines() {
                    editor.set_status_msg("Invalid text object range", MessageKind::Error);
                    return;
                }
                let start_offset = buf.rope.line_to_char(sr).saturating_add(sc);
                let end_offset = buf.rope.line_to_char(er).saturating_add(ec);

                if end_offset <= start_offset || end_offset > buf.rope.len_chars() {
                    editor.set_status_msg("Invalid text object range", MessageKind::Error);
                    return;
                }

                let text = buf.rope.slice(start_offset..end_offset).to_string();
                editor.yank_to_register(text, register);
            } else {
                editor.set_status_msg("No function found around cursor", MessageKind::Error);
            }
        }

        Action::DeleteAroundFunction => {
            if let Err(msg) = check_around_function_safetynet(editor) {
                editor.set_status_msg(&msg, MessageKind::Error);
                return;
            }

            let (orig_row, orig_col) = {
                let win = editor.active_window();
                (win.row, win.col)
            };

            // 1. Try exact cursor position
            let info = editor.function_around_span_info();

            // 2. If not found, try moving cursor to first non-whitespace char on the line
            // (handles cursor in leading whitespace before `pub fn`)
            let info = if info.is_none() {
                let new_col = {
                    let buf = editor.buf();
                    if orig_row < buf.len_lines() {
                        let line = buf.line_text(orig_row);
                        line.chars().position(|c| !c.is_whitespace())
                    } else {
                        None
                    }
                }; // buf dropped here

                if let Some(col) = new_col {
                    if col != orig_col {
                        let win = editor.active_window_mut();
                        win.col = col;
                        let i = editor.function_around_span_info();
                        let win = editor.active_window_mut();
                        win.col = orig_col;
                        i
                    } else {
                        None
                    }
                } else {
                    None
                }
            } else {
                info
            };

            // 3. If still not found, try one character back (e.g. cursor right after `}`)
            let info = if info.is_none() && orig_col > 0 {
                let win = editor.active_window_mut();
                win.col = orig_col - 1;
                let i = editor.function_around_span_info();
                let win = editor.active_window_mut();
                win.col = orig_col;
                i
            } else {
                info
            };

            // Ensure original cursor is restored before we continue
            {
                let win = editor.active_window_mut();
                win.row = orig_row;
                win.col = orig_col;
            }

            if let Some(info) = info {
                let deleted = {
                    let (win, buf) = editor.active_window_and_buf_mut();
                    let start_char = buf.rope.line_to_char(info.start_row);

                    // To include the closing brace and its trailing newline,
                    // we must delete up to the start of the line *after* end_row.
                    let end_row_exclusive = (info.end_row + 1).min(buf.len_lines());
                    let end_char = if end_row_exclusive < buf.len_lines() {
                        buf.rope.line_to_char(end_row_exclusive)
                    } else {
                        buf.rope.len_chars()
                    };

                    let text = buf.rope.slice(start_char..end_char).to_string();
                    buf.rope.remove(start_char..end_char);
                    buf.mark_modified();
                    buf.parse_syntax();
                    text
                }; // win and buf dropped here

                // Yank the deleted text via register system
                editor.yank_to_register(deleted, register);

                // Reposition cursor
                {
                    let (win, buf) = editor.active_window_and_buf_mut();
                    let max_row = buf.len_lines().saturating_sub(1);
                    win.row = info.start_row.min(max_row);
                    win.col = 0;
                    win.clamp_cursor(buf);
                    win.desired_col = win.col;
                }
                editor.record_action(RepeatableAction::DeleteAroundFunction, 1);
            } else {
                editor.set_status_msg("No function found around cursor", MessageKind::Error);
                editor.action_failed = true;
            }
        }
        // ---------------------------------------------------------------
        // Window management
        // ---------------------------------------------------------------
        Action::EnterWindowNav => {
            editor.window_nav_pending = true;
            editor.set_status_msg(
                "WINDOW: h/j/k/l=move  s=hsplit  v=vsplit  o=only  q=close  Esc=cancel",
                MessageKind::Info,
            );
        }
        Action::EnterCloseWindowNav => {
            editor.close_window_nav_pending = true;
            editor.set_status_msg(
                "CLOSE WINDOW: h/j/k/l=close dir  d/q=close current  Esc=cancel",
                MessageKind::Info,
            );
        }
        Action::CloseWindowLeft => {
            editor.close_window_left();
        }
        Action::CloseWindowRight => {
            editor.close_window_right();
        }
        Action::CloseWindowUp => {
            editor.close_window_up();
        }
        Action::CloseWindowDown => {
            editor.close_window_down();
        }
        Action::SplitHorizontal => {
            editor.split_horizontal();
        }
        Action::SplitVertical => {
            editor.split_vertical();
        }
        Action::CloseWindow => {
            editor.close_window(false);
        }
        Action::OnlyWindow => {
            editor.only_window();
        }
        Action::FocusNextWindow => {
            editor.focus_next_window();
        }
        Action::FocusPrevWindow => {
            editor.focus_prev_window();
        }
        Action::FocusWindowLeft => {
            editor.focus_window_left();
        }
        Action::FocusWindowRight => {
            editor.focus_window_right();
        }
        Action::FocusWindowUp => {
            editor.focus_window_up();
        }
        Action::FocusWindowDown => {
            editor.focus_window_down();
        }
        Action::SwitchBuffer(idx) => {
            editor.switch_buffer_by_index(idx);
        }
        Action::BufferNext => {
            editor.switch_next_buffer();
            editor.record_action(RepeatableAction::BufferNext, 1);
        }
        Action::BufferPrev => {
            editor.switch_prev_buffer();
            editor.record_action(RepeatableAction::BufferPrev, 1);
        }

        // ---------------------------------------------------------------
        // Editing
        // ---------------------------------------------------------------
        Action::Backspace => {
            let has_extra = !editor.active_window().extra_cursors.is_empty();
            let delete_range = if !has_extra {
                let (win, buf) = editor.active_window_and_buf_mut();
                let row = win.row;
                let col = win.col;
                if col > 0 {
                    let line_text = buf.line_text(row);
                    let mut char_idx = 0;
                    let mut prev_char_idx = 0;
                    for grapheme in line_text.graphemes(true) {
                        if char_idx == col {
                            break;
                        }
                        prev_char_idx = char_idx;
                        char_idx += grapheme.chars().count();
                    }
                    Some((row, prev_char_idx, row, col))
                } else {
                    None
                }
            } else {
                None
            };
            apply_to_all_cursors(editor, |win, buf| {
                editing::backspace(win, buf);
            });
            if has_extra {
                if let Some(filename) = editor.active_filename() {
                    editor.lsp_notify_change_full(std::path::PathBuf::from(filename));
                }
            } else {
                editor.lsp_notify_backspace(delete_range);
            }
            editor.on_completion_edit();
        }
        Action::DeleteCharForward => {
            let is_brief_selecting =
                editor.mode() == Mode::Brief && editor.active_window().visual_anchor.is_some();
            if is_brief_selecting {
                // Delete the selection (without yanking to clipboard)
                let mode = Mode::Visual;
                let range = {
                    let (win, buf) = editor.active_window_and_buf_mut();
                    win.get_selection_range(buf, mode)
                };
                if let Some((start_char, end_char)) = range {
                    let (win, buf) = editor.active_window_and_buf_mut();
                    buf.rope.remove(start_char..end_char);
                    buf.mark_modified();
                    let new_line = buf.rope.char_to_line(start_char);
                    win.row = new_line;
                    win.col = start_char.saturating_sub(buf.rope.line_to_char(new_line));
                    win.clamp_cursor(buf);
                    buf.parse_syntax();
                }
                editor.active_window_mut().visual_anchor = None;
            } else {
                let has_extra = !editor.active_window().extra_cursors.is_empty();
                let delete_range = if !has_extra {
                    let (win, buf) = editor.active_window_and_buf_mut();
                    let row = win.row;
                    let col = win.col;
                    let line_len = buf.line_char_len(row);
                    if col < line_len {
                        let line_text = buf.line_text(row);
                        let mut char_idx = 0;
                        let mut grapheme_len = 1;
                        for grapheme in line_text.graphemes(true) {
                            if char_idx == col {
                                grapheme_len = grapheme.chars().count();
                                break;
                            }
                            char_idx += grapheme.chars().count();
                        }
                        Some((row, col, row, col + grapheme_len))
                    } else {
                        None
                    }
                } else {
                    None
                };
                apply_to_all_cursors(editor, |win, buf| {
                    for _ in 0..count {
                        editing::delete_char_forward(win, buf);
                    }
                });
                if has_extra {
                    if let Some(filename) = editor.active_filename() {
                        editor.lsp_notify_change_full(std::path::PathBuf::from(filename));
                    }
                } else if count == 1 {
                    editor.lsp_notify_delete_forward(delete_range);
                }
            }
            if matches!(editor.mode(), Mode::Insert | Mode::Brief) && !is_brief_selecting {
                editor.on_completion_edit();
            } else {
                editor.comp.on_edit();
            }
        }
        Action::BriefCopySelection => {
            if editor.active_window().visual_anchor.is_some() {
                let range = {
                    let (win, buf) = editor.active_window_and_buf_mut();
                    win.get_selection_range(buf, Mode::Visual)
                };
                if let Some((start_char, end_char)) = range {
                    let text = editor.buf().rope.slice(start_char..end_char).to_string();
                    editor.yank_to_register(text, register);
                    editor.clipboard_is_block = false;
                    editor.set_status_msg("Copied selection", MessageKind::Info);
                }
                editor.active_window_mut().visual_anchor = None;
            }
            // If no selection, do nothing (NOP)
        }
        Action::BriefCutSelection => {
            if editor.active_window().visual_anchor.is_some() {
                let range = {
                    let (win, buf) = editor.active_window_and_buf_mut();
                    win.get_selection_range(buf, Mode::Visual)
                };
                if let Some((start_char, end_char)) = range {
                    let text = editor.buf().rope.slice(start_char..end_char).to_string();
                    editor.yank_to_register(text, register);
                    editor.clipboard_is_block = false;

                    let (win, buf) = editor.active_window_and_buf_mut();
                    buf.rope.remove(start_char..end_char);
                    buf.mark_modified();
                    let new_line = buf.rope.char_to_line(start_char);
                    win.row = new_line;
                    win.col = start_char.saturating_sub(buf.rope.line_to_char(new_line));
                    win.clamp_cursor(buf);
                    buf.parse_syntax();

                    // Notify LSP of cut
                    if let Some(filename) = editor.active_filename() {
                        editor.lsp_notify_change_full(std::path::PathBuf::from(filename));
                    }

                    editor.set_status_msg("Cut selection", MessageKind::Info);
                }
                editor.active_window_mut().visual_anchor = None;
                editor.on_completion_edit();
            }
        }
        Action::DeleteCurrentLine => {
            let has_extra = !editor.active_window().extra_cursors.is_empty();
            let mut deleted_text = String::new();
            if has_extra {
                apply_to_all_cursors(editor, |win, buf| {
                    editing::delete_current_line(win, buf);
                });
                let line_text = editor.line_text(editor.active_row());
                deleted_text.push_str(&line_text);
                deleted_text.push('\n');
            } else {
                for _ in 0..count {
                    let line_text = editor.line_text(editor.active_row());
                    deleted_text.push_str(&line_text);
                    deleted_text.push('\n');
                    let (win, buf) = editor.active_window_and_buf_mut();
                    editing::delete_current_line(win, buf);
                }
            }
            editor.yank_to_register(deleted_text, register);
            if let Some(filename) = editor.active_filename() {
                editor.lsp_notify_change_full(std::path::PathBuf::from(filename));
            }
            let (win, buf) = editor.active_window_and_buf_mut();
            win.col = win.col.min(editing::line_display_width(buf, win.row));
            editor.comp.on_edit();
        }
        Action::DeleteToEndOfLine => {
            apply_to_all_cursors(editor, |win, buf| {
                editing::delete_to_end_of_line(win, buf);
            });
            if let Some(filename) = editor.active_filename() {
                editor.lsp_notify_change_full(std::path::PathBuf::from(filename));
            }
            editor.comp.on_edit();
        }
        Action::InsertNewline => {
            apply_to_all_cursors(editor, |win, buf| {
                editing::insert_newline(win, buf);
            });
            let (win, buf) = editor.active_window_and_buf_mut();
            buf.parse_syntax();
            if let Some(filename) = editor.active_filename() {
                editor.lsp_notify_change_full(std::path::PathBuf::from(filename));
            }
            editor.on_completion_edit();
        }
        Action::InsertTab => {
            apply_to_all_cursors(editor, |win, buf| {
                editing::insert_tab(win, buf);
            });
            let (win, buf) = editor.active_window_and_buf_mut();
            buf.parse_syntax();
            if let Some(filename) = editor.active_filename() {
                editor.lsp_notify_change_full(std::path::PathBuf::from(filename));
            }
            editor.on_completion_edit();
        }
        Action::Undo => {
            editor.active_window_mut().extra_cursors.clear();
            let (win, buf) = editor.active_window_and_buf_mut();
            let (row, col) = (win.row, win.col); // Capture current cursor
            if let Some(snap) = buf.pop_undo(row, col) {
                buf.rope = snap.rope;
                buf.modified = snap.modified;
                buf.parse_syntax();
                win.row = snap.cursor_row;
                win.col = snap.cursor_col;
            }
            editor.comp.on_leave_insert();
        }
        // Add the Redo action handler
        Action::Redo => {
            editor.active_window_mut().extra_cursors.clear();
            let (win, buf) = editor.active_window_and_buf_mut();
            let (row, col) = (win.row, win.col); // Capture current cursor
            if let Some(snap) = buf.pop_redo(row, col) {
                buf.rope = snap.rope;
                buf.modified = snap.modified;
                buf.parse_syntax();
                win.row = snap.cursor_row;
                win.col = snap.cursor_col;
                editor.set_status_msg("Redo", MessageKind::Info);
            } else {
                editor.set_status_msg("Already at newest change", MessageKind::Info);
            }
            editor.comp.on_leave_insert();
        }
        Action::InsertChar(ch) => {
            let has_extra = !editor.active_window().extra_cursors.is_empty();
            apply_to_all_cursors(editor, |win, buf| {
                editing::insert_char(win, buf, ch);
            });
            if has_extra {
                if let Some(filename) = editor.active_filename() {
                    editor.lsp_notify_change_full(std::path::PathBuf::from(filename));
                }
            } else {
                editor.lsp_notify_insert_edit(ch);
            }
            editor.on_completion_edit();
        }
        // ---------------------------------------------------------------
        // Mode transitions
        // ---------------------------------------------------------------
        Action::EnterInsert => {
            editor.enter_insert();
        }
        Action::EnterAppend => {
            {
                apply_movement_to_all_cursors(editor, |w, b| {
                    let step = editing::grapheme_width_at_char(b, w.row, w.col);
                    let max_width = editing::line_display_width(b, w.row);
                    w.col = (w.col + step).min(max_width);
                });
            }
            editor.enter_insert();
        }
        Action::EnterInsertLineStart => {
            {
                apply_movement_to_all_cursors(editor, |w, b| {
                    movement::move_line_start(w, b);
                });
            }
            editor.enter_insert();
        }
        Action::EnterInsertLineEnd => {
            {
                apply_movement_to_all_cursors(editor, |w, b| {
                    movement::move_line_end(w, b, Mode::Insert);
                });
            }
            editor.enter_insert();
        }
        Action::InsertNewlineBelow => {
            apply_to_all_cursors(editor, |win, buf| {
                editing::insert_newline_below(win, buf);
            });
            let (win, buf) = editor.active_window_and_buf_mut();
            buf.parse_syntax();
            editor.enter_insert();
        }
        Action::InsertNewlineAbove => {
            apply_to_all_cursors(editor, |win, buf| {
                editing::insert_newline_above(win, buf);
            });
            let (win, buf) = editor.active_window_and_buf_mut();
            buf.parse_syntax();
            editor.enter_insert();
        }
        Action::EnterCommand => {
            let prev_mode = editor.mode();
            // Save the visual range BEFORE any anchor clearing
            let visual_range = if matches!(
                prev_mode,
                Mode::Visual | Mode::VisualLine | Mode::VisualBlock
            ) {
                editor.get_visual_line_range()
            } else {
                None
            };
            // Save it on the editor for use in command execution
            editor.saved_visual_range = visual_range;
            let visual_anchor = if matches!(
                prev_mode,
                Mode::Visual | Mode::VisualLine | Mode::VisualBlock
            ) {
                editor.active_window().visual_anchor
            } else {
                None
            };
            editor.active_window_mut().extra_cursors.clear();
            editor.enter_command();
            if visual_anchor.is_some() {
                editor.active_window_mut().visual_anchor = visual_anchor;
            }
            if matches!(
                prev_mode,
                Mode::Visual | Mode::VisualLine | Mode::VisualBlock
            ) {
                for ch in "'<,'>".chars() {
                    editor.push_command(ch);
                }
            }
        }
        Action::EnterNormal => {
            editor.finalize_visual_block_insert(pre_captured_insert.clone());
            if editor.mode() == Mode::Insert || editor.mode() == Mode::Brief {
                let (row, col) = {
                    let win = editor.active_window();
                    (win.row, win.col)
                };
                let buf = editor.buf();
                let max_col = editing::line_display_width(buf, row).saturating_sub(1);
                let new_col = if col > 0 { col - 1 } else { 0 };
                let win = editor.active_window_mut();
                win.col = new_col.min(max_col);
                win.desired_col = win.col;
            }
            editor.change_mode(Mode::Normal);
            editor.clear_status_msg();
            editor.active_window_mut().extra_cursors.clear();
        }
        Action::FilePicker => {
            let initial = editor
                .popup
                .last_file_picker_dir
                .clone()
                .unwrap_or_else(|| std::path::PathBuf::from("."));
            editor.popup.open_file_picker(&initial, false);
        }

        // ---------------------------------------------------------------
        // Vim Search Operations
        // ---------------------------------------------------------------
        Action::EnterSearch => {
            if editor.mode() != Mode::Search {
                editor.prev_mode = editor.mode();
            }
            editor.active_window_mut().extra_cursors.clear();
            editor.set_mode(Mode::Search);
        }
        Action::CancelSearch => {
            let target_mode = editor.prev_mode;
            editor.set_mode(target_mode);
        }
        Action::SearchCurrentWord => {
            if editor.mode() != Mode::Search {
                editor.prev_mode = editor.mode();
            }
            if let Some(word) = editor.get_word_under_cursor() {
                editor.last_search_query = Some(word.clone());
                editor.set_status_msg(&format!("/{}", word), MessageKind::Info);
                execute_action(editor, Action::SearchNext);
            } else {
                editor.set_status_msg("No word under cursor", MessageKind::Error);
            }
        }
        Action::ExecuteSearch => {
            let query = editor.command().to_string();
            let target_mode = editor.prev_mode;
            editor.set_mode(target_mode);
            if !query.is_empty() {
                // Save the search query to history so Up/Down can find it later
                editor.append_and_save_search_history(&query);

                editor.last_search_query = Some(query.clone());
                let (start_char, text) = {
                    let (win, buf) = editor.active_window_and_buf_mut();
                    (win.cursor_char_offset(buf), buf.rope.to_string())
                };
                let start_byte = text
                    .char_indices()
                    .nth(start_char)
                    .map(|(b, _)| b)
                    .unwrap_or(text.len());
                let mut found_char = text[start_byte..].find(&query).map(|rel_byte| {
                    let abs_byte = start_byte + rel_byte;
                    text[..abs_byte].chars().count()
                });
                let mut wrapped = false;
                if found_char.is_none() && editor.config.search_wrap_enabled {
                    found_char = text
                        .find(&query)
                        .map(|abs_byte| text[..abs_byte].chars().count());
                    wrapped = true;
                }
                if let Some(pos) = found_char {
                    if wrapped {
                        editor.set_status_msg(
                            "search hit BOTTOM, continuing at TOP",
                            MessageKind::Info,
                        );
                    }
                    let gutter = editor.active_gutter_width();
                    let (win, buf) = editor.active_window_and_buf_mut();
                    let row = buf.rope.char_to_line(pos);
                    win.row = row;
                    let line_text = buf.line_text(row);
                    let char_offset = pos - buf.rope.line_to_char(row);
                    win.col = char_offset;
                    win.desired_col = crate::ed::editing::visual_col_from_char_idx(
                        &line_text,
                        char_offset,
                        buf.tab_size,
                    );
                    let viewport_h = win.position.height;
                    let viewport_w = win.position.width;
                    win.scroll_to_cursor(viewport_h, viewport_w, gutter);
                } else {
                    let err_msg = if editor.config.search_wrap_enabled {
                        format!("Pattern not found: {}", query)
                    } else {
                        format!("Pattern not found (wrapscan disabled): {}", query)
                    };
                    editor.set_status_msg(&err_msg, MessageKind::Error);
                }
            }
        }
        Action::SearchNext => {
            if let Some(query) = editor.last_search_query.clone() {
                let (start_char, text) = {
                    let (win, buf) = editor.active_window_and_buf_mut();
                    let start = win
                        .cursor_char_offset(buf)
                        .saturating_add(1)
                        .min(buf.rope.len_chars());
                    (start, buf.rope.to_string())
                };
                let start_byte = text
                    .char_indices()
                    .nth(start_char)
                    .map(|(b, _)| b)
                    .unwrap_or(text.len());
                let mut found_char = text[start_byte..].find(&query).map(|rel_byte| {
                    let abs_byte = start_byte + rel_byte;
                    text[..abs_byte].chars().count()
                });
                let mut wrapped = false;
                if found_char.is_none() {
                    if editor.config.search_wrap_enabled {
                        found_char = text
                            .find(&query)
                            .map(|abs_byte| text[..abs_byte].chars().count());
                        wrapped = true;
                    } else {
                        editor.set_status_msg(
                            "search hit BOTTOM, wrapscan disabled",
                            MessageKind::Error,
                        );
                    }
                }
                if let Some(pos) = found_char {
                    if wrapped {
                        editor.set_status_msg(
                            "search hit BOTTOM, continuing at TOP",
                            MessageKind::Info,
                        );
                    }
                    let gutter = editor.active_gutter_width();
                    let (win, buf) = editor.active_window_and_buf_mut();
                    let row = buf.rope.char_to_line(pos);
                    win.row = row;
                    let line_text = buf.line_text(row);
                    let char_offset = pos - buf.rope.line_to_char(row);
                    win.col = char_offset;
                    win.desired_col = crate::ed::editing::visual_col_from_char_idx(
                        &line_text,
                        char_offset,
                        buf.tab_size,
                    );
                    let viewport_h = win.position.height;
                    let viewport_w = win.position.width;
                    win.scroll_to_cursor(viewport_h, viewport_w, gutter);
                }
            }
        }
        Action::SearchPrev => {
            if let Some(query) = editor.last_search_query.clone() {
                let (start_char, text) = {
                    let (win, buf) = editor.active_window_and_buf_mut();
                    let start = win.cursor_char_offset(buf).saturating_sub(1);
                    (start, buf.rope.to_string())
                };
                let start_byte = text
                    .char_indices()
                    .nth(start_char)
                    .map(|(b, _)| b)
                    .unwrap_or(text.len());
                let mut found_char = text[..start_byte]
                    .rfind(&query)
                    .map(|abs_byte| text[..abs_byte].chars().count());
                let mut wrapped = false;
                if found_char.is_none() {
                    if editor.config.search_wrap_enabled {
                        found_char = text
                            .rfind(&query)
                            .map(|abs_byte| text[..abs_byte].chars().count());
                        wrapped = true;
                    } else {
                        editor.set_status_msg(
                            "search hit TOP, wrapscan disabled",
                            MessageKind::Error,
                        );
                    }
                }
                if let Some(pos) = found_char {
                    if wrapped {
                        editor.set_status_msg(
                            "search hit TOP, continuing at BOTTOM",
                            MessageKind::Info,
                        );
                    }
                    let gutter = editor.active_gutter_width();
                    let (win, buf) = editor.active_window_and_buf_mut();
                    let row = buf.rope.char_to_line(pos);
                    win.row = row;
                    let line_text = buf.line_text(row);
                    let char_offset = pos - buf.rope.line_to_char(row);
                    win.col = char_offset;
                    win.desired_col = crate::ed::editing::visual_col_from_char_idx(
                        &line_text,
                        char_offset,
                        buf.tab_size,
                    );
                    let viewport_h = win.position.height;
                    let viewport_w = win.position.width;
                    win.scroll_to_cursor(viewport_h, viewport_w, gutter);
                }
            }
        }
        Action::Mru => {
            editor.open_mru_popup(true);
        }
        Action::FunctionList => {
            let entries = crate::popup::function_list::extract_functions(editor.buf());
            editor.popup.function_list =
                Some(crate::popup::function_list::FunctionListPopup::new(entries));
        }
        Action::Guide => {
            editor.open_guide_popup();
        }
        Action::HunkNext => {
            editor.jump_to_next_hunk();
        }
        Action::HunkPrev => {
            editor.jump_to_prev_hunk();
        }
        Action::GitRevert => {
            editor.revert_hunk();
        }
        Action::HunkPopup => {
            editor.open_hunk_popup();
        }
        Action::ToggleGitBlame => {
            editor.toggle_git_blame();
        }
        Action::GitLog => {
            editor.open_git_log(None);
        }
        Action::BufferList => {
            editor.trigger_buffer_list_popup();
        }
        Action::LastRg => {
            editor.ripgrep_last();
        }
        Action::RgUnderCursor => {
            editor.ripgrep_under_cursor();
        }
        Action::BookMarks => {
            editor.open_marks_popup();
        }
        Action::QuitAll => {
            editor.quit_all_check();
        }
        Action::BufferClose => {
            editor.close_buffer();
        }
        Action::ForceQuitAll => {
            editor.force_quit();
        }
        Action::BookmarkSet => {
            editor.pending_input = PendingInput::SetBookmark;
            editor.set_status_msg("Mark: press a letter (a-z)", MessageKind::Info);
        }
        Action::BookmarkGoto => {
            if editor.config.bookmark_popup_goto {
                // Rich UI mode: open the popup immediately
                editor.open_marks_popup();
            } else {
                // Vim mode: wait for the next key (a-z or `)
                editor.pending_input = PendingInput::GotoBookmark;
                editor.set_status_msg(
                    "Mark: press a letter (a-z) or ` to jump back",
                    MessageKind::Info,
                );
            }
        }
        Action::JumpLastPosition => {
            editor.jump_last_position();
        }
        Action::EnterLlmPrompt => {
            editor.set_mode(Mode::LlmPrompt);
            editor.llm.prompt.clear();
            editor.set_status_msg("LLM Prompt: ", MessageKind::Info);
        }
        Action::GitStatus => {
            editor.open_git_status();
        }

        // ---------------------------------------------------------------------------
        // Action::YankToSystemClipboard  (read-only, no cursor change needed)
        // ---------------------------------------------------------------------------
        Action::YankToSystemClipboard => {
            editor.yank_to_system_clipboard();
        }

        // ---------------------------------------------------------------------------
        // Action::PasteFromSystemClipboard
        // ---------------------------------------------------------------------------
        Action::PasteFromSystemClipboard => {
            let bid = editor.buf().id;
            let (start_row, start_col) = {
                let win = editor.active_window();
                (win.row, win.col)
            };

            editor.paste_from_system_clipboard();

            // Clamp first (fixes the 20 000-col bug), then snap viewport.
            clamp_cursor_after_paste(editor);
            editor.buf_mut().parse_syntax();
            editor.snap_cursor_to_viewport();
            editor.git_debounce.notify_edit(bid);
            let _ = (start_row, start_col); // no longer needed; kept to avoid refactor noise
        }

        // ---------------------------------------------------------------------------
        // Action::CutToSystemClipboard
        // ---------------------------------------------------------------------------
        Action::CutToSystemClipboard => {
            let bid = editor.buf().id;
            editor.cut_to_system_clipboard();
            clamp_cursor_after_paste(editor);
            editor.buf_mut().parse_syntax();
            editor.snap_cursor_to_viewport();
            editor.git_debounce.notify_edit(bid);
        }

        // ---------------------------------------------------------------------------
        // Action::YankWordToSystemClipboard  (read-only)
        // ---------------------------------------------------------------------------
        Action::YankWordToSystemClipboard => {
            editor.yank_word_to_system_clipboard();
        }

        // ---------------------------------------------------------------------------
        // Action::PutFromSystemClipboardBelow
        // ---------------------------------------------------------------------------
        Action::PutFromSystemClipboardBelow => {
            let bid = editor.buf().id;
            editor.put_from_system_clipboard_below();

            // Clamp, then position the cursor at the start of the inserted line.
            clamp_cursor_after_paste(editor);
            {
                let win = editor.active_window_mut();
                win.col = 0;
                win.desired_col = 0;
            }
            editor.buf_mut().parse_syntax();
            editor.snap_cursor_to_viewport();
            editor.git_debounce.notify_edit(bid);
        }

        // ---------------------------------------------------------------------------
        // Action::ClipboardReplaceBuffer
        // ---------------------------------------------------------------------------
        Action::ClipboardReplaceBuffer => {
            let bid = editor.buf().id;
            {
                let (win, buf) = editor.active_window_and_buf_mut();
                let total_chars = buf.rope.len_chars();
                if total_chars > 0 {
                    buf.rope.remove(0..total_chars);
                }
                win.row = 0;
                win.col = 0;
                win.desired_col = 0;
                buf.mark_modified();
            }

            editor.paste_from_system_clipboard();

            // ── FIX: enforce trailing-newline invariant ───────────────
            // Without this the rope can end without '\n', which causes
            // line_text / line_char_len to strip a real character on the
            // last line.  After :save the file lacks '\n'; on next open,
            // open_file appends one → visible "ghost" extra line.
            {
                let buf = editor.buf_mut();
                let len = buf.rope.len_chars();
                if len == 0 || buf.rope.char(len - 1) != '\n' {
                    buf.rope.insert(len, "\n");
                    buf.mark_modified();
                }
            }

            {
                let win = editor.active_window_mut();
                win.row = 0;
                win.col = 0;
                win.desired_col = 0;
            }
            clamp_cursor_after_paste(editor);
            editor.buf_mut().parse_syntax();
            editor.snap_cursor_to_viewport();
            editor.git_debounce.notify_edit(bid);
        }
        Action::EnterCommandPalette => {
            let entries = crate::popup::command_palette::build_command_entries();
            editor.popup.open_command_palette(entries);
        }
        Action::ClearSearchHighlight => {
            editor.last_search_query = None;
            editor.buf_mut().search_pattern = None;
            editor.set_status_msg("Search highlight cleared", MessageKind::Info);
        }
        Action::MatchBracket => {
            let (row, col) = {
                let win = editor.active_window();
                (win.row, win.col)
            };

            // 1. Try Tree-sitter first (100% accurate, ignores strings/comments)
            let ts_match = editor.buf().syntax.find_matching_bracket(row, col);

            if let Some((target_row, target_col)) = ts_match {
                let win = editor.active_window_mut();
                win.row = target_row;
                win.col = target_col;
                win.desired_col = target_col;
            } else {
                // 2. Fallback: If tree-sitter fails (e.g. no grammar loaded),
                // you can optionally call a traditional scanner here.
                // editor.find_matching_bracket_fallback();
                editor.set_status_msg("No matching bracket found", MessageKind::Info);
                editor.action_failed = true;
            }

            editor.snap_cursor_to_viewport();
        }
        Action::ToggleComment => {
            let lang = editor
                .buf()
                .syntax
                .language_id
                .as_deref()
                .unwrap_or("unknown");
            let prefix = crate::ed::misc_helper::comment_prefix_for_lang(lang);
            let prefix_with_space = format!("{} ", prefix);

            let (win, buf) = editor.active_window_and_buf_mut();
            let row = win.row;

            if row >= buf.len_lines() {
                return;
            }

            let start_char = buf.rope.line_to_char(row);
            let is_last_line = row + 1 >= buf.len_lines();
            let end_char = if is_last_line {
                buf.rope.len_chars()
            } else {
                buf.rope.line_to_char(row + 1)
            };

            // Extract current line text without the trailing newline
            let current_line = buf.rope.slice(start_char..end_char).to_string();
            let line_without_newline = current_line.trim_end_matches('\n');

            let trimmed = line_without_newline.trim_start();
            let leading_ws_len = line_without_newline.len() - trimmed.len();
            let leading_ws = &line_without_newline[..leading_ws_len];

            let new_content = if trimmed.starts_with(prefix_with_space.as_str()) {
                // Remove comment: "// " -> ""
                format!("{}{}", leading_ws, &trimmed[prefix_with_space.len()..])
            } else if trimmed.starts_with(prefix) {
                // Remove comment: "//" -> "" (no trailing space after prefix)
                format!("{}{}", leading_ws, &trimmed[prefix.len()..])
            } else if trimmed.is_empty() {
                // Empty line: just insert the prefix
                format!("{}{}", leading_ws, prefix)
            } else {
                // Add comment: "code" -> "// code"
                format!("{}{}{}", leading_ws, prefix_with_space, trimmed)
            };

            // Replace line in rope
            buf.rope.remove(start_char..end_char);

            let replacement = if is_last_line {
                new_content
            } else {
                format!("{}\n", new_content)
            };

            buf.rope.insert(start_char, &replacement);
            buf.mark_modified();
            buf.parse_syntax();

            // Move to next line (clamped to the end of the buffer)
            let max_row = buf.len_lines().saturating_sub(1);
            win.row = (row + 1).min(max_row);
            // Optional: Reset column to 0 or first non-blank character
            win.col = 0;
            win.desired_col = 0;

            editor.comp.on_edit();
        }
        Action::CommandEnterRegisterMode => {
            editor.pending_register = true;
        }
        Action::CommandInsertFilename => {
            let text = editor.active_filename().unwrap_or("").to_string();
            editor.insert_command_text(&text);
            editor.pending_register = false;
        }
        Action::CommandInsertWord => {
            let text = editor.get_word_under_cursor().unwrap_or_default();
            editor.insert_command_text(&text);
            editor.pending_register = false;
        }
        Action::CommandInsertLine => {
            // Vim uses 1-based line numbers for : commands
            let row = editor.active_row() + 1;
            editor.insert_command_text(&row.to_string());
            editor.pending_register = false;
        }
        Action::CommandCancelRegister => {
            editor.pending_register = false;
        }
        Action::DeleteToEndOfFile => {
            // Save current position
            let start_row = editor.active_window().row;
            let start_col = editor.active_window().col;

            // Apply count prefix: move cursor down (count-1) lines first,
            // but never beyond the last line.
            if count > 1 {
                let (win, buf) = editor.active_window_and_buf_mut();
                let target_row = (start_row + count - 1).min(buf.len_lines().saturating_sub(1));
                win.row = target_row;
                win.col = win.col.min(editing::line_display_width(buf, target_row));
            }

            // Now delete from current line to end of file.
            let (win, buf) = editor.active_window_and_buf_mut();
            let from_row = win.row;
            let total_lines = buf.len_lines();
            if from_row >= total_lines {
                // Already at EOF – nothing to delete.
                return;
            }

            let start_char = buf.rope.line_to_char(from_row);
            let end_char = buf.rope.len_chars(); // to end of buffer

            let deleted = buf.rope.slice(start_char..end_char).to_string();
            buf.rope.remove(start_char..end_char);
            buf.mark_modified();

            // Yank into default register
            // editor.clipboard = Some(deleted);
            // editor.clipboard_is_block = false;

            // Put cursor at the first line (now EOF) and column 0
            win.row = from_row.min(buf.len_lines().saturating_sub(1));
            win.col = 0;
            win.desired_col = 0;
            win.clamp_cursor(buf);

            buf.parse_syntax();
            editor.comp.on_edit();

            // Record for dot repeat (if needed)
            editor.record_action(RepeatableAction::DeleteToEndOfFile, 1);
        }
        // ---------------------------------------------------------------
        // Tag navigation
        // ---------------------------------------------------------------
        Action::TagJump => {
            editor.tag_under_cursor();
        }
        Action::TagBack => {
            editor.tag_back();
        }
        // tag_fd_action_fdsearch
        Action::FdSearch => {
            let root = crate::git::gutter::find_git_root(&std::path::PathBuf::from(
                editor.active_filename().unwrap_or("."),
            ))
            .unwrap_or_else(|| std::path::PathBuf::from("."));

            editor.popup.open_fd(&root, "");
        }
        Action::ManualComplete => {
            editor.trigger_manual_completion();
        }
        Action::SearchSymbols => {
            // Use the word under cursor as the initial query.
            if let Some(word) = editor.get_word_under_cursor() {
                editor.symbols_search(&word);
            } else {
                editor.set_status_msg(
                    "No word under cursor. Use :symbols <query>",
                    MessageKind::Info,
                );
            }
        }
        Action::EasyMotion => {
            editor.enter_easymotion();
        }
        Action::ToggleTrueFalse => {
            let (row, col) = {
                let win = editor.active_window();
                (win.row, win.col)
            };

            if let Some(word) = editor.get_word_under_cursor() {
                let replacement = match word.as_str() {
                    "true" => Some("false"),
                    "false" => Some("true"),
                    "True" => Some("False"),
                    "False" => Some("True"),
                    "TRUE" => Some("FALSE"),
                    "FALSE" => Some("TRUE"),
                    _ => None,
                };

                if let Some(new_word) = replacement {
                    let buf = editor.buf();
                    let line = buf.line_text(row);
                    let chars: Vec<char> = line.chars().collect();

                    // Find word boundaries
                    let word_start = {
                        let mut s = col;
                        while s > 0
                            && (chars
                                .get(s - 1)
                                .map_or(false, |c| c.is_alphanumeric() || *c == '_'))
                        {
                            s -= 1;
                        }
                        s
                    };
                    let word_end = {
                        let mut e = col;
                        while e < chars.len()
                            && (chars
                                .get(e)
                                .map_or(false, |c| c.is_alphanumeric() || *c == '_'))
                        {
                            e += 1;
                        }
                        e
                    };

                    let (win, buf) = editor.active_window_and_buf_mut();
                    let line_start = buf.rope.line_to_char(row);
                    let start_offset = line_start + word_start;
                    let end_offset = line_start + word_end;

                    if end_offset <= buf.rope.len_chars() {
                        buf.rope.remove(start_offset..end_offset);
                        buf.rope.insert(start_offset, new_word);
                        buf.mark_modified();
                        buf.parse_syntax();
                        // Position cursor at last char of the new word
                        win.col = word_start + new_word.chars().count().saturating_sub(1);
                        win.desired_col = win.col;
                    }

                    editor.comp.on_edit();
                } else {
                    editor.set_status_msg(
                        &format!("'{}' is not a boolean", word),
                        MessageKind::Error,
                    );
                }
            } else {
                editor.set_status_msg("No word under cursor", MessageKind::Error);
            }
        }
        Action::SwissKnife => {
            let (row, col) = {
                let win = editor.active_window();
                (win.row, win.col)
            };

            // ── Job 1: Toggle boolean under cursor ────────────────
            if let Some(word) = editor.get_word_under_cursor() {
                let replacement = match word.as_str() {
                    "true" => Some("false"),
                    "false" => Some("true"),
                    "True" => Some("False"),
                    "False" => Some("True"),
                    "TRUE" => Some("FALSE"),
                    "FALSE" => Some("TRUE"),
                    _ => None,
                };

                if let Some(new_word) = replacement {
                    let buf = editor.buf();
                    let line = buf.line_text(row);
                    let chars: Vec<char> = line.chars().collect();

                    let word_start = {
                        let mut s = col;
                        while s > 0
                            && chars
                                .get(s - 1)
                                .map_or(false, |c| c.is_alphanumeric() || *c == '_')
                        {
                            s -= 1;
                        }
                        s
                    };
                    let word_end = {
                        let mut e = col;
                        while e < chars.len()
                            && chars
                                .get(e)
                                .map_or(false, |c| c.is_alphanumeric() || *c == '_')
                        {
                            e += 1;
                        }
                        e
                    };

                    let (win, buf) = editor.active_window_and_buf_mut();
                    let line_start = buf.rope.line_to_char(row);
                    let start_offset = line_start + word_start;
                    let end_offset = line_start + word_end;

                    if end_offset <= buf.rope.len_chars() {
                        buf.rope.remove(start_offset..end_offset);
                        buf.rope.insert(start_offset, new_word);
                        buf.mark_modified();
                        buf.parse_syntax();
                        win.col = word_start + new_word.chars().count().saturating_sub(1);
                        win.desired_col = win.col;
                    }
                    editor.comp.on_edit();
                    return; // Handled
                }
            }

            // ── Job 2: Insert placeholder on empty line ────────────
            {
                let buf = editor.buf();
                let line = buf.line_text(row);
                let trimmed = line.trim_end_matches('\n').trim_end_matches('\r');

                if trimmed.trim_start().is_empty() {
                    // Scan upward for the last non-empty line
                    let mut up_indent = String::new();
                    let mut up_ends_with_close = false;
                    for scan_row in (0..row).rev() {
                        let scan_line = buf.line_text(scan_row);
                        let scan_trimmed = scan_line.trim_end_matches('\n').trim_end_matches('\r');
                        if !scan_trimmed.is_empty() {
                            up_indent = scan_trimmed
                                .chars()
                                .take_while(|c| c.is_whitespace())
                                .collect();
                            let last_char = scan_trimmed.chars().last().unwrap_or(' ');
                            up_ends_with_close =
                                last_char == '}' || last_char == ')' || last_char == ']';
                            break;
                        }
                    }

                    // Scan downward for the next non-empty line
                    let mut down_indent: Option<String> = None;
                    for scan_row in (row + 1)..buf.len_lines() {
                        let scan_line = buf.line_text(scan_row);
                        let scan_trimmed = scan_line.trim_end_matches('\n').trim_end_matches('\r');
                        if !scan_trimmed.is_empty() {
                            down_indent = Some(
                                scan_trimmed
                                    .chars()
                                    .take_while(|c| c.is_whitespace())
                                    .collect(),
                            );
                            break;
                        }
                    }

                    // Pick the correct indent:
                    // - If line above ends with } ) ] → use the indent from below
                    //   (we exited a block, align with the enclosing level)
                    // - Otherwise → use the indent from above
                    //   (we're inside a block, align with siblings)
                    let indent = if up_ends_with_close {
                        down_indent.unwrap_or(up_indent)
                    } else {
                        // If current line has MORE whitespace than the parent,
                        // prefer the current line's indent (deeper nesting).
                        let current_ws: String =
                            trimmed.chars().take_while(|c| c.is_whitespace()).collect();
                        if current_ws.len() > up_indent.len() {
                            current_ws
                        } else {
                            up_indent
                        }
                    };

                    let placeholder = format!("{}//--  (anchor dont removed) --//", indent);

                    let (win, buf) = editor.active_window_and_buf_mut();

                    let line_start = buf.rope.line_to_char(row);
                    let line_end = if row + 1 < buf.len_lines() {
                        buf.rope.line_to_char(row + 1)
                    } else {
                        buf.rope.len_chars()
                    };

                    buf.rope.remove(line_start..line_end);
                    let insert_text = format!("{}\n", placeholder);
                    buf.rope.insert(line_start, &insert_text);
                    buf.mark_modified();
                    buf.parse_syntax();

                    // Position cursor right after '[ '
                    win.col = indent.chars().count() + 5;
                    win.desired_col = win.col;

                    editor.comp.on_edit();
                    return;
                }
            }
            // ── Future Job 3: Add more context actions here ────────
        }
        Action::LlmExplainFunction => editor.llm_explain_function(),
        Action::LlmReview => editor.llm_review(),
        Action::LlmAddToChat => editor.llm_add_to_chat(),
        Action::EnterReplace => {
            editor.replace_count = count;
            editor.pending_input = PendingInput::ReplaceChar;
            editor.set_status_msg("r", MessageKind::Info);
        }
        Action::ReplaceChar(ch) => {
            let replace_count = editor.replace_count.max(1);
            editor.replace_count = 0;

            if ch == '\n' {
                let (win, buf) = editor.active_window_and_buf_mut();
                let row = win.row;
                let col = win.col;
                let line_len = buf.line_char_len(row);

                if col < line_len {
                    let line_start = buf.rope.line_to_char(row); // <-- FIX: was buf.rope.line_start = ...
                    let offset = line_start + col;
                    buf.rope.remove(offset..offset + 1);
                    buf.rope.insert(offset, "\n");
                    buf.mark_modified();
                    buf.parse_syntax();
                    win.row = row + 1;
                    win.col = 0;
                    win.desired_col = 0;
                }

                // Notify LSP
                if let Some(filename) = editor.active_filename() {
                    editor.lsp_notify_change_full(std::path::PathBuf::from(filename));
                }
            } else {
                let (win, buf) = editor.active_window_and_buf_mut();
                let row = win.row;
                let col = win.col;
                let line_len = buf.line_char_len(row);

                if col < line_len {
                    let line_start = buf.rope.line_to_char(row); // <-- FIX: was buf.rope.line_start = ...
                    let offset = line_start + col;
                    let available = line_len - col;
                    let n = replace_count.min(available);

                    if n > 0 {
                        let end_offset = offset + n;
                        buf.rope.remove(offset..end_offset);
                        let replacement: String = std::iter::repeat(ch).take(n).collect();
                        buf.rope.insert(offset, &replacement);
                        buf.mark_modified();
                        buf.parse_syntax();

                        win.col = col + n - 1;
                        win.desired_col = win.col;
                    }

                    // Notify LSP
                    if let Some(filename) = editor.active_filename() {
                        editor.lsp_notify_change_full(std::path::PathBuf::from(filename));
                    }
                }
            }
            editor.comp.on_edit();
            editor.clear_status_msg();
        }

        Action::TransZhLine => {
            editor.trans_zh_line();
        }
        // ── Build actions ─────────────────────────────────────────────
        Action::BuildRun => {
            editor.run_build();
        }
        Action::BuildNextError => {
            editor.build_next_error();
        }
        Action::BuildPrevError => {
            editor.build_prev_error();
        }
        Action::BuildGotoError => {
            editor.build_goto_error();
        }
        Action::BuildClose => {
            editor.build_close();
        }
        Action::FnInfo => {
            editor.open_fn_info_popup();
        }
        Action::InputBox => {
            editor.open_input_box_with_hint(
                "info",
                "",
                "[Esc] cancel  [Ctrl+u] clear  [Ctrl+k] kill-to-end",
            );
        }

        //-- Action::ExitMode execute_action (anchor dont removed) --//
        Action::ExitMode => {
            let current_mode = editor.mode();

            // ── ④ Block-column replication on exit from insert session ─────
            // pre_captured_insert was grabbed at the very top of this function,
            // BEFORE the recording engine cleared insert_buffer.  It is safe to
            // use here regardless of what the recording engine did above.
            if let Some(state) = editor.visual_block_insert_state.take() {
                // Clear the selection anchor so that the ghost box does not persist
                // when entering visual block mode again.
                editor.active_window_mut().visual_anchor = None;

                if let Some(typed_text) = pre_captured_insert {
                    if !typed_text.is_empty() {
                        let cursor_row = editor.active_window().row;
                        let (win_row, win_col) = {
                            let win = editor.active_window();
                            (win.row, win.col)
                        };
                        let buf = editor.buf_mut();
                        for &r in &state.rows {
                            if r == cursor_row {
                                continue; // already inserted on this line normally
                            }
                            if r >= buf.len_lines() {
                                continue;
                            }
                            let line_len = buf.line_char_len(r);
                            let col = state.col;
                            if col > line_len {
                                let pad = " ".repeat(col - line_len);
                                let off = buf.rope.line_to_char(r) + line_len;
                                buf.rope.insert(off, &pad);
                            }
                            let off = buf.rope.line_to_char(r) + col;
                            buf.rope.insert(off, &typed_text);
                        }
                        buf.mark_modified();
                        buf.parse_syntax();
                    }
                }
                // insert_buffer is already None — recording engine cleared it.
            }

            // Brief mode: Esc cancels active selection first
            if current_mode == Mode::Brief && editor.active_window().visual_anchor.is_some() {
                editor.active_window_mut().visual_anchor = None;
                editor.set_status_msg("Selection cancelled", MessageKind::Info);
            } else if current_mode == Mode::Command {
                let target = editor.prev_mode;
                editor.set_mode(target);
                editor.clear_status_msg();
            } else if current_mode == Mode::Brief {
                editor.clear_completions();
                editor.clear_status_msg();
            } else if current_mode == Mode::Insert {
                let (row, col) = {
                    let win = editor.active_window();
                    (win.row, win.col)
                };
                let buf = editor.buf();
                let max_col = editing::line_display_width(buf, row).saturating_sub(1);
                let new_col = if col > 0 { col - 1 } else { 0 };
                let win = editor.active_window_mut();
                win.col = new_col.min(max_col);
                win.desired_col = win.col;
                editor.enter_normal();
                editor.clear_status_msg();
            } else if current_mode == Mode::Search {
                let target = editor.prev_mode;
                editor.set_mode(target);
                editor.clear_status_msg();
            } else if current_mode == Mode::Visual
                || current_mode == Mode::VisualLine
                || current_mode == Mode::VisualBlock
            {
                let target = if editor.prev_mode == Mode::Brief {
                    Mode::Brief
                } else {
                    Mode::Normal
                };
                editor.set_mode(target);
                editor.active_window_mut().visual_anchor = None;
                editor.clear_status_msg();
            }
            editor.active_window_mut().extra_cursors.clear();
        }

        // ---------------------------------------------------------------
        // Completion
        // ---------------------------------------------------------------
        Action::AcceptCompletion => {
            editor.accept_completion();
            editor.comp.on_edit();
        }
        Action::CycleCompletionNext => {
            editor.cycle_completion(1);
        }
        Action::CycleCompletionPrev => {
            editor.cycle_completion(-1);
        }

        // ---------------------------------------------------------------
        // Command line
        // ---------------------------------------------------------------
        Action::ExecuteCommand => {
            let cmd = editor.command().to_string();
            let prev = editor.prev_mode;
            editor.set_mode(prev);
            editor.append_and_save_history(&cmd);
            crate::repl::command::execute(editor, &cmd);
        }
        Action::CommandBackspace => {
            editor.pop_command();
            editor.reset_history_idx();
            editor.comp.on_edit();
        }
        Action::CommandChar(ch) => {
            editor.push_command(ch);
            editor.reset_history_idx();
            editor.comp.on_edit();
        }
        Action::CommandHistoryPrev => {
            editor.history_prev();
        }
        Action::CommandHistoryNext => {
            editor.history_next();
        }
        Action::CompleteCommand => {
            let current = editor.completions();
            if current.is_empty() {
                let text = editor.command().to_string();
                let cursor = editor.command_cursor;
                let before_cursor = &text[..cursor];
                let word_start = before_cursor
                    .rfind(|c: char| c.is_whitespace() || c == ':')
                    .map(|i| i + 1)
                    .unwrap_or(0);
                let current_word = &before_cursor[word_start..];

                let items = if current_word.starts_with("./") {
                    let path_items = crate::comp::path_complete::complete_path(current_word);
                    // Map "./src" to "e ./src" so set_command() gets the full line
                    path_items
                        .into_iter()
                        .map(|p| {
                            let mut full_cmd = text[..word_start].to_string();
                            full_cmd.push_str(&p);
                            full_cmd.push_str(&text[cursor..]);
                            full_cmd
                        })
                        .collect()
                } else {
                    crate::repl::command::complete_command(&text, &editor.cmd_history)
                };

                if !items.is_empty() {
                    editor.set_completions(items);
                }
            } else {
                editor.cycle_completion(1);
            }
            let idx = editor.completion_idx();
            let candidate = editor.completions().get(idx).cloned();
            if let Some(c) = candidate {
                editor.set_command(c);
            }
        }
        // ---------------------------------------------------------------
        // File / lifecycle
        // ---------------------------------------------------------------
        Action::Save => {
            match editor.save_active_buffer() {
                Ok(Some(_warning)) => {
                    // Warning already displayed by save_active_buffer()
                }
                Ok(None) => {
                    let name = editor.active_filename().unwrap_or("?").to_string();
                    editor.set_status_msg(&format!("Saved {}", name), MessageKind::Success);
                    // ← Notify ctagd daemon so it updates its DB + LSP
                    editor.notify_ctagd_saved();
                }
                Err(_e) => {
                    // Error already displayed by save_active_buffer()
                }
            }
            editor.maybe_refresh_buffer_words();
            {
                let max_row = editor.buf().len_lines().saturating_sub(1);
                let safe_row = editor.active_window().row.min(max_row);
                let max_col = editing::line_display_width(editor.buf(), safe_row);
                let win = editor.active_window_mut();
                win.row = safe_row;
                win.col = win.col.min(max_col);
                win.desired_col = win.col;
            }
        }
        Action::SaveAs => {
            editor.enter_command();
            for ch in "w ".chars() {
                editor.push_command(ch);
            }
        }
        Action::Quit => {
            editor.quit_check();
        }
        Action::ForceQuit => {
            editor.force_quit();
        }

        // ---------------------------------------------------------------
        // Clipboard
        // ---------------------------------------------------------------
        Action::YankCurrentLine => {
            let is_brief_selecting =
                editor.mode() == Mode::Brief && editor.active_window().visual_anchor.is_some();

            let yanked_text = if is_brief_selecting {
                // Yank selection
                let range = {
                    let (win, buf) = editor.active_window_and_buf_mut();
                    win.get_selection_range(buf, Mode::Visual)
                };
                if let Some((start_char, end_char)) = range {
                    editor.buf().rope.slice(start_char..end_char).to_string()
                } else {
                    String::new()
                }
            } else {
                // Yank line(s)
                let mut text = String::new();
                {
                    let win = editor.active_window();
                    let buf = editor.buf();
                    let end_row = (win.row + count).min(buf.len_lines());
                    for r in win.row..end_row {
                        text.push_str(&buf.line_text(r));
                        text.push('\n');
                    }
                }
                text
            };

            // Track as last pure yank (register 0)
            editor.yank_register_0 = Some(yanked_text.clone());
            editor.yank_to_register(yanked_text, register);

            if is_brief_selecting {
                editor.active_window_mut().visual_anchor = None;
            } else if count > 1 {
                editor.set_status_msg(&format!("Yanked {} lines", count), MessageKind::Info);
            } else {
                editor.set_status_msg("Yanked 1 line", MessageKind::Info);
            }
        }
        Action::YankCurrentWord => {
            let is_brief_selecting =
                editor.mode() == Mode::Brief && editor.active_window().visual_anchor.is_some();
            if is_brief_selecting {
                execute_action(editor, Action::YankCurrentLine);
            } else {
                if let Some(word) = editor.get_word_under_cursor() {
                    editor.yank_to_register(word.clone(), register);
                    editor.set_status_msg(&format!("Yanked word: {}", word), MessageKind::Info);
                } else {
                    editor.set_status_msg("No word under cursor", MessageKind::Error);
                }
            }
        }
        Action::Paste => {
            let text = editor.paste_from_register(register);
            if let Some(text) = text {
                if editor.clipboard_is_block && register.is_none() {
                    paste_block(editor, &text);
                    editor.comp.on_edit();

                    // Notify LSP of paste
                    if let Some(filename) = editor.active_filename() {
                        editor.lsp_notify_change_full(std::path::PathBuf::from(filename));
                    }
                } else {
                    let (win, buf) = editor.active_window_and_buf_mut();
                    if text.ends_with('\n') {
                        editing::paste_line_below(win, buf, &text);
                    } else {
                        editing::paste_text(win, buf, &text);
                    }

                    // Notify LSP of paste
                    if let Some(filename) = editor.active_filename() {
                        editor.lsp_notify_change_full(std::path::PathBuf::from(filename));
                    }

                    editor.comp.on_edit();
                }
            } else {
                editor.set_status_msg("Register is empty", MessageKind::Error);
                editor.action_failed = true;
            }
        }

        // ---------------------------------------------------------------
        // Gutter Display Toggles
        // ---------------------------------------------------------------
        Action::ToggleLineNumbers => {
            editor.config.line_numbers_enabled = !editor.config.line_numbers_enabled;
            let status = if editor.config.line_numbers_enabled {
                "on"
            } else {
                "off"
            };
            editor.set_status_msg(&format!("Line numbers: {}", status), MessageKind::Success);
            let _ = editor.config.save();
        }
        Action::ToggleRelativeLineNumbers => {
            editor.config.relative_line_numbers = !editor.config.relative_line_numbers;
            let status = if editor.config.relative_line_numbers {
                "on"
            } else {
                "off"
            };
            editor.set_status_msg(
                &format!("Relative line numbers: {}", status),
                MessageKind::Success,
            );
            let _ = editor.config.save();
        }
        Action::ToggleGitGutter => {
            editor.config.git_gutter_enabled = !editor.config.git_gutter_enabled;
            let status = if editor.config.git_gutter_enabled {
                "on"
            } else {
                "off"
            };
            editor.set_status_msg(&format!("Git gutter: {}", status), MessageKind::Success);
            let _ = editor.config.save();
        }
        Action::ToggleBookmarks => {
            editor.config.bookmarks_enabled = !editor.config.bookmarks_enabled;
            let status = if editor.config.bookmarks_enabled {
                "on"
            } else {
                "off"
            };
            editor.set_status_msg(
                &format!("Bookmarks display: {}", status),
                MessageKind::Success,
            );
            let _ = editor.config.save();
        }
        Action::ToggleBookmarkAtCursor => {
            let row = editor.active_row();
            let col = editor.active_window().col;
            let buf = editor.buf_mut();
            let existing = buf
                .named_bookmarks
                .iter()
                .find(|(_, &(r, _))| r == row)
                .map(|(&c, _)| c);
            if let Some(ch) = existing {
                buf.named_bookmarks.remove(&ch);
                buf.bookmarks.remove(&row);
                editor.set_status_msg(&format!("Mark '{}' removed", ch), MessageKind::Info);
            } else {
                let mut next_ch = None;
                for c in 'a'..='z' {
                    if !buf.named_bookmarks.contains_key(&c) {
                        next_ch = Some(c);
                        break;
                    }
                }
                if let Some(c) = next_ch {
                    buf.named_bookmarks.insert(c, (row, col));
                    buf.bookmarks.insert(row);
                    editor.set_status_msg(&format!("Mark '{}' set", c), MessageKind::Info);
                } else {
                    editor.set_status_msg("All marks a-z are already set", MessageKind::Error);
                }
            }
        }

        // ---------------------------------------------------------------
        // Config
        // ---------------------------------------------------------------
        Action::TogglePopup => {
            editor.config.popup_enabled = !editor.config.popup_enabled;
            let status = if editor.config.popup_enabled {
                "enabled"
            } else {
                "disabled"
            };
            editor.set_status_msg(&format!("Which-key popup {}", status), MessageKind::Success);
            let _ = editor.config.save();
        }

        // ---------------------------------------------------------------
        // Vim Dot Repeat
        // ---------------------------------------------------------------
        Action::RepeatLastChange => {
            editor.repeat_last_action(count);
        }
    }
}
// ---------------------------------------------------------------------------
// get_all_mode_bindings  (reference popup helper)
// ---------------------------------------------------------------------------

/// Return every known binding for `mode` as `(key, description)` pairs.
pub fn get_all_mode_bindings(mode: Mode) -> Vec<(String, String)> {
    match mode {
        Mode::Normal => {
            let mut bindings: Vec<(String, String)> = get_default_actions()
                .into_iter()
                .map(|(key, action): (&str, Action)| {
                    (key.to_string(), action_display_name(&action))
                }) // ← explicit types
                .collect();
            // Ensure zz appears with a friendly description even if the
            // generic action_display_name is terse
            bindings.push(("z z".into(), "Center cursor on screen".into()));
            bindings.push(("d a f".into(), "Delete around function".into()));
            bindings.push(("y a f".into(), "Yank around function".into()));
            bindings.push(("y i f".into(), "Yank inside function".into()));
            bindings.push(("g d".into(), "Go to definition (ctagd → ctags)".into()));
            bindings.push((
                "g t".into(),
                "Translate Chinese line → infobar + reg z".into(),
            ));
            bindings
        }

        Mode::Insert => vec![
            ("Esc".into(), "Exit to Normal".into()),
            ("Backspace".into(), "Delete backward".into()),
            ("Delete".into(), "Delete forward".into()),
            ("Enter".into(), "New line".into()),
            ("Tab".into(), "Accept completion / Insert tab".into()),
            ("→".into(), "Accept completion / Move right".into()),
            ("Ctrl+n".into(), "Cycle completion next".into()),
            ("Ctrl+p".into(), "Cycle completion prev".into()),
            ("Home".into(), "Line start".into()),
            ("End".into(), "Line end".into()),
            ("← ↑ ↓".into(), "Move cursor".into()),
            ("PageUp".into(), "Page up".into()),
            ("PageDown".into(), "Page down".into()),
            // Configurable Alt shortcuts (same as Brief)
            ("Alt+d".into(), "Delete current line".into()),
            ("Alt+u".into(), "Undo".into()),
        ],

        Mode::Brief => vec![
            ("Esc".into(), "Clear completions".into()),
            ("F9".into(), "Command mode".into()),
            ("Tab / →".into(), "Accept completion / Tab".into()),
            ("↑ / ↓".into(), "Cycle completion / Move".into()),
            // Alt
            ("Alt+/".into(), "Manual complete".into()),
            ("Alt+1-4".into(), "Switch buffer 1-4".into()),
            ("Alt+<".into(), "First line".into()),
            ("Alt+>".into(), "Last line".into()),
            ("Alt+a".into(), "Line start".into()),
            ("Alt+b".into(), "Word backward".into()),
            ("Alt+d".into(), "Delete line".into()),
            ("Alt+e".into(), "Line end".into()),
            ("Alt+f".into(), "Word forward".into()),
            ("Alt+j".into(), "Open marks popup".into()),
            ("Alt+k".into(), "Delete to EOL".into()),
            ("Alt+l".into(), "Start/Cancel Selection".into()),
            ("Alt+n".into(), "Next window".into()),
            ("Alt+o".into(), "Save as…".into()),
            ("Alt+q".into(), "Quit".into()),
            ("Alt+s".into(), "Search".into()),
            ("Alt+u".into(), "Undo".into()),
            ("Alt+w".into(), "Force quit".into()),
            ("Alt+w".into(), "Save".into()),
            ("Alt+x".into(), "Exit to Normal".into()),
            ("Alt+y".into(), "Yank line".into()),
            // Ctrl
            ("Ctrl+c".into(), "Copy".into()),
            ("Ctrl+x".into(), "Cut".into()),
            ("Ctrl+v".into(), "Paste".into()),
            ("Ctrl+n/p".into(), "Cycle completion".into()),
        ],

        Mode::Visual | Mode::VisualLine | Mode::VisualBlock => vec![
            ("Esc".into(), "Exit to Normal".into()),
            ("y".into(), "Yank selection".into()),
            ("d / x".into(), "Delete/cut selection".into()),
            ("c".into(), "Change selection".into()),
            (">".into(), "Indent selection lines".into()),
            ("<".into(), "Outdent selection lines".into()),
            ("← ↑ ↓ →".into(), "Move cursor / adjust selection".into()),
            ("+y".into(), "Yank to system clipboard".into()),
        ],

        Mode::Command => vec![
            ("Esc".into(), "Exit command mode".into()),
            ("Enter".into(), "Execute command".into()),
            ("Tab".into(), "Autocomplete".into()),
            ("Backspace".into(), "Delete backward".into()),
            ("↑ / ↓".into(), "Command history".into()),
        ],

        Mode::Search => vec![
            ("Esc".into(), "Exit search mode".into()),
            ("Enter".into(), "Execute search".into()),
            ("Backspace".into(), "Delete backward".into()),
        ],

        Mode::LlmPrompt => vec![
            ("Esc".into(), "Cancel prompt".into()),
            ("Enter".into(), "Submit query to local LLM".into()),
            ("Ctrl+r".into(), "Insert register content".into()),
        ],
    }
}

fn action_keeps_brief_selection(action: &Action) -> bool {
    if let Action::Chain(ref actions) = action {
        return actions.iter().all(|a| action_keeps_brief_selection(a));
    }
    matches!(
        action,
        Action::MoveLeft
            | Action::MoveRight
            | Action::MoveUp
            | Action::MoveDown
            | Action::MoveWordForward
            | Action::MoveWordBackward
            | Action::MoveLineStart
            | Action::MoveLineEnd
            | Action::MoveToFirstLine
            | Action::MoveToLastLine
            | Action::PageUp
            | Action::PageDown
            | Action::BriefSelectionToggle
            | Action::MatchBracket
            | Action::YankCurrentLine
            | Action::YankCurrentWord
            | Action::CycleCompletionNext
            | Action::CycleCompletionPrev
            | Action::ExtendSelectionLeft
            | Action::ExtendSelectionRight
            | Action::ExtendSelectionUp
            | Action::ExtendSelectionDown
            | Action::ExtendSelectionWordForward
            | Action::ExtendSelectionWordBackward
            | Action::ExtendSelectionLineStart
            | Action::ExtendSelectionLineEnd
            | Action::ExtendSelectionToFirstLine
            | Action::ExtendSelectionToLastLine
            | Action::ExtendSelectionPageUp
            | Action::ExtendSelectionPageDown
            | Action::BriefCopySelection
            | Action::BriefCutSelection
            | Action::DeleteCharForward
            | Action::IndentSelection
            | Action::OutdentSelection
    )
}

// ---------------------------------------------------------------------------
// Shared Custom Binding Helpers (clean, deduplicated)
// ---------------------------------------------------------------------------

/// Helper to get the active keybinding submap directly.
pub fn get_active_bindings(
    config: &Config,
    mode: Mode,
) -> &std::collections::HashMap<String, String> {
    match mode {
        Mode::Normal => &config.keybindings.normal,
        Mode::Insert => &config.keybindings.insert,
        Mode::Brief => &config.keybindings.brief,
        Mode::Command => &config.keybindings.command,
        Mode::Visual | Mode::VisualLine | Mode::VisualBlock => &config.keybindings.visual,
        Mode::Search => &config.keybindings.command,
        Mode::LlmPrompt => &config.keybindings.command,
    }
}

// ── System clipboard ────────────────────────────────────────────────────────
// All paste operations must call `clamp_cursor_after_paste` when done to
// ensure the cursor never lands past the real end of the line.  Previously
// `paste_from_system_clipboard` could leave the cursor at column 20 000+
// because the rope inserts multi-line text and the column wasn't re-clamped.
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// Internal helper – always call this after any paste that touches the cursor.
// ---------------------------------------------------------------------------
fn clamp_cursor_after_paste(editor: &mut Editor) {
    let (win, buf) = editor.active_window_and_buf_mut();
    let max_row = buf.len_lines().saturating_sub(1);
    win.row = win.row.min(max_row);
    let max_col = editing::line_display_width(buf, win.row).saturating_sub(1);
    // saturating_sub(1) would underflow on an empty line; guard for that.
    win.col = if editing::line_display_width(buf, win.row) == 0 {
        0
    } else {
        win.col.min(max_col)
    };
    win.desired_col = win.col;
}

/// Helper: check if the last status message was an error.
/// Used by `Then` chains to detect failure.
fn last_action_was_error(editor: &Editor) -> bool {
    editor.action_failed || editor.status_kind == MessageKind::Error
}
