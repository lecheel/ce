// File: ./ed/repeat.rs

use crate::ed::editor::Editor;
use crate::ed::ext::CommandResult;
use crate::ed::mode::MessageKind;
use crate::keybind::bindings::Action;

// Note: If you have these defined in other modules, the compiler will resolve them
#[allow(unused_imports)]
// use crate::ed::build::BuildExt;
#[allow(unused_imports)]
// use crate::ed::llm_ext::LlmExt;
#[allow(unused_imports)]
// use crate::llm::LlmPreset;

/// Types of actions that can be repeated with dot command
#[derive(Clone, Debug, PartialEq)]
pub enum RepeatableAction {
    /// Insert text at cursor position
    Insert(String),
    /// Delete characters (count, direction)
    DeleteChars {
        count: usize,
        direction: DeleteDirection,
    },
    // RipgrepNextResult,
    // RipgrepPrevResult,
    // QuickfixNext,
    // QuickfixPrev,
    /// Switch to the next buffer
    BufferNext,
    /// Switch to the previous buffer
    BufferPrev,
    /// Navigate to the next quickfix/ripgrep result
    QuickfixNext,
    /// Navigate to the previous quickfix/ripgrep result
    QuickfixPrev,
    /// Delete line
    DeleteLine,
    /// Delete to end of line (d$)
    DeleteToLineEnd,
    /// Delete to start of line (d0 / Ctrl-U in insert)
    DeleteToLineStart,
    /// Delete word forward (dw)
    DeleteWordForward,
    /// Delete word backward (Ctrl-W in insert, db in normal)
    DeleteWordBack,
    DeleteAroundFunction,
    ToggleComment,
    /// Change (delete + insert)
    Change {
        deleted: String,
        inserted: String,
    },
    /// Replace character
    ReplaceChar(char),
    /// Paste from register
    Paste {
        register: char,
        after_cursor: bool,
    },
    /// Indent/outdent
    Indent {
        count: usize,
        outdent: bool,
    },
    IndentTs {
        count: usize,
    },
    /// Join lines
    JoinLines {
        count: usize,
    },
    /// Substitute (e.g., `s/old/new/`)
    Substitute {
        pattern: String,
        replacement: String,
        flags: String,
    },
    /// LLM quick action (translate, explain, summarize, check English)
    /// On repeat, re-grabs the current text under cursor.
    // LlmQuickAction {
    // preset: LlmPreset,
    // },
    /// Custom command
    Custom(String),
}

#[derive(Clone, Debug, PartialEq)]
pub enum DeleteDirection {
    Left,
    Right,
}

/// Stores the last action for dot repeat
#[derive(Clone, Debug)]
pub struct LastAction {
    pub action: RepeatableAction,
    pub count: usize,
}

pub trait RepeatExt {
    /// Repeat the last action (dot command)
    fn repeat_last_action(&mut self, explicit_count: usize) -> CommandResult;

    /// Record an action for potential repetition
    fn record_action(&mut self, action: RepeatableAction, count: usize);

    /// Clear the last action (e.g., after undo)
    fn clear_last_action(&mut self);
}

impl RepeatExt for Editor {
    fn repeat_last_action(&mut self, explicit_count: usize) -> CommandResult {
        let action = self.last_action.action.clone();

        // Vim rules for count with dot:
        // If a count is given to `.` (e.g. `3.`), it overrides the original count.
        // Otherwise, the original count (e.g. from `2dd`) is used.
        let effective_count = if explicit_count > 1 {
            explicit_count
        } else {
            self.last_action.count
        };

        // Inject the effective count so execute_action's loops work correctly
        self.current_count = effective_count;

        let action_to_process = match action {
            RepeatableAction::BufferNext => {
                self.switch_next_buffer();
                self.current_count = 0; // reset since we bypass execute_action
                return CommandResult::Handled;
            }
            RepeatableAction::BufferPrev => {
                self.switch_prev_buffer();
                self.current_count = 0;
                return CommandResult::Handled;
            }
            RepeatableAction::QuickfixNext => {
                self.quickfix_next();
                self.current_count = 0;
                return CommandResult::Handled;
            }
            RepeatableAction::QuickfixPrev => {
                self.quickfix_prev();
                self.current_count = 0;
                return CommandResult::Handled;
            }
            RepeatableAction::IndentTs { count: _ } => {
                self.set_status_msg("Cannot repeat indent_ts yet", MessageKind::Error);
                self.current_count = 0;
                return CommandResult::Handled;
            }
            RepeatableAction::DeleteLine => Action::DeleteCurrentLine,
            RepeatableAction::DeleteToLineEnd => Action::DeleteToEndOfLine,
            RepeatableAction::DeleteToLineStart => {
                self.set_status_msg(
                    "Delete to start of line not supported yet",
                    MessageKind::Error,
                );
                self.current_count = 0;
                return CommandResult::NotHandled;
            }
            RepeatableAction::DeleteWordForward => Action::DeleteInsideWord,
            RepeatableAction::DeleteWordBack => Action::DeleteInsideWord,
            RepeatableAction::DeleteAroundFunction => Action::DeleteInsideFunction,
            RepeatableAction::Insert(text) => {
                // Inline insertion replay
                if !text.is_empty() {
                    let (win, buf) = self.active_window_and_buf_mut();
                    let (row, col) = (win.row, win.col);
                    buf.push_undo(row, col);
                    for _ in 0..effective_count {
                        crate::ed::editing::paste_text(win, buf, &text);
                    }
                }
                self.current_count = 0;
                return CommandResult::Handled;
            }
            RepeatableAction::DeleteChars { direction, .. } => {
                if direction == DeleteDirection::Left {
                    Action::Backspace
                } else {
                    Action::DeleteCharForward
                }
            }
            RepeatableAction::ReplaceChar(c) => Action::InsertChar(c),
            RepeatableAction::Paste { .. } => Action::Paste,
            RepeatableAction::Indent { outdent, .. } => {
                if outdent {
                    Action::OutdentLine
                } else {
                    Action::IndentLine
                }
            }
            RepeatableAction::JoinLines { .. } => {
                self.set_status_msg("Join lines not supported yet", MessageKind::Error);
                self.current_count = 0;
                return CommandResult::NotHandled;
            }
            _ => {
                self.set_status_msg("Cannot repeat this action", MessageKind::Error);
                self.current_count = 0;
                return CommandResult::NotHandled;
            }
        };

        // Execute the mapped Action through standard bindings pipeline.
        // execute_action will consume `self.current_count` and reset it to 0.
        crate::keybind::bindings::execute_action(self, action_to_process);

        CommandResult::Handled
    }

    fn record_action(&mut self, action: RepeatableAction, count: usize) {
        self.last_action = LastAction { action, count };
        self.repeat_pending = true;
    }

    fn clear_last_action(&mut self) {
        self.last_action = LastAction::default();
        self.repeat_pending = false;
    }
}

impl Default for LastAction {
    fn default() -> Self {
        Self {
            action: RepeatableAction::Insert(String::new()),
            count: 1,
        }
    }
}
