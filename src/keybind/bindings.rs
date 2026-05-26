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

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::str::FromStr;

// ---------------------------------------------------------------------------
// Action — representation of all editor commands
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
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

    // Editing
    Backspace,
    DeleteCharForward,
    DeleteCurrentLine,
    DeleteToEndOfLine,
    InsertNewline,
    InsertNewlineBelow,
    InsertNewlineAbove,
    Undo,
    InsertChar(char),
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
    CommandChar(char),
    CompleteCommand,
    CommandHistoryPrev,
    CommandHistoryNext,

    // Copy / Paste Register
    YankCurrentLine,
    Paste,

    // Config Toggles
    TogglePopup,

    // Window management
    SplitHorizontal,
    SplitVertical,
    CloseWindow,
    OnlyWindow,
    FocusNextWindow,
    FocusPrevWindow,
    FocusWindowLeft,
    FocusWindowRight,
    FocusWindowUp,
    FocusWindowDown,
    SwitchBuffer(usize),

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

    BookmarkSet,  // 'm' prefix — awaits next char
    BookmarkGoto, // '`' prefix — awaits next char or '`' for ping-pong
    JumpLastPosition,

    // ── System clipboard
    YankToSystemClipboard,
    PasteFromSystemClipboard,
    CutToSystemClipboard,
    YankWordToSystemClipboard,
    PutFromSystemClipboardBelow,

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
    Quit,
    ForceQuit,
    QuitAll,
    ForceQuitAll,

    //-- enum Actions (anchor dont removed) --//
    EnterLlmPrompt,
    FunctionList,
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
    ExecuteSearch,
    SearchNext,
    SearchPrev,
    SearchCurrentWord,

    // Visual Selection Modes
    EnterVisual,
    EnterVisualLine,
    YankSelection,
    DeleteSelection,
    ChangeSelection,
    IndentSelection,
    OutdentSelection,

    // Vim Dot Repeat
    RepeatLastChange,
}

impl Action {
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
        )
    }
}

impl FromStr for Action {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().replace('_', "").as_str() {
            "moveleft" | "left" => Ok(Action::MoveLeft),
            "moveright" | "right" => Ok(Action::MoveRight),
            "moveup" | "up" => Ok(Action::MoveUp),
            "movedown" | "down" => Ok(Action::MoveDown),
            "movewordforward" | "wordforward" => Ok(Action::MoveWordForward),
            "movewordbackward" | "wordbackward" => Ok(Action::MoveWordBackward),
            "movelinestart" | "linestart" => Ok(Action::MoveLineStart),
            "movelineend" | "lineend" => Ok(Action::MoveLineEnd),
            "movetofirstline" | "firstline" => Ok(Action::MoveToFirstLine),
            "movetolastline" | "lastline" => Ok(Action::MoveToLastLine),
            "pageup" => Ok(Action::PageUp),
            "pagedown" => Ok(Action::PageDown),

            "backspace" => Ok(Action::Backspace),
            "delete" | "deletecharforward" => Ok(Action::DeleteCharForward),
            "deletecurrentline" | "deleteline" => Ok(Action::DeleteCurrentLine),
            "deletetoendofline" | "deleteendline" | "d$" => Ok(Action::DeleteToEndOfLine),
            "insertnewline" | "newline" => Ok(Action::InsertNewline),
            "insertnewlinebelow" | "newlinebelow" => Ok(Action::InsertNewlineBelow),
            "insertnewlineabove" | "newlineabove" => Ok(Action::InsertNewlineAbove),
            "undo" => Ok(Action::Undo),
            "inserttab" | "tab" => Ok(Action::InsertTab),
            "indentline" | "indent" | ">" => Ok(Action::IndentLine),
            "outdentline" | "outdent" | "<" => Ok(Action::OutdentLine),

            "enterinsert" | "insert" => Ok(Action::EnterInsert),
            "enterappend" | "append" => Ok(Action::EnterAppend),
            "enterinsertlinestart" | "insertlinestart" => Ok(Action::EnterInsertLineStart),
            "enterinsertlineend" | "insertlineend" => Ok(Action::EnterInsertLineEnd),
            "entercommand" | "command" => Ok(Action::EnterCommand),
            "enternormal" | "normal" => Ok(Action::EnterNormal),
            "exitmode" | "esc" | "normalmode" => Ok(Action::ExitMode),

            "acceptcompletion" | "accept" => Ok(Action::AcceptCompletion),
            "cyclecompletionnext" | "completionnext" => Ok(Action::CycleCompletionNext),
            "cyclecompletionprev" | "completionprev" => Ok(Action::CycleCompletionPrev),
            "completecommand" | "complete" | "tabcomplete" => Ok(Action::CompleteCommand),
            "commandhistoryprev" | "cmdprev" | "historyprev" => Ok(Action::CommandHistoryPrev),
            "commandhistorynext" | "cmdnext" | "historynext" => Ok(Action::CommandHistoryNext),

            "yankcurrentline" | "yankline" | "yy" => Ok(Action::YankCurrentLine),
            "paste" | "put" | "p" => Ok(Action::Paste),

            "togglepopup" | "togglewhichkey" => Ok(Action::TogglePopup),

            // Window management
            "splithorizontal" | "split" | "sp" => Ok(Action::SplitHorizontal),
            "splitvertical" | "vsplit" | "vs" => Ok(Action::SplitVertical),
            "closewindow" | "winclose" | "wq" => Ok(Action::CloseWindow),
            "onlywindow" | "winonly" | "on" => Ok(Action::OnlyWindow),
            "focusnextwindow" | "focusnext" => Ok(Action::FocusNextWindow),
            "focusprevwindow" | "focusprev" => Ok(Action::FocusPrevWindow),
            "focuswindowleft" | "windowleft" | "wholeft" => Ok(Action::FocusWindowLeft),
            "focuswindowright" | "windowright" | "whoright" => Ok(Action::FocusWindowRight),
            "focuswindowup" | "windowup" | "whoup" => Ok(Action::FocusWindowUp),
            "focuswindowdown" | "windowdown" | "whodown" => Ok(Action::FocusWindowDown),
            "switchbuffer1" | "buf1" => Ok(Action::SwitchBuffer(0)),
            "switchbuffer2" | "buf2" => Ok(Action::SwitchBuffer(1)),
            "switchbuffer3" | "buf3" => Ok(Action::SwitchBuffer(2)),
            "switchbuffer4" | "buf4" => Ok(Action::SwitchBuffer(3)),

            "deleteinsideword" | "diw" => Ok(Action::DeleteInsideWord),
            "changeinsideword" | "ciw" => Ok(Action::ChangeInsideWord),
            "deleteinsidequotes" | "di\"" => Ok(Action::DeleteInsideQuotes),
            "changeinsidequotes" | "ci\"" => Ok(Action::ChangeInsideQuotes),
            "deleteinsideparens" | "di(" | "di)" => Ok(Action::DeleteInsideParens),
            "changeinsideparens" | "ci(" | "ci)" => Ok(Action::ChangeInsideParens),
            "deleteinsidebraces" | "di{" | "di}" => Ok(Action::DeleteInsideBraces),
            "changeinsidebraces" | "ci{" | "ci}" => Ok(Action::ChangeInsideBraces),
            "deleteinsidebrackets" | "di[" | "di]" => Ok(Action::DeleteInsideBrackets),
            "changeinsidebrackets" | "ci[" | "ci]" => Ok(Action::ChangeInsideBrackets),
            "deleteinsidefunction" | "dif" => Ok(Action::DeleteInsideFunction),
            "changeinsidefunction" | "cif" => Ok(Action::ChangeInsideFunction),
            "togglelinenumbers" | "togglelines" | "linenumbers" => Ok(Action::ToggleLineNumbers),
            "togglerelativelinenumbers" | "togglerelative" | "relativenumber" => {
                Ok(Action::ToggleRelativeLineNumbers)
            }
            "togglegitgutter" | "gitgutter" => Ok(Action::ToggleGitGutter),
            "togglebookmarks" | "bookmarks" => Ok(Action::ToggleBookmarks),
            "togglebookmarkatcursor" | "togglebookmark" | "bookmark" => {
                Ok(Action::ToggleBookmarkAtCursor)
            }
            "entervisual" | "visual" => Ok(Action::EnterVisual),
            "entervisualline" | "visualline" => Ok(Action::EnterVisualLine),
            "yankselection" => Ok(Action::YankSelection),
            "deleteselection" => Ok(Action::DeleteSelection),
            "changeselection" => Ok(Action::ChangeSelection),
            "indentselection" => Ok(Action::IndentSelection),
            "outdentselection" => Ok(Action::OutdentSelection),
            "bn" | "bnext" | "buffernext" => Ok(Action::BufferNext),
            "bp" | "bprev" | "bufferprev" => Ok(Action::BufferPrev),
            "mru" => Ok(Action::Mru),
            "entersearch" | "search" | "/" => Ok(Action::EnterSearch),
            "executesearch" => Ok(Action::ExecuteSearch),
            "searchnext" | "next" | "n" => Ok(Action::SearchNext),
            "searchprev" | "prev" | "N" => Ok(Action::SearchPrev),
            "searchcurrentword" | "curword" | "*" => Ok(Action::SearchCurrentWord),
            "functions" | "funlist" | "fns" | "fn" => Ok(Action::FunctionList),
            "hunknext" | "nexthunk" | "hunkn" => Ok(Action::HunkNext),
            "hunkprev" | "prevhunk" | "hunkp" => Ok(Action::HunkPrev),
            "gitrevert" | "reverthunk" | "grevert" => Ok(Action::GitRevert),
            "hunkpopup" | "hunkdiff" => Ok(Action::HunkPopup),
            "gitlog" | "glog" | "tig" => Ok(Action::GitLog),
            "gitstatus" | "gs" => Ok(Action::GitStatus),

            "bufferlist" | "ls" | "buffers" => Ok(Action::BufferList),
            "bufferclose" => Ok(Action::BufferClose),
            "quitall" | "qa" => Ok(Action::QuitAll),
            "forcequitall" | "qa!" | "qall!" => Ok(Action::ForceQuitAll),
            "lastrg" => Ok(Action::LastRg),
            "rgundercursor" => Ok(Action::RgUnderCursor),
            "marks" => Ok(Action::BookMarks),
            "llmprompt" => Ok(Action::EnterLlmPrompt),

            //-- FromStr commands action enterbrief (anchor dont removed) --//
            "enterbrief" | "brief" => Ok(Action::EnterBrief),
            "filepicker" => Ok(Action::FilePicker),
            "save" => Ok(Action::Save),
            "quit" => Ok(Action::Quit),
            "forcequit" => Ok(Action::ForceQuit),

            _ => anyhow::bail!("Unknown keybind action: {}", s),
        }
    }
}

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
        ("v", Action::EnterVisual),
        ("V", Action::EnterVisualLine),
        // Repeater Command
        (".", Action::RepeatLastChange),
    ]
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
                    KeyCode::Char('c') => Some(Action::ExitMode),
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

        Mode::Command => match raw_key.code {
            KeyCode::Esc => Some(Action::ExitMode),
            KeyCode::Enter => Some(Action::ExecuteCommand),
            KeyCode::Tab => Some(Action::CompleteCommand),
            KeyCode::Backspace => Some(Action::CommandBackspace),
            KeyCode::Char(ch) => Some(Action::CommandChar(ch)),
            KeyCode::Up => Some(Action::CommandHistoryPrev),
            KeyCode::Down => Some(Action::CommandHistoryNext),
            _ => None,
        },

        Mode::Search => match raw_key.code {
            KeyCode::Esc => Some(Action::ExitMode),
            KeyCode::Enter => Some(Action::ExecuteSearch),
            KeyCode::Backspace => Some(Action::CommandBackspace),
            KeyCode::Char(ch) => Some(Action::CommandChar(ch)),
            _ => None,
        },
        Mode::LlmPrompt => None,
        Mode::Normal => None,

        Mode::Visual | Mode::VisualLine => match key_str {
            "y" => Some(Action::YankSelection),
            // "+" => Some(Action::YankToSystemClipboard),
            "d" | "x" => Some(Action::DeleteSelection),
            "c" => Some(Action::ChangeSelection),
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
    // Only process Alt (no Alt+Ctrl, no Alt+Shift for letters)
    if key.modifiers.contains(KeyModifiers::CONTROL) {
        return None;
    }

    match key.code {
        // ── File ────────────────────────────────
        KeyCode::Char('w') | KeyCode::Char('W') => Some(Action::Save),
        KeyCode::Char('q') | KeyCode::Char('Q') => Some(Action::Quit),
        KeyCode::Char('e') | KeyCode::Char('E') => Some(Action::FilePicker),

        // ── Movement ────────────────────────────
        KeyCode::Char('f') | KeyCode::Char('F') => Some(Action::MoveWordForward),
        KeyCode::Char('<') => Some(Action::MoveToFirstLine),
        KeyCode::Char('>') => Some(Action::MoveToLastLine),

        // ── Editing ─────────────────────────────
        KeyCode::Char('d') | KeyCode::Char('D') => Some(Action::DeleteCurrentLine),
        KeyCode::Char('k') | KeyCode::Char('K') => Some(Action::DeleteToEndOfLine),
        KeyCode::Char('u') | KeyCode::Char('U') => Some(Action::Undo),
        KeyCode::Char('y') | KeyCode::Char('Y') => Some(Action::YankCurrentLine),
        KeyCode::Char('p') | KeyCode::Char('P') => Some(Action::Paste),

        // ── Window / Buffer ─────────────────────
        KeyCode::Char('b') | KeyCode::Char('B') => Some(Action::BufferList),
        KeyCode::Char('n') | KeyCode::Char('N') => Some(Action::FocusNextWindow),
        KeyCode::Char('1') => Some(Action::SwitchBuffer(0)),
        KeyCode::Char('2') => Some(Action::SwitchBuffer(1)),
        KeyCode::Char('3') => Some(Action::SwitchBuffer(2)),
        KeyCode::Char('4') => Some(Action::SwitchBuffer(3)),

        // ── Mode toggle ─────────────────────────
        KeyCode::Char('x') | KeyCode::Char('X') => Some(Action::ExitMode),

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

    // ── SAFETY: Prevent mutating actions on read-only special buffers ──
    if action.modifies_buffer() && editor.buf().is_readonly() {
        editor.set_status_msg("Buffer is read-only", MessageKind::Error);
        return;
    }

    // Notify Git debounce engine of any buffer modification
    if action.modifies_buffer() {
        let buf_id = editor.buf().id;
        editor.git_debounce.notify_edit(buf_id);
    }

    // ── 1. Recording State Engine ──────────────────────────────────────────
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
                    let final_text = text.clone();
                    editor.record_action(RepeatableAction::Insert(final_text), 1);
                    editor.insert_buffer = None;
                }
                _ => {} // Ignore navigation keys during insertion
            }
        } else {
            if enters_insert_mode(action) {
                editor.insert_buffer = Some(String::new());
            } else if action.modifies_buffer() {
                // Record standalone normal mode buffer edits
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

    // ── 2. Action Execution ──────────────────────────────────────────────────
    match action {
        Action::EnterBrief => {
            editor.enter_brief();
        }
        // ---------------------------------------------------------------
        // Visual Selection Operations
        // ---------------------------------------------------------------
        Action::EnterVisual => {
            if editor.mode() == Mode::Visual {
                editor.enter_normal();
            } else {
                editor.set_mode(Mode::Visual);
            }
        }
        Action::EnterVisualLine => {
            if editor.mode() == Mode::VisualLine {
                editor.enter_normal();
            } else {
                editor.set_mode(Mode::VisualLine);
            }
        }
        Action::YankSelection => {
            let mode = editor.mode();
            let range = {
                let (win, buf) = editor.active_window_and_buf_mut();
                win.get_selection_range(buf, mode)
            };
            if let Some((start_char, end_char)) = range {
                let text = editor.buf().rope.slice(start_char..end_char).to_string();
                editor.clipboard = Some(text);
                editor.set_status_msg("Yanked selection", MessageKind::Info);
            }
            editor.enter_normal();
        }
        Action::DeleteSelection => {
            let mode = editor.mode();
            let range = {
                let (win, buf) = editor.active_window_and_buf_mut();
                win.get_selection_range(buf, mode)
            };
            if let Some((start_char, end_char)) = range {
                let text = editor.buf().rope.slice(start_char..end_char).to_string();
                editor.clipboard = Some(text);

                // Scope the mutable borrow of window and buffer for deletion
                let (win, buf) = editor.active_window_and_buf_mut();
                buf.push_undo(win.row, win.col);
                buf.rope.remove(start_char..end_char);
                buf.modified = true;

                let new_line = buf.rope.char_to_line(start_char);
                win.row = new_line;
                win.col = start_char.saturating_sub(buf.rope.line_to_char(new_line));
                win.clamp_cursor(buf);

                buf.parse_syntax();
            }
            editor.enter_normal();
        }
        Action::ChangeSelection => {
            let mode = editor.mode();
            let range = {
                let (win, buf) = editor.active_window_and_buf_mut();
                win.get_selection_range(buf, mode)
            };
            if let Some((start_char, end_char)) = range {
                // Scope the mutable borrow of window and buffer for deletion
                let (win, buf) = editor.active_window_and_buf_mut();
                buf.push_undo(win.row, win.col);
                buf.rope.remove(start_char..end_char);
                buf.modified = true;

                let new_line = buf.rope.char_to_line(start_char);
                win.row = new_line;
                win.col = start_char.saturating_sub(buf.rope.line_to_char(new_line));
                win.clamp_cursor(buf);

                buf.parse_syntax();
            }
            editor.enter_insert();
        }
        Action::IndentSelection => {
            let (win, buf) = editor.active_window_and_buf_mut();
            if let Some(anchor) = win.visual_anchor {
                let r1 = anchor.0.min(win.row);
                let r2 = anchor.0.max(win.row);
                buf.push_undo(win.row, win.col);
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
                buf.push_undo(win.row, win.col);
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
        // Movement — read-only buffer queries, cursor on Window
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
            {
                let (win, buf) = editor.active_window_and_buf_mut();
                movement::move_line_start(win, buf);
            }
            editor.snap_cursor_to_viewport();
            editor.comp.on_leave_insert();
        }
        Action::MoveLineEnd => {
            let current_mode = editor.mode();
            {
                let (win, buf) = editor.active_window_and_buf_mut();
                movement::move_line_end(win, buf, current_mode);
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
        // Editing — incremental parsing triggered inside helper operations
        // ---------------------------------------------------------------
        Action::Backspace => {
            let (win, buf) = editor.active_window_and_buf_mut();
            let (row, col) = (win.row, win.col);
            buf.push_undo(row, col);
            editing::backspace(win, buf);
            editor.comp.on_edit();
        }
        Action::DeleteCharForward => {
            let (win, buf) = editor.active_window_and_buf_mut();
            let (row, col) = (win.row, win.col);
            buf.push_undo(row, col);
            editing::delete_char_forward(win, buf);
            editor.comp.on_edit();
        }
        Action::DeleteCurrentLine => {
            let line_text = editor.line_text(editor.active_row());
            editor.clipboard = Some(format!("{}\n", line_text));
            let (win, buf) = editor.active_window_and_buf_mut();
            let (row, col) = (win.row, win.col);
            buf.push_undo(row, col);
            editing::delete_current_line(win, buf);
            editor.comp.on_edit();
        }
        Action::DeleteToEndOfLine => {
            let (win, buf) = editor.active_window_and_buf_mut();
            let (row, col) = (win.row, win.col);
            buf.push_undo(row, col);
            editing::delete_to_end_of_line(win, buf);
            editor.comp.on_edit();
        }
        Action::InsertNewline => {
            let (win, buf) = editor.active_window_and_buf_mut();
            let (row, col) = (win.row, win.col);
            buf.push_undo(row, col);
            editing::insert_newline(win, buf);
            buf.parse_syntax();
            editor.comp.on_edit();
        }
        Action::InsertTab => {
            let (win, buf) = editor.active_window_and_buf_mut();
            let (row, col) = (win.row, win.col);
            buf.push_undo(row, col);
            editing::insert_tab(win, buf);
            buf.parse_syntax();
            editor.comp.on_edit();
        }
        Action::Undo => {
            let (win, buf) = editor.active_window_and_buf_mut();
            if let Some(snap) = buf.pop_undo() {
                buf.rope = snap.rope;
                buf.modified = snap.modified;
                buf.parse_syntax(); // Way 1: Full Parse

                win.row = snap.cursor_row;
                win.col = snap.cursor_col;
            }
            editor.comp.on_leave_insert();
        }
        Action::IndentLine => {
            let (win, buf) = editor.active_window_and_buf_mut();
            let (row, col) = (win.row, win.col);
            buf.push_undo(row, col);
            editing::indent_line(win, buf);
            editor.comp.on_edit();
        }
        Action::OutdentLine => {
            let (win, buf) = editor.active_window_and_buf_mut();
            let (row, col) = (win.row, win.col);
            buf.push_undo(row, col);
            editing::outdent_line(win, buf);
            editor.comp.on_edit();
        }

        Action::InsertChar(ch) => {
            let (win, buf) = editor.active_window_and_buf_mut();
            let (row, col) = (win.row, win.col);
            buf.push_undo(row, col);
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
            buf.push_undo(row, col);
            editing::insert_newline_below(win, buf);
            buf.parse_syntax();
            let _ = buf;
            editor.enter_insert();
        }
        Action::InsertNewlineAbove => {
            let (win, buf) = editor.active_window_and_buf_mut();
            let (row, col) = (win.row, win.col);
            buf.push_undo(row, col);
            editing::insert_newline_above(win, buf);
            buf.parse_syntax();
            let _ = buf;
            editor.enter_insert();
        }
        Action::EnterCommand => {
            editor.enter_command();
        }
        Action::EnterNormal => {
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
            editor.enter_normal();
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
            editor.set_mode(Mode::Search);
        }
        Action::SearchCurrentWord => {
            if let Some(word) = editor.get_word_under_cursor() {
                // Set search query and feedback message
                editor.last_search_query = Some(word.clone());
                editor.set_status_msg(&format!("/{}", word), MessageKind::Info);

                // Immediately delegate execution to SearchNext to jump forward
                execute_action(editor, Action::SearchNext);
            } else {
                editor.set_status_msg("No word under cursor", MessageKind::Error);
            }
        }
        Action::ExecuteSearch => {
            let query = editor.command().to_string();
            editor.set_mode(Mode::Normal);

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
                    // Wrap-around to the beginning of the file
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
                        // Wrap-around back to top (wrapscan on)
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

                // FIX: Convert char offset to byte offset before slicing
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
                        // Wrap-around back to bottom (wrapscan on)
                        found_char = text[start_byte..].rfind(&query).map(|rel_byte| {
                            let abs_byte = start_byte + rel_byte;
                            text[..abs_byte].chars().count()
                        });
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
            editor.open_mru_popup();
        }

        Action::FunctionList => {
            // Update namespace targets
            let entries = crate::popup::function_list::extract_functions(editor.buf());
            editor.popup.function_list =
                Some(crate::popup::function_list::FunctionListPopup::new(entries));
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
            editor.open_git_log();
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
            editor.force_quit(); // <-- ForceQuit already handles positional saves and exits
        }

        Action::BookmarkSet => {
            editor.pending_input = PendingInput::SetBookmark;
            editor.set_status_msg("Mark: press a letter (a-z)", MessageKind::Info);
        }

        Action::BookmarkGoto => {
            editor.pending_input = PendingInput::GotoBookmark;
            editor.set_status_msg(
                "Jump: press a letter or ` for last position",
                MessageKind::Info,
            );
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
        // ── System clipboard ───────────────────────────────────────
        Action::YankToSystemClipboard => {
            editor.yank_to_system_clipboard();
        }
        Action::PasteFromSystemClipboard => {
            editor.paste_from_system_clipboard();
        }
        Action::CutToSystemClipboard => {
            editor.cut_to_system_clipboard();
        }
        Action::YankWordToSystemClipboard => {
            editor.yank_word_to_system_clipboard();
        }
        Action::PutFromSystemClipboardBelow => {
            editor.put_from_system_clipboard_below();
        }

        //-- Action::ExitMode execute_action (anchor dont removed) --//
        Action::ExitMode => {
            let current_mode = editor.mode();

            if current_mode == Mode::Command {
                // Returning from Command mode → go back to where we came from
                let target = editor.prev_mode;
                editor.set_mode(target);
                editor.clear_status_msg();
            } else if current_mode == Mode::Brief {
                // In Brief mode, Esc just clears completions/status
                // but does NOT change mode
                editor.clear_completions();
                editor.clear_status_msg();
            } else if current_mode == Mode::Insert {
                // In Insert mode, Esc goes to Normal
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
                // Returning from Search mode → go back to previous mode
                let target = editor.prev_mode;
                editor.set_mode(target);
                editor.clear_status_msg();
            } else if current_mode == Mode::Visual || current_mode == Mode::VisualLine {
                editor.enter_normal();
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
            editor.cmd_history_idx = None; // Reset search position on edit
            editor.history_search_prefix = None;
            editor.comp.on_edit();
        }
        Action::CommandChar(ch) => {
            editor.push_command(ch);
            editor.cmd_history_idx = None; // Reset search position on edit
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
                // Pass history list to complete_command
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

                // Clamp cursor in case rustfmt (or any save hook)
                // shortened the file, which would leave win.row out of bounds.
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
            let line_text = editor.line_text(editor.active_row());
            editor.clipboard = Some(format!("{}\n", line_text));
            editor.set_status_msg("Yanked 1 line", MessageKind::Info);
        }
        Action::Paste => {
            if let Some(text) = editor.clipboard.clone() {
                let (win, buf) = editor.active_window_and_buf_mut();
                let (row, col) = (win.row, win.col);
                buf.push_undo(row, col);
                if text.ends_with('\n') {
                    editing::paste_line_below(win, buf, &text);
                } else {
                    editing::paste_text(win, buf, &text);
                }
                let _ = buf;
                editor.comp.on_edit();
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
            let buf = editor.buf_mut();
            if buf.bookmarks.contains(&row) {
                buf.bookmarks.remove(&row);
                editor.set_status_msg("Bookmark removed", MessageKind::Info);
            } else {
                buf.bookmarks.insert(row);
                editor.set_status_msg("Bookmark added", MessageKind::Info);
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
        // Vim Dot Repeat Execution
        // ---------------------------------------------------------------
        Action::RepeatLastChange => {
            editor.repeat_last_action();
        }
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

// ---------------------------------------------------------------------------
// get_all_mode_bindings  (reference popup helper)
// ---------------------------------------------------------------------------

/// Return every known binding for `mode` as `(key, description)` pairs.
pub fn get_all_mode_bindings(mode: Mode) -> Vec<(String, String)> {
    match mode {
        Mode::Normal => get_default_actions()
            .into_iter()
            .map(|(key, action)| (key.to_string(), action_display_name(&action)))
            .collect(),

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
            ("Alt+s".into(), "Save".into()),
            ("Alt+q".into(), "Quit".into()),
            ("Alt+w".into(), "Force quit".into()),
            ("Alt+d".into(), "Delete line".into()),
            ("Alt+k".into(), "Delete to EOL".into()),
            ("Alt+u".into(), "Undo".into()),
            ("Alt+y".into(), "Yank line".into()),
            ("Alt+p".into(), "Paste".into()),
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
            ("Ctrl+s".into(), "Save".into()),
            ("Ctrl+c".into(), "Exit to Normal".into()),
            ("Ctrl+n/p".into(), "Cycle completion".into()),
        ],

        Mode::Visual | Mode::VisualLine => vec![
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
fn get_active_bindings(config: &Config, mode: Mode) -> &std::collections::HashMap<String, String> {
    match mode {
        Mode::Normal => &config.keybindings.normal,
        Mode::Insert => &config.keybindings.insert,
        Mode::Brief => &config.keybindings.brief,
        Mode::Command => &config.keybindings.command,
        Mode::Visual | Mode::VisualLine => &config.keybindings.visual,
        Mode::Search => &config.keybindings.command,
        Mode::LlmPrompt => &config.keybindings.command,
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
            return action_str.parse::<Action>().ok();
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
