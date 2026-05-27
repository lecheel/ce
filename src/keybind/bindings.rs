//--+ keybind/bindings.rs
//! Configurable keybindings and Actions.
//!
//! Maps physical Crossterm key events to logical Editor `Action` items,
//! supporting custom user mappings parsed from the configuration file.

use crate::config::app_config::Config;
use crate::ed::editor::Editor;
use crate::ed::editor::PendingInput;
use crate::ed::mode::{MessageKind, Mode};
use crate::ed::repeat::{DeleteDirection, RepeatExt, RepeatableAction};
use crate::ed::{editing, movement};
use crate::keybind::binding_ex::action_display_name;
use crate::keybind::binding_ex::delete_block;
use crate::keybind::binding_ex::find_custom_action;
use crate::keybind::binding_ex::paste_block;
use crate::keybind::binding_ex::yank_block;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use serde::{Deserialize, Serialize};
use std::sync::Mutex;
use std::time::{Duration, Instant};
use strum::{AsRefStr, EnumIter, EnumString};

// ========== BRIEF MODE HOME/END TRACKERS ==========
struct BriefHomeTracker {
    last_press: Option<Instant>,
    count: usize,
}

struct BriefEndTracker {
    last_press: Option<Instant>,
    count: usize,
}

static BRIEF_HOME_TRACKER: Mutex<BriefHomeTracker> = Mutex::new(BriefHomeTracker {
    last_press: None,
    count: 0,
});

static BRIEF_END_TRACKER: Mutex<BriefEndTracker> = Mutex::new(BriefEndTracker {
    last_press: None,
    count: 0,
});

/// Reset both trackers when any key other than Home/End is pressed or timeout occurs.
fn reset_brief_trackers() {
    if let Ok(mut tracker) = BRIEF_HOME_TRACKER.lock() {
        tracker.last_press = None;
        tracker.count = 0;
    }
    if let Ok(mut tracker) = BRIEF_END_TRACKER.lock() {
        tracker.last_press = None;
        tracker.count = 0;
    }
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

/// Inspects the function surrounding the cursor and returns `Ok(())` if the
/// around-function operation may proceed, or `Err(message)` when the
/// safetynet rejects it.
///
/// Two checks are performed:
///   1. **Line-count cap** – aborts if the function exceeds
///      `AROUND_FN_MAX_LINES` (500 by default).
///   2. **Nested-function guard** – aborts if the function body contains
///      one or more inner `fn` definitions (e.g. `pub fn inner() { … }`).
///      This prevents accidentally nuking an outer function that
///      encapsulates several helpers.
fn check_around_function_safetynet(editor: &Editor) -> Result<(), String> {
    match editor.function_around_span_info() {
        Some(info) => {
            if info.line_count > AROUND_FN_MAX_LINES {
                return Err(format!(
                    "Function spans {} lines (limit {}). Operation aborted for safety.",
                    info.line_count, AROUND_FN_MAX_LINES
                ));
            }
            if info.nested_fn_count > 0 {
                return Err(format!(
                    "Function contains {} nested fn definition(s). Operation aborted for safety.",
                    info.nested_fn_count
                ));
            }
            Ok(())
        }
        // Could not resolve span (e.g. no tree-sitter parse). Allow through
        // on a best-effort basis so the action still works for simple cases.
        None => Ok(()),
    }
}
// ---------------------------------------------------------------------------
// Action — representation of all editor commands
// ---------------------------------------------------------------------------
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize,  AsRefStr, EnumString, EnumIter)]
#[strum(serialize_all = "snake_case")] // MoveLeft -> "move_left" automatically
#[strum(ascii_case_insensitive)]
#[rustfmt::skip]
pub enum Action {
    // Navigation
    MoveLeft,
    MoveRight,
    MoveUp,
    MoveDown,
    MoveWordForward,
    MoveWordBackward,
    MoveLineStart,
    MoveLineEnd,
    MoveToFirstLine,
    MoveToLastLine,
    PageUp,
    PageDown,
    ScrollCenter,

    // Editing
    Backspace,
    DeleteCharForward,
    DeleteCurrentLine,
    DeleteToEndOfLine,
    InsertNewline,
    InsertNewlineBelow,
    InsertNewlineAbove,
    Undo,
    InsertTab,
    IndentLine,
    OutdentLine,

    // Modes
    EnterInsert,
    EnterAppend,
    EnterInsertLineStart,
    EnterInsertLineEnd,
    EnterCommand,
    EnterBrief,
    EnterNormal,
    ExitMode,

    // Completion
    AcceptCompletion,
    CycleCompletionNext,
    CycleCompletionPrev,

    // Command Line
    ExecuteCommand,
    CommandBackspace,
    CompleteCommand,
    CommandHistoryPrev,
    CommandHistoryNext,
    CommandLineStart,
    CommandLineEnd,
    CommandLineLeft,
    CommandLineRight,
    CommandDeleteChar,
    CommandLineKillToEnd,
    CommandClear,

    // Copy / Paste Register
    YankCurrentLine,
    YankCurrentWord,
    YankWordToSystemClipboard,    
    Paste,

    // Config Toggles
    TogglePopup,

    // Window management
    SplitHorizontal,
    SplitVertical,
    CloseWindow,
    #[strum(serialize = "only_window", serialize = "only")]
    OnlyWindow,
    FocusNextWindow,
    FocusPrevWindow,
    FocusWindowLeft,
    FocusWindowRight,
    FocusWindowUp,
    FocusWindowDown,

    // Text Objects
    DeleteInsideWord,
    ChangeInsideWord,
    DeleteInsideQuotes,
    ChangeInsideQuotes,
    DeleteInsideParens,
    ChangeInsideParens,
    DeleteInsideFunction,
    ChangeInsideFunction,
    DeleteInsideBraces,
    ChangeInsideBraces,
    DeleteInsideBrackets,
    ChangeInsideBrackets,
    DeleteAroundFunction,
    ClearSearchHighlight,

    BookmarkSet,
    BookmarkGoto,
    JumpLastPosition,

    // System clipboard
    YankToSystemClipboard,
    PasteFromSystemClipboard,
    CutToSystemClipboard,
    PutFromSystemClipboardBelow,
    ClipboardReplaceBuffer,

    // Gutter Display Toggles
    ToggleLineNumbers,
    ToggleRelativeLineNumbers,
    ToggleGitGutter,
    ToggleBookmarks,
    ToggleBookmarkAtCursor,

    // Files / Lifecycle
    BufferNext,
    BufferPrev,
    FilePicker,
    BufferList,
    BufferClose,
    Save,
    #[strum(serialize = "save_as", serialize = "write")]
    SaveAs,
    Quit,
    #[strum(serialize = "force_quit", serialize = "q!")]
    ForceQuit,
    #[strum(serialize = "quit_all", serialize = "qa")]
    QuitAll,
    #[strum(serialize = "force_quit_all", serialize = "qa!", serialize = "qall!")]
    ForceQuitAll,

    // Brief ops
    BriefSelectionToggle,

    // Extend Selection
    ExtendSelectionLeft,
    ExtendSelectionRight,
    ExtendSelectionUp,
    ExtendSelectionDown,
    ExtendSelectionWordForward,
    ExtendSelectionWordBackward,
    ExtendSelectionLineStart,
    ExtendSelectionLineEnd,
    ExtendSelectionToFirstLine,
    ExtendSelectionToLastLine,
    ExtendSelectionPageUp,
    ExtendSelectionPageDown,
    EnterLlmPrompt,
    EnterCommandPalette,
    
    FunctionList,
    Guide,
    LastRg,
    RgUnderCursor,
    BookMarks,
    Mru,
    HunkNext,
    HunkPrev,
    GitRevert,
    HunkPopup,
    GitLog,
    GitStatus,

    // Vim Search Actions
    EnterSearch,
    CancelSearch,
    ExecuteSearch,
    SearchNext,
    SearchPrev,
    SearchCurrentWord,

    // Visual Selection Modes
    EnterVisual,
    EnterVisualLine,
    EnterVisualBlock,
    VisualBlockInsert,
    VisualBlockAppend,
    YankSelection,
    DeleteSelection,
    ChangeSelection,
    IndentSelection,
    OutdentSelection,

    // Vim Dot Repeat
    RepeatLastChange,

    // ── DISABLED: Dynamic Tuple Variants ──────────────────────────
    // strum cannot generate FromStr for variants containing data.
    #[strum(disabled)]
    InsertChar(char),
    #[strum(disabled)]
    CommandChar(char),
    #[strum(disabled)]
    SwitchBuffer(usize),
}

impl Action {
    /// Parses an action string, handling strum aliases and dynamic variants.
    /// Replaces the standard `FromStr` to support custom logic.
    pub fn parse(s: &str) -> Result<Self, anyhow::Error> {
        let lower = s.to_lowercase().replace('_', "");

        // 1. Handle dynamic tuple variants manually
        if lower.starts_with("switchbuffer") || lower.starts_with("buf") {
            let idx = if lower.starts_with("buf") {
                lower
                    .trim_start_matches("buf")
                    .parse::<usize>()
                    .unwrap_or(1)
                    .saturating_sub(1)
            } else {
                lower
                    .trim_start_matches("switchbuffer")
                    .parse::<usize>()
                    .unwrap_or(1)
                    .saturating_sub(1)
            };
            return Ok(Action::SwitchBuffer(idx));
        }

        // 2. Let Strum handle the heavy lifting!
        // This automatically covers all the snake_case names, CamelCase names,
        // and all the #[strum(serialize = "...")] aliases like "dd", "yy", "daf", etc.
        if let Ok(action) = s.parse::<Self>() {
            return Ok(action);
        }

        // 3. Fallback for squished no-underscore strings that strum doesn't natively generate
        // (e.g. "deletecurrentline" instead of "delete_current_line")
        if let Ok(action) = lower.parse::<Self>() {
            return Ok(action);
        }

        anyhow::bail!("Unknown keybind action: {}", s)
    }
    /// Convert the variant name to `snake_case`.
    ///
    /// ```text
    /// MoveLeft           → "move_left"
    /// DeleteInsideWord   → "delete_inside_word"
    /// SwitchBuffer(2)    → "switch_buffer_3"   (1-indexed for display)
    /// InsertChar('x')    → "insert_char"       (payload stripped)
    /// ```
    pub fn snake_name(&self) -> String {
        match self {
            // Handle dynamic variants manually
            Action::SwitchBuffer(n) => format!("switch_buffer_{}", n + 1),
            Action::InsertChar(_) => "insert_char".to_string(),
            Action::CommandChar(_) => "command_char".to_string(),

            // Every other variant is fully automatic!
            // e.g., Action::MoveLeft.as_ref() -> "move_left"
            _ => self.as_ref().to_string(),
        }
    }

    /// Returns true if this action modifies the buffer text.
    /// Used to gate expensive operations like syntax parsing.
    pub fn modifies_buffer(&self) -> bool {
        matches!(
            self,
            Action::Backspace
                | Action::DeleteCharForward
                | Action::DeleteCurrentLine
                | Action::DeleteToEndOfLine
                | Action::InsertNewline
                | Action::InsertNewlineBelow
                | Action::InsertNewlineAbove
                | Action::InsertTab
                | Action::Undo
                | Action::IndentLine
                | Action::OutdentLine
                | Action::InsertChar(_)
                | Action::AcceptCompletion
                | Action::Paste
                | Action::DeleteInsideWord
                | Action::ChangeInsideWord
                | Action::DeleteInsideQuotes
                | Action::ChangeInsideQuotes
                | Action::DeleteInsideParens
                | Action::ChangeInsideParens
                | Action::DeleteInsideFunction
                | Action::ChangeInsideFunction
                | Action::DeleteInsideBraces
                | Action::ChangeInsideBraces
                | Action::DeleteInsideBrackets
                | Action::ChangeInsideBrackets
                | Action::DeleteAroundFunction
                | Action::ClipboardReplaceBuffer
                | Action::DeleteSelection
                | Action::IndentSelection
                | Action::OutdentSelection
                | Action::GitRevert
                | Action::PasteFromSystemClipboard
                | Action::CutToSystemClipboard
                | Action::PutFromSystemClipboardBelow
        )
    }
}

/// Calls only the strum-generated string matching, without going through
/// our custom `FromStr` (avoids infinite recursion).
fn action_from_strum(s: &str) -> Option<Action> {
    // strum's EnumString uses `std::str::FromStr` — we can't call it
    // without recursing.  Instead, enumerate the known static variants
    // that strum would handle, using `AsRefStr` in reverse via a lookup.
    //
    // The pragmatic solution: use `strum::IntoEnumIterator` if you derive
    // it, or maintain a flat match here for all non-tuple variants.
    //
    // Simplest correct approach: add `EnumString` as a *separate* derive
    // on a mirror enum, or use the approach below with strum's feature.
    use strum::IntoEnumIterator; // requires: strum_macros derive `EnumIter`
    for variant in Action::iter() {
        // Skip tuple variants (they don't impl AsRef cleanly)
        match variant {
            Action::SwitchBuffer(_) | Action::InsertChar(_) | Action::CommandChar(_) => continue,
            _ => {}
        }
        if variant.as_ref().eq_ignore_ascii_case(s) {
            return Some(variant);
        }
    }
    None
}

// ---------------------------------------------------------------------------
// Default Actions Mapping database
// ---------------------------------------------------------------------------

pub fn get_default_actions() -> Vec<(&'static str, Action)> {
    vec![
        // Basic movement
        ("h", Action::MoveLeft),
        ("left", Action::MoveLeft),
        ("j", Action::MoveDown),
        ("down", Action::MoveDown),
        ("k", Action::MoveUp),
        ("up", Action::MoveUp),
        ("l", Action::MoveRight),
        ("right", Action::MoveRight),
        // Word movement
        ("w", Action::MoveWordForward),
        ("b", Action::MoveWordBackward),
        // Line movement
        ("0", Action::MoveLineStart),
        ("home", Action::MoveLineStart),
        ("$", Action::MoveLineEnd),
        ("end", Action::MoveLineEnd),
        // File movement
        ("G", Action::MoveToLastLine),
        ("g g", Action::MoveToFirstLine),
        ("pageup", Action::PageUp),
        ("pagedown", Action::PageDown),
        ("z z", Action::ScrollCenter),
        // searching
        ("/", Action::EnterSearch),
        ("n", Action::SearchNext),
        ("N", Action::SearchPrev),
        ("*", Action::SearchCurrentWord),
        // Editing
        ("i", Action::EnterInsert),
        ("a", Action::EnterAppend),
        ("g i", Action::EnterBrief),
        ("I", Action::EnterInsertLineStart),
        ("A", Action::EnterInsertLineEnd),
        ("o", Action::InsertNewlineBelow),
        ("O", Action::InsertNewlineAbove),
        ("x", Action::DeleteCharForward),
        ("u", Action::Undo),
        ("p", Action::Paste),
        ("m", Action::BookmarkSet),
        ("`", Action::BookmarkGoto),
        // Copy
        ("+ y".into(), Action::YankToSystemClipboard), // "+y  — yank to system
        ("+ p".into(), Action::PasteFromSystemClipboard), // "+p  — paste from system
        ("+ d".into(), Action::CutToSystemClipboard),  // "+d  — cut to system
        ("+ y w".into(), Action::YankWordToSystemClipboard), // "+yw — yank word to system
        ("+ p u".into(), Action::PutFromSystemClipboardBelow), // "+pu — put below from system
        // Command mode
        (":", Action::EnterCommand),
        // Line operations
        ("d d", Action::DeleteCurrentLine),
        ("d $", Action::DeleteToEndOfLine),
        ("y y", Action::YankCurrentLine),
        (">", Action::IndentLine),
        ("<", Action::OutdentLine),
        ("space g r", Action::GitRevert),
        // Global shortcuts
        ("alt+d", Action::DeleteCurrentLine),
        ("ctrl+alt+p", Action::EnterCommandPalette),
        // Leader key sequences (Space)
        ("space Q", Action::QuitAll),
        ("space p v", Action::Paste),
        ("space t t", Action::TogglePopup),
        ("space t n", Action::ToggleLineNumbers),
        ("space t r", Action::ToggleRelativeLineNumbers),
        ("space t g", Action::ToggleGitGutter),
        ("space t b", Action::ToggleBookmarks),
        ("space m", Action::ToggleBookmarkAtCursor),
        ("space f", Action::FunctionList),
        ("space r", Action::Mru),
        ("space b", Action::BufferList),
        // Window management
        ("ctrl+w s", Action::SplitHorizontal),
        ("ctrl+w v", Action::SplitVertical),
        ("ctrl+w q", Action::CloseWindow),
        ("ctrl+w o", Action::OnlyWindow),
        ("ctrl+w w", Action::FocusNextWindow),
        ("ctrl+w h", Action::FocusWindowLeft),
        ("ctrl+w j", Action::FocusWindowDown),
        ("ctrl+w k", Action::FocusWindowUp),
        ("ctrl+w l", Action::FocusWindowRight),
        // Text Objects
        ("d i w", Action::DeleteInsideWord),
        ("c i w", Action::ChangeInsideWord),
        ("d i \"", Action::DeleteInsideQuotes),
        ("c i \"", Action::ChangeInsideQuotes),
        ("d i (", Action::DeleteInsideParens),
        ("c i (", Action::ChangeInsideParens),
        ("d i {", Action::DeleteInsideBraces),
        ("c i {", Action::ChangeInsideBraces),
        ("d i [", Action::DeleteInsideBrackets),
        ("c i [", Action::ChangeInsideBrackets),
        ("d i f", Action::DeleteInsideFunction),
        ("c i f", Action::ChangeInsideFunction),
        ("d a f", Action::DeleteAroundFunction),
        ("v", Action::EnterVisual),
        ("V", Action::EnterVisualLine),
        ("ctrl+v", Action::EnterVisualBlock),
        // Repeater Command
        (".", Action::RepeatLastChange),
    ]
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
                    KeyCode::Char('c') | KeyCode::Char('C') => Some(Action::YankCurrentLine),
                    KeyCode::Char('x') | KeyCode::Char('X') => Some(Action::DeleteCurrentLine),
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
                _ => None,
            }
        }

        Mode::LlmPrompt => None,
        Mode::Normal => None,

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

fn enters_insert_mode(action: Action) -> bool {
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

fn exits_insert_mode(action: Action) -> bool {
    matches!(action, Action::EnterNormal | Action::ExitMode)
}

// ---------------------------------------------------------------------------
// Execute Action
// ---------------------------------------------------------------------------

pub fn execute_action(editor: &mut Editor, action: Action) {
    log::debug!("execute_action: {:?}", action);

    let pre_captured_insert: Option<String> =
        if action == Action::ExitMode || action == Action::EnterNormal {
            editor.insert_buffer.clone()
        } else {
            None
        };

    if action != Action::RepeatLastChange {
        reset_brief_trackers();
    }
    if action != Action::MoveLineStart && action != Action::MoveLineEnd {
        reset_brief_trackers();
    }

    // ── MASTER UNDO GUARD ────────────────────────────────────────────
    // Determines if we should take an undo snapshot BEFORE executing the action.
    let is_typing_mode = matches!(editor.mode(), Mode::Insert | Mode::Brief);

    let is_entering_insert = matches!(
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
    );

    // ExitMode and EnterNormal finalize block inserts and must NEVER push undo,
    // as they must merge into the snapshot taken when the insert session started.
    let is_block_finalize = matches!(action, Action::ExitMode | Action::EnterNormal);
    let is_undo_or_repeat = matches!(action, Action::Undo | Action::RepeatLastChange);

    if !is_undo_or_repeat
        && !is_block_finalize
        && (is_entering_insert || (!is_typing_mode && action.modifies_buffer()))
    {
        let (win, buf) = editor.active_window_and_buf_mut();
        buf.push_undo(win.row, win.col);
    }
    // ─────────────────────────────────────────────────────────────────

    // ── Brief trackers reset ───────────────────────────────────────────────
    if action != Action::RepeatLastChange {
        reset_brief_trackers();
    }
    if action != Action::MoveLineStart && action != Action::MoveLineEnd {
        reset_brief_trackers();
    }

    // ── Brief Mode: Cancel selection on non-navigation actions ────────────
    if editor.mode() == Mode::Brief
        && editor.active_window().visual_anchor.is_some()
        && editor.visual_block_insert_state.is_none()
    {
        let keeps_selection = matches!(
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
        );
        if !keeps_selection {
            editor.active_window_mut().visual_anchor = None;
        }
    }

    // ── SAFETY: Prevent mutating actions on read-only special buffers ──────
    if action.modifies_buffer() && editor.buf().is_readonly() {
        editor.set_status_msg("Buffer is read-only", MessageKind::Error);
        return;
    }

    // Notify Git debounce engine of any buffer modification
    if action.modifies_buffer() {
        let buf_id = editor.buf().id;
        editor.git_debounce.notify_edit(buf_id);
    }

    // ── ② Recording State Engine ───────────────────────────────────────────
    if action != Action::RepeatLastChange {
        if let Some(ref mut text) = editor.insert_buffer {
            match action {
                Action::InsertChar(ch) => {
                    text.push(ch);
                }
                Action::InsertNewline => {
                    text.push('\n');
                }
                Action::InsertTab => {
                    text.push_str("    ");
                }
                Action::Backspace => {
                    text.pop();
                }
                Action::EnterNormal | Action::ExitMode => {
                    // Recording engine finalises the insert session and clears
                    // insert_buffer.  pre_captured_insert already holds a copy
                    // so block-column replication below is unaffected.
                    let final_text = text.clone();
                    editor.record_action(RepeatableAction::Insert(final_text), 1);
                    editor.insert_buffer = None;
                }
                _ => {}
            }
        } else {
            if enters_insert_mode(action) {
                editor.insert_buffer = Some(String::new());
            } else if action.modifies_buffer() {
                match action {
                    Action::Backspace => {
                        editor.record_action(
                            RepeatableAction::DeleteChars {
                                count: 1,
                                direction: DeleteDirection::Left,
                            },
                            1,
                        );
                    }
                    Action::DeleteCharForward => {
                        editor.record_action(
                            RepeatableAction::DeleteChars {
                                count: 1,
                                direction: DeleteDirection::Right,
                            },
                            1,
                        );
                    }
                    Action::DeleteCurrentLine => {
                        editor.record_action(RepeatableAction::DeleteLine, 1);
                    }
                    Action::DeleteToEndOfLine => {
                        editor.record_action(RepeatableAction::DeleteToLineEnd, 1);
                    }
                    Action::DeleteInsideWord => {
                        editor.record_action(RepeatableAction::DeleteWordForward, 1);
                    }
                    Action::IndentLine => {
                        editor.record_action(
                            RepeatableAction::Indent {
                                count: 1,
                                outdent: false,
                            },
                            1,
                        );
                    }
                    Action::OutdentLine => {
                        editor.record_action(
                            RepeatableAction::Indent {
                                count: 1,
                                outdent: true,
                            },
                            1,
                        );
                    }
                    Action::Paste => {
                        editor.record_action(
                            RepeatableAction::Paste {
                                register: '"',
                                after_cursor: true,
                            },
                            1,
                        );
                    }
                    _ => {}
                }
            }
        }
    }

    // ── ③ Action Execution ─────────────────────────────────────────────────
    // When inside a visual-block insert session, suppress per-keystroke undo
    // snapshots. A single snapshot is taken during ExitMode replication.
    let in_block_insert = editor.visual_block_insert_state.is_some();
    match action {
        Action::EnterBrief => {
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
        Action::EnterVisual => {
            if editor.mode() == Mode::Visual {
                editor.change_mode(Mode::Normal);
            } else {
                editor.change_mode(Mode::Visual);
            }
        }
        Action::EnterVisualLine => {
            if editor.mode() == Mode::VisualLine {
                editor.change_mode(Mode::Normal);
            } else {
                editor.change_mode(Mode::VisualLine);
            }
        }
        Action::EnterVisualBlock => {
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
        Action::VisualBlockInsert => {
            let anchor_opt = editor.active_window().visual_anchor;
            if let Some(anchor) = anchor_opt {
                let win_row = editor.active_window().row;
                let win_col = editor.active_window().col;

                let r1 = anchor.0.min(win_row);
                let r2 = anchor.0.max(win_row);
                let c1 = anchor.1.min(win_col);
                let rows: Vec<usize> = (r1..=r2).collect();

                editor.visual_block_insert_state =
                    Some(crate::ed::editor::VisualBlockInsertState { rows, col: c1 });

                let (win, buf) = editor.active_window_and_buf_mut();
                win.row = r1;

                // Pad active line if it is shorter than target column so alignment is correct
                let line_len = buf.line_char_len(r1);
                if c1 > line_len {
                    let pad = " ".repeat(c1 - line_len);
                    let off = buf.rope.line_to_char(r1) + line_len;
                    buf.rope.insert(off, &pad);
                    buf.modified = true;
                }
                win.col = c1;

                // Re-anchor to BOTTOM of block to keep highlights spanning correctly
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

        Action::VisualBlockAppend => {
            let anchor_opt = editor.active_window().visual_anchor;
            if let Some(anchor) = anchor_opt {
                let win_row = editor.active_window().row;
                let win_col = editor.active_window().col;

                let r1 = anchor.0.min(win_row);
                let r2 = anchor.0.max(win_row);
                let c2 = anchor.1.max(win_col);

                let rows: Vec<usize> = (r1..=r2).collect();
                let insert_col = c2 + 1;

                editor.visual_block_insert_state =
                    Some(crate::ed::editor::VisualBlockInsertState {
                        rows,
                        col: insert_col,
                    });

                let (win, buf) = editor.active_window_and_buf_mut();
                win.row = r1;

                // Pad active line if it is shorter than target column so alignment is correct
                let line_len = buf.line_char_len(r1);
                if insert_col > line_len {
                    let pad = " ".repeat(insert_col - line_len);
                    let off = buf.rope.line_to_char(r1) + line_len;
                    buf.rope.insert(off, &pad);
                    buf.modified = true;
                }
                win.col = insert_col;

                // Re-anchor to BOTTOM of block to keep highlights spanning correctly
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

        Action::YankSelection => {
            let mode = editor.mode();
            if mode == Mode::VisualBlock {
                if let Some(text) = yank_block(editor) {
                    editor.clipboard = Some(text);
                    editor.clipboard_is_block = true;
                    editor.set_status_msg("Yanked rectangular block", MessageKind::Info);
                }
            } else {
                let range = {
                    let (win, buf) = editor.active_window_and_buf_mut();
                    win.get_selection_range(buf, mode)
                };
                if let Some((start_char, end_char)) = range {
                    let text = editor.buf().rope.slice(start_char..end_char).to_string();
                    editor.clipboard = Some(text);
                    editor.clipboard_is_block = false;
                    editor.set_status_msg("Yanked selection", MessageKind::Info);
                }
            }
            let target_mode = if editor.prev_mode == Mode::Brief {
                Mode::Brief
            } else {
                Mode::Normal
            };
            editor.change_mode(target_mode); // Anchor is automatically cleared
        }
        Action::DeleteSelection => {
            let mode = editor.mode();
            if mode == Mode::VisualBlock {
                if let Some(text) = yank_block(editor) {
                    editor.clipboard = Some(text);
                    editor.clipboard_is_block = true;
                }
                delete_block(editor);
            } else {
                let range = {
                    let (win, buf) = editor.active_window_and_buf_mut();
                    win.get_selection_range(buf, mode)
                };
                if let Some((start_char, end_char)) = range {
                    let text = editor.buf().rope.slice(start_char..end_char).to_string();
                    editor.clipboard = Some(text);
                    editor.clipboard_is_block = false;
                    let (win, buf) = editor.active_window_and_buf_mut();
                    buf.rope.remove(start_char..end_char);
                    buf.modified = true;
                    let new_line = buf.rope.char_to_line(start_char);
                    win.row = new_line;
                    win.col = start_char.saturating_sub(buf.rope.line_to_char(new_line));
                    win.clamp_cursor(buf);
                    buf.parse_syntax();
                }
            }
            let target_mode = if editor.prev_mode == Mode::Brief {
                Mode::Brief
            } else {
                Mode::Normal
            };
            editor.change_mode(target_mode); // Anchor is automatically cleared
        }
        Action::ChangeSelection => {
            let mode = editor.mode();
            if mode == Mode::VisualBlock {
                if let Some(text) = yank_block(editor) {
                    editor.clipboard = Some(text);
                    editor.clipboard_is_block = true;
                }
                delete_block(editor);
            } else {
                let range = {
                    let (win, buf) = editor.active_window_and_buf_mut();
                    win.get_selection_range(buf, mode)
                };
                if let Some((start_char, end_char)) = range {
                    let (win, buf) = editor.active_window_and_buf_mut();
                    buf.rope.remove(start_char..end_char);
                    buf.modified = true;
                    let new_line = buf.rope.char_to_line(start_char);
                    win.row = new_line;
                    win.col = start_char.saturating_sub(buf.rope.line_to_char(new_line));
                    win.clamp_cursor(buf);
                    buf.parse_syntax();
                }
            }
            if editor.prev_mode == Mode::Brief {
                editor.change_mode(Mode::Brief);
            } else {
                editor.change_mode(Mode::Insert);
            }
        }

        Action::IndentSelection => {
            let (win, buf) = editor.active_window_and_buf_mut();
            if let Some(anchor) = win.visual_anchor {
                let r1 = anchor.0.min(win.row);
                let r2 = anchor.0.max(win.row);
                for r in r1..=r2 {
                    let mut temp_win = win.clone();
                    temp_win.row = r;
                    editing::indent_line(&mut temp_win, buf);
                }
                buf.parse_syntax();
            }
            editor.enter_normal();
        }
        Action::OutdentSelection => {
            let (win, buf) = editor.active_window_and_buf_mut();
            if let Some(anchor) = win.visual_anchor {
                let r1 = anchor.0.min(win.row);
                let r2 = anchor.0.max(win.row);
                for r in r1..=r2 {
                    let mut temp_win = win.clone();
                    temp_win.row = r;
                    editing::outdent_line(&mut temp_win, buf);
                }
                buf.parse_syntax();
            }
            editor.enter_normal();
        }

        // ---------------------------------------------------------------
        // Movement
        // ---------------------------------------------------------------
        Action::MoveLeft => {
            {
                let (win, buf) = editor.active_window_and_buf_mut();
                movement::move_left(win, buf);
            }
            editor.snap_cursor_to_viewport();
            editor.comp.on_leave_insert();
        }
        Action::MoveRight => {
            {
                let (win, buf) = editor.active_window_and_buf_mut();
                movement::move_right(win, buf);
            }
            editor.snap_cursor_to_viewport();
            editor.comp.on_leave_insert();
        }
        Action::MoveUp => {
            {
                let (win, buf) = editor.active_window_and_buf_mut();
                movement::move_up(win, buf);
            }
            editor.snap_cursor_to_viewport();
            editor.comp.on_leave_insert();
        }
        Action::MoveDown => {
            {
                let (win, buf) = editor.active_window_and_buf_mut();
                movement::move_down(win, buf);
            }
            editor.snap_cursor_to_viewport();
            editor.comp.on_leave_insert();
        }
        Action::MoveWordForward => {
            {
                let (win, buf) = editor.active_window_and_buf_mut();
                movement::move_word_forward(win, buf);
            }
            editor.snap_cursor_to_viewport();
            editor.comp.on_leave_insert();
        }
        Action::MoveWordBackward => {
            {
                let (win, buf) = editor.active_window_and_buf_mut();
                movement::move_word_backward(win, buf);
            }
            editor.snap_cursor_to_viewport();
            editor.comp.on_leave_insert();
        }
        Action::MoveLineStart => {
            let is_brief = editor.mode() == Mode::Brief;
            {
                let (win, buf) = editor.active_window_and_buf_mut();
                if is_brief {
                    let now = Instant::now();
                    let mut tracker = BRIEF_HOME_TRACKER.lock().unwrap();
                    let is_consecutive = if let Some(last) = tracker.last_press {
                        now.duration_since(last) < Duration::from_millis(500)
                    } else {
                        false
                    };
                    if is_consecutive {
                        tracker.count = (tracker.count + 1) % 3;
                    } else {
                        tracker.count = 0;
                    }
                    tracker.last_press = Some(now);
                    match tracker.count {
                        0 => movement::move_line_start(win, buf),
                        1 => {
                            win.row = win.scroll_line;
                            win.col = 0;
                            win.clamp_cursor(buf);
                        }
                        _ => movement::move_to_first_line(win, buf),
                    }
                } else {
                    movement::move_line_start(win, buf);
                }
            }
            editor.snap_cursor_to_viewport();
            editor.comp.on_leave_insert();
        }
        Action::MoveLineEnd => {
            let current_mode = editor.mode();
            let is_brief = current_mode == Mode::Brief;
            {
                let (win, buf) = editor.active_window_and_buf_mut();
                if is_brief {
                    let now = Instant::now();
                    let mut tracker = BRIEF_END_TRACKER.lock().unwrap();
                    let is_consecutive = if let Some(last) = tracker.last_press {
                        now.duration_since(last) < Duration::from_millis(500)
                    } else {
                        false
                    };
                    if is_consecutive {
                        tracker.count = (tracker.count + 1) % 3;
                    } else {
                        tracker.count = 0;
                    }
                    tracker.last_press = Some(now);
                    match tracker.count {
                        0 => movement::move_line_end(win, buf, current_mode),
                        1 => {
                            let last_visible = (win.scroll_line
                                + win.position.height.saturating_sub(1))
                            .min(buf.len_lines().saturating_sub(1));
                            win.row = last_visible;
                            win.col = buf.line_char_len(win.row).saturating_sub(1);
                            win.desired_col = win.col;
                            win.clamp_cursor(buf);
                        }
                        _ => movement::move_to_last_line(win, buf),
                    }
                } else {
                    movement::move_line_end(win, buf, current_mode);
                }
            }
            editor.snap_cursor_to_viewport();
            editor.comp.on_leave_insert();
        }
        Action::MoveToFirstLine => {
            {
                let (win, buf) = editor.active_window_and_buf_mut();
                movement::move_to_first_line(win, buf);
            }
            editor.snap_cursor_to_viewport();
            editor.comp.on_leave_insert();
        }
        Action::MoveToLastLine => {
            {
                let (win, buf) = editor.active_window_and_buf_mut();
                movement::move_to_last_line(win, buf);
            }
            editor.snap_cursor_to_viewport();
            editor.comp.on_leave_insert();
        }
        Action::PageUp => {
            {
                let (win, buf) = editor.active_window_and_buf_mut();
                let jump = win.position.height.saturating_sub(2).max(1);
                movement::page_up(win, buf, jump);
            }
            editor.snap_cursor_to_viewport();
            editor.comp.on_leave_insert();
        }
        Action::PageDown => {
            {
                let (win, buf) = editor.active_window_and_buf_mut();
                let jump = win.position.height.saturating_sub(2).max(1);
                movement::page_down(win, buf, jump);
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
                editor.command_cursor -= 1;
            }
        }
        Action::CommandLineRight => {
            if editor.command_cursor < editor.command().len() {
                editor.command_cursor += 1;
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
        // Text Objects
        // ---------------------------------------------------------------
        Action::DeleteInsideWord => {
            editor.edit_text_object(crate::ed::syntax::TextObject::Word, true, false);
        }
        Action::ChangeInsideWord => {
            editor.edit_text_object(crate::ed::syntax::TextObject::Word, true, true);
        }
        Action::DeleteInsideQuotes => {
            editor.edit_text_object(crate::ed::syntax::TextObject::Quotes, true, false);
        }
        Action::ChangeInsideQuotes => {
            editor.edit_text_object(crate::ed::syntax::TextObject::Quotes, true, true);
        }
        Action::DeleteInsideParens => {
            editor.edit_text_object(crate::ed::syntax::TextObject::Parens, true, false);
        }
        Action::ChangeInsideParens => {
            editor.edit_text_object(crate::ed::syntax::TextObject::Parens, true, true);
        }
        Action::DeleteInsideFunction => {
            editor.edit_text_object(crate::ed::syntax::TextObject::Function, true, false);
        }
        Action::ChangeInsideFunction => {
            editor.edit_text_object(crate::ed::syntax::TextObject::Function, true, true);
        }
        Action::DeleteInsideBraces => {
            editor.edit_text_object(crate::ed::syntax::TextObject::Braces, true, false);
        }
        Action::ChangeInsideBraces => {
            editor.edit_text_object(crate::ed::syntax::TextObject::Braces, true, true);
        }
        Action::DeleteInsideBrackets => {
            editor.edit_text_object(crate::ed::syntax::TextObject::Brackets, true, false);
        }
        Action::ChangeInsideBrackets => {
            editor.edit_text_object(crate::ed::syntax::TextObject::Brackets, true, true);
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
                    buf.modified = true;
                    buf.parse_syntax();
                    text
                }; // win and buf dropped here

                // Yank the deleted text
                editor.clipboard = Some(deleted);

                // Reposition cursor
                {
                    let (win, buf) = editor.active_window_and_buf_mut();
                    let max_row = buf.len_lines().saturating_sub(1);
                    win.row = info.start_row.min(max_row);
                    win.col = 0;
                    win.clamp_cursor(buf);
                    win.desired_col = win.col;
                }
            } else {
                editor.set_status_msg("No function found around cursor", MessageKind::Error);
            }
        }
        // ---------------------------------------------------------------
        // Window management
        // ---------------------------------------------------------------
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
            let (win, buf) = editor.active_window_and_buf_mut();
            let (row, col) = (win.row, win.col);
            editing::backspace(win, buf);
            editor.comp.on_edit();
        }
        Action::DeleteCharForward => {
            let (win, buf) = editor.active_window_and_buf_mut();
            let (row, col) = (win.row, win.col);
            editing::delete_char_forward(win, buf);
            editor.comp.on_edit();
        }
        Action::DeleteCurrentLine => {
            let line_text = editor.line_text(editor.active_row());
            editor.clipboard = Some(format!("{}\n", line_text));
            let (win, buf) = editor.active_window_and_buf_mut();
            let (row, col) = (win.row, win.col);
            editing::delete_current_line(win, buf);
            editor.comp.on_edit();
        }
        Action::DeleteToEndOfLine => {
            let (win, buf) = editor.active_window_and_buf_mut();
            let (row, col) = (win.row, win.col);
            editing::delete_to_end_of_line(win, buf);
            editor.comp.on_edit();
        }
        Action::InsertNewline => {
            let (win, buf) = editor.active_window_and_buf_mut();
            let (row, col) = (win.row, win.col);
            editing::insert_newline(win, buf);
            buf.parse_syntax();
            editor.comp.on_edit();
        }
        Action::InsertTab => {
            let (win, buf) = editor.active_window_and_buf_mut();
            let (row, col) = (win.row, win.col);
            editing::insert_tab(win, buf);
            buf.parse_syntax();
            editor.comp.on_edit();
        }
        Action::Undo => {
            let (win, buf) = editor.active_window_and_buf_mut();
            if let Some(snap) = buf.pop_undo() {
                buf.rope = snap.rope;
                buf.modified = snap.modified;
                buf.parse_syntax();
                win.row = snap.cursor_row;
                win.col = snap.cursor_col;
            }
            editor.comp.on_leave_insert();
        }
        Action::IndentLine => {
            let (win, buf) = editor.active_window_and_buf_mut();
            let (row, col) = (win.row, win.col);
            editing::indent_line(win, buf);
            editor.comp.on_edit();
        }
        Action::OutdentLine => {
            let (win, buf) = editor.active_window_and_buf_mut();
            let (row, col) = (win.row, win.col);
            editing::outdent_line(win, buf);
            editor.comp.on_edit();
        }
        Action::InsertChar(ch) => {
            let (win, buf) = editor.active_window_and_buf_mut();
            let (row, col) = (win.row, win.col);
            editing::insert_char(win, buf, ch);
            editor.comp.on_edit();
        }

        // ---------------------------------------------------------------
        // Mode transitions
        // ---------------------------------------------------------------
        Action::EnterInsert => {
            editor.enter_insert();
        }
        Action::EnterAppend => {
            let (win, buf) = editor.active_window_and_buf_mut();
            win.col = (win.col + 1).min(buf.line_char_len(win.row));
            let _ = buf;
            editor.enter_insert();
        }
        Action::EnterInsertLineStart => {
            let (win, buf) = editor.active_window_and_buf_mut();
            movement::move_line_start(win, buf);
            let _ = buf;
            editor.enter_insert();
        }
        Action::EnterInsertLineEnd => {
            let (win, buf) = editor.active_window_and_buf_mut();
            movement::move_line_end(win, buf, Mode::Insert);
            let _ = buf;
            editor.enter_insert();
        }
        Action::InsertNewlineBelow => {
            let (win, buf) = editor.active_window_and_buf_mut();
            let (row, col) = (win.row, win.col);
            editing::insert_newline_below(win, buf);
            buf.parse_syntax();
            let _ = buf;
            editor.enter_insert();
        }
        Action::InsertNewlineAbove => {
            let (win, buf) = editor.active_window_and_buf_mut();
            let (row, col) = (win.row, win.col);
            editing::insert_newline_above(win, buf);
            buf.parse_syntax();
            let _ = buf;
            editor.enter_insert();
        }
        Action::EnterCommand => {
            editor.enter_command();
        }
        Action::EnterNormal => {
            editor.finalize_visual_block_insert(pre_captured_insert.clone());
            if editor.mode() == Mode::Insert || editor.mode() == Mode::Brief {
                let (row, col) = {
                    let win = editor.active_window();
                    (win.row, win.col)
                };
                let max_col = editor.buf().line_char_len(row).saturating_sub(1);
                let new_col = if col > 0 { col - 1 } else { 0 };
                let win = editor.active_window_mut();
                win.col = new_col.min(max_col);
                win.desired_col = win.col;
            }
            editor.change_mode(Mode::Normal);
            editor.clear_status_msg();
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
                    win.col = pos - buf.rope.line_to_char(row);
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
                    win.col = pos - buf.rope.line_to_char(row);
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
                    win.col = pos - buf.rope.line_to_char(row);
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
            editor.open_marks_popup();
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
                buf.modified = true;
            }

            editor.paste_from_system_clipboard();

            // Reset to top, clamp, parse once.
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
                        buf.modified = true;
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
                let max_col = editor.buf().line_char_len(row).saturating_sub(1);
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
            editor.cmd_history_idx = None;
            editor.history_search_prefix = None;
            editor.comp.on_edit();
        }
        Action::CommandChar(ch) => {
            editor.push_command(ch);
            editor.cmd_history_idx = None;
            editor.history_search_prefix = None;
            editor.comp.on_edit();
        }
        Action::CommandHistoryPrev => {
            editor.history_prev();
        }
        Action::CommandHistoryNext => {
            editor.history_next();
        }
        Action::CompleteCommand => {
            let current = editor.completions().to_vec();
            if current.is_empty() {
                let items =
                    crate::repl::command::complete_command(editor.command(), &editor.cmd_history);
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
            if let Err(e) = editor.save_active_buffer() {
                editor.set_status_msg(&format!("Save failed: {}", e), MessageKind::Error);
            } else {
                let name = editor.active_filename().unwrap_or("?").to_string();
                editor.set_status_msg(&format!("Saved {}", name), MessageKind::Success);
                editor.refresh_buffer_words();
                {
                    let max_row = editor.buf().len_lines().saturating_sub(1);
                    let safe_row = editor.active_window().row.min(max_row);
                    let max_col = editor.buf().line_char_len(safe_row);
                    let win = editor.active_window_mut();
                    win.row = safe_row;
                    win.col = win.col.min(max_col);
                    win.desired_col = win.col;
                }
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
            if is_brief_selecting {
                let range = {
                    let (win, buf) = editor.active_window_and_buf_mut();
                    win.get_selection_range(buf, Mode::Visual)
                };
                if let Some((start_char, end_char)) = range {
                    let text = editor.buf().rope.slice(start_char..end_char).to_string();
                    editor.clipboard = Some(text);
                    editor.set_status_msg("Yanked selection", MessageKind::Info);
                }
                editor.active_window_mut().visual_anchor = None;
            } else {
                let line_text = editor.line_text(editor.active_row());
                editor.clipboard = Some(format!("{}\n", line_text));
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
                    editor.clipboard = Some(word.clone());
                    editor.set_status_msg(&format!("Yanked word: {}", word), MessageKind::Info);
                } else {
                    editor.set_status_msg("No word under cursor", MessageKind::Error);
                }
            }
        }
        Action::Paste => {
            if let Some(text) = editor.clipboard.clone() {
                if editor.clipboard_is_block {
                    paste_block(editor, &text);
                    editor.comp.on_edit();
                } else {
                    let (win, buf) = editor.active_window_and_buf_mut();
                    let (row, col) = (win.row, win.col);
                    if text.ends_with('\n') {
                        editing::paste_line_below(win, buf, &text);
                    } else {
                        editing::paste_text(win, buf, &text);
                    }
                    editor.comp.on_edit();
                }
            } else {
                editor.set_status_msg("Yank register is empty", MessageKind::Error);
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
            editor.repeat_last_action();
        }
    }
}
// ---------------------------------------------------------------------------
// get_all_mode_bindings  (reference popup helper)
// ---------------------------------------------------------------------------

//gg/ Return every known binding for `mode` as `(key, description)` pairs.
pub fn get_all_mode_bindings(mode: Mode) -> Vec<(String, String)> {
    match mode {
        Mode::Normal => {
            let mut bindings: Vec<(String, String)> = get_default_actions()
                .into_iter()
                .map(|(key, action)| (key.to_string(), action_display_name(&action)))
                .collect();
            // Ensure zz appears with a friendly description even if the
            // generic action_display_name is terse
            bindings.push(("z z".into(), "Center cursor on screen".into()));
            bindings.push(("d a f".into(), "Delete around function".into()));
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
            ("Alt+s".into(), "Search".into()),
            ("Alt+w".into(), "Save".into()),
            ("Alt+o".into(), "Save as…".into()),
            ("Alt+q".into(), "Quit".into()),
            ("Alt+w".into(), "Force quit".into()),
            ("Alt+d".into(), "Delete line".into()),
            ("Alt+j".into(), "Open marks popup".into()), // Update description
            ("Alt+k".into(), "Delete to EOL".into()),
            ("Alt+u".into(), "Undo".into()),
            ("Alt+y".into(), "Yank line".into()),
            ("Alt+l".into(), "Start/Cancel Selection".into()),
            ("Alt+b".into(), "Word backward".into()),
            ("Alt+f".into(), "Word forward".into()),
            ("Alt+a".into(), "Line start".into()),
            ("Alt+e".into(), "Line end".into()),
            ("Alt+<".into(), "First line".into()),
            ("Alt+>".into(), "Last line".into()),
            ("Alt+n".into(), "Next window".into()),
            ("Alt+1-4".into(), "Switch buffer 1-4".into()),
            ("Alt+x".into(), "Exit to Normal".into()),
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
    let max_col = buf.line_char_len(win.row).saturating_sub(1);
    // saturating_sub(1) would underflow on an empty line; guard for that.
    win.col = if buf.line_char_len(win.row) == 0 {
        0
    } else {
        win.col.min(max_col)
    };
    win.desired_col = win.col;
}
