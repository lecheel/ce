// keybind/actions.rs
//! Editor action definitions.

use serde::{Deserialize, Serialize};
use strum::{AsRefStr, EnumIter, EnumString};

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
    DeleteToEndOfFile,
    InsertNewline,
    InsertNewlineBelow,
    InsertNewlineAbove,
    Undo,
    InsertTab,
    IndentLine,
    OutdentLine,
    ToggleComment,

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
    CommandEnterRegisterMode,
    CommandInsertFilename,
    CommandInsertWord,
    CommandInsertLine,
    CommandCancelRegister,    

    // Copy / Paste Register
    YankCurrentLine,
    YankCurrentWord,
    YankWordToSystemClipboard,
    BriefCutSelection,
    BriefCopySelection,
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
    DeleteWordForward,
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
    MatchBracket,

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

    /// Returns true if this action is a "jump" motion that should update
    /// the jump-back register for `` (backtick) ping-pong.
    pub fn is_jump(&self) -> bool {
        matches!(
            self,
            Action::MoveToFirstLine
                | Action::MoveToLastLine
                | Action::PageUp
                | Action::PageDown
                | Action::SearchNext
                | Action::SearchPrev
                | Action::SearchCurrentWord
                | Action::MatchBracket
                | Action::HunkNext
                | Action::HunkPrev // Note: BookmarkGoto and JumpLastPosition handle their own
                                   // jump-back saving internally, so they are excluded here.
        )
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
                | Action::DeleteToEndOfFile
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
                | Action::DeleteWordForward
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
                | Action::ToggleComment
                | Action::BriefCopySelection
                | Action::BriefCutSelection
        )
    }
}

/// Strum-only lookup (avoids recursing through our custom FromStr).
pub fn action_from_strum(s: &str) -> Option<Action> {
    use strum::IntoEnumIterator;
    for variant in Action::iter() {
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
