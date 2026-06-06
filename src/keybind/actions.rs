// keybind/actions.rs
//! Editor action definitions.

use serde::{Deserialize, Serialize};
use strum::{AsRefStr, EnumIter, EnumString};

// ---------------------------------------------------------------------------
// Action — representation of all editor commands
// ---------------------------------------------------------------------------
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize,  AsRefStr, EnumString, EnumIter)]
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
    Redo,
    InsertTab,
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
    EasyMotion,

    // Completion
    AcceptCompletion,
    CycleCompletionNext,
    CycleCompletionPrev,
    ManualComplete,
    ToggleTrueFalse,
    SwissKnife,

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

    // ── Tag navigation ──────────────────────────────────────────────
    TagJump,      // C-]  — jump to tag under cursor
    TagBack,      // C-t  — return from tag jump

    // Config Toggles
    TogglePopup,
    InputBox,

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
    EnterWindowNav,
    EnterCloseWindowNav,
    CloseWindowLeft,    
    CloseWindowRight,   
    CloseWindowUp,      
    CloseWindowDown, 
 
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
    YankAroundFunction,
    YankInsideFunction,
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
    FdSearch, //tag_fd_action
    SearchSymbols,
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
    /// Open a single-buffer codecompanion-style LLM chat
    CodeLlmChat,
    /// Send the prompt in a CodeLlm buffer
    CodeLlmSend,
    LlmExplainFunction,
    LlmReview,    
    // LlmFix,       
    // LlmGenerate,  
    LlmAddToChat,
    TransZhLine,
    FnInfo,
    
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

    /// Entered `r` prefix — waiting for the replacement character.
    EnterReplace,
    /// Replace character(s) under cursor with `ch`.
    ReplaceChar(char),    

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

    // Build
    BuildRun,
    BuildNextError,
    BuildPrevError,
    BuildGotoError,
    BuildClose,

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
    // Chained actions from config "action1 | action2"
    #[strum(disabled)]
    Chain(Vec<Action>),
    // Conditional chain "action1 && action2" — stops on first failure
    #[strum(disabled)]
    Then(Vec<Action>),    
}

impl Action {
    /// Parses an action string, handling strum aliases and dynamic variants.
    /// Replaces the standard `FromStr` to support custom logic.
    pub fn parse(s: &str) -> Result<Self, anyhow::Error> {
        let s = s.trim();

        // ── Conditional chain: "toggle_comment && move_down" ──────
        if s.contains("&&") {
            let parts: Vec<&str> = s.split("&&").map(|p| p.trim()).collect();
            if parts.len() > 1 {
                let actions: Vec<Action> = parts
                    .iter()
                    .map(|p| Action::parse(p))
                    .collect::<Result<Vec<_>, _>>()?;
                return Ok(Action::Then(actions));
            }
        }

        // ── Chain: "search_next | move_down" ──────────────────────
        if s.contains('|') {
            let parts: Vec<&str> = s.split('|').map(|p| p.trim()).collect();
            if parts.len() > 1 {
                let actions: Vec<Action> = parts
                    .iter()
                    .map(|p| Action::parse(p))
                    .collect::<Result<Vec<_>, _>>()?;
                return Ok(Action::Chain(actions));
            }
        }

        let lower = s.to_lowercase().replace('_', "");

        // ── FIX: Only match "switchbuffer:N" or "bufN" (digit immediately after) ──
        if lower.starts_with("switchbuffer:") {
            let idx_str = lower.trim_start_matches("switchbuffer:");
            let idx: usize = idx_str
                .parse::<usize>() // Specified the type here
                .map_err(|_| anyhow::anyhow!("Invalid switch_buffer index: {}", idx_str))?
                .saturating_sub(1); // 1-based to 0-based
            return Ok(Action::SwitchBuffer(idx));
        }

        // ── Standard strum-based parsing for all other actions ──────
        if let Ok(action) = s.parse::<Self>() {
            return Ok(action);
        }

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
            Action::Chain(_) => "chain".to_string(),
            Action::Then(_) => "then".to_string(),
            // Every other variant is fully automatic!
            // e.g., Action::MoveLeft.as_ref() -> "move_left"
            _ => self.as_ref().to_string(),
        }
    }

    /// Returns true if this action is a "jump" motion that should update
    /// the jump-back register for `` (backtick) ping-pong.
    pub fn is_jump(&self) -> bool {
        match self {
            Action::Chain(ref actions) | Action::Then(ref actions) => {
                actions.iter().any(|a| a.is_jump())
            }
            _ => matches!(
                self,
                Action::MoveToFirstLine
                    | Action::MoveToLastLine
                    | Action::PageUp
                    | Action::PageDown
                    | Action::SearchNext
                    | Action::SearchPrev
                    | Action::SearchCurrentWord
                    | Action::MatchBracket
                    | Action::TagJump
                    | Action::HunkNext
                    | Action::HunkPrev // Note: BookmarkGoto and JumpLastPosition handle their own
                                       // jump-back saving internally, so they are excluded here.
            ),
        }
    }

    /// Returns true if this action modifies the buffer text.
    /// Used to gate expensive operations like syntax parsing.
    pub fn modifies_buffer(&self) -> bool {
        if let Action::Chain(ref actions) | Action::Then(ref actions) = self {
            return actions.iter().any(|a| a.modifies_buffer());
        }
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
                | Action::ToggleTrueFalse
                | Action::ReplaceChar(_)
                | Action::SwissKnife
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
