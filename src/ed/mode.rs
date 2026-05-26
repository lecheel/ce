//! Core editor mode definitions.
//!
//! `Mode` represents the three editor modes (Normal, Insert, Command) and is
//! shared across every module that needs to reason about the current editor
//! state.  Keeping it in its own leaf module avoids circular dependencies
//! between `ed`, `keybind`, `comp`, and `render`.

// ---------------------------------------------------------------------------
// Mode — the three editor modes
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, Default, Hash, Eq, PartialEq)]
pub enum Mode {
    #[default]
    Normal,
    Insert,
    Command,
    Brief,
    Visual,     // Character-wise selection
    VisualLine, // Line-wise selection
    Search,
    LlmPrompt,
}

// ---------------------------------------------------------------------------
// MessageKind — status-bar message severity
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MessageKind {
    Info,
    Error,
    Success,
}
