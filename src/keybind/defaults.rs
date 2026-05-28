// keybind/defaults.rs
//! Default keybinding table.

use crate::keybind::actions::Action;

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
        ("%", Action::MatchBracket),
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
        ("delete", Action::DeleteCharForward),
        ("u", Action::Undo),
        ("p", Action::Paste),
        ("g c c", Action::ToggleComment),
        // bookmarks
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
        ("y w", Action::YankCurrentWord),
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
        ("d w", Action::DeleteWordForward),
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
