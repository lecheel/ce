//! Extension trait for the editor.
//!
//! `EditorExt` is the primary extension point for plugins and future features
//! (popup menus, custom commands, etc.).  Extensions receive minimal, stable
//! callbacks and return actions that the editor loop processes.
//!
//! This module is intentionally small — the real extension logic will live in
//! concrete implementations (e.g. `popup::MenuExt`, custom user plugins).

// ---------------------------------------------------------------------------
// CommandResult — what an extension returns from a command hook
// ---------------------------------------------------------------------------

/// Returned by [`EditorExt::on_command`] to indicate whether the extension
/// handled the command.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommandResult {
    /// The extension handled the command; no further processing needed.
    Handled,
    /// The extension did not handle the command; continue with built-in logic.
    NotHandled,
}

// ---------------------------------------------------------------------------
// EditorExt — the extension trait
// ---------------------------------------------------------------------------

/// Trait for editor extensions.
///
/// All methods have default no-op implementations so that implementors only
/// need to override the hooks they care about.
pub trait EditorExt: Send + Sync {
    /// Human-readable name for the extension (used in diagnostics / UI).
    fn name(&self) -> &str;

    /// Called before the built-in command handler processes a command string.
    ///
    /// If the extension returns [`CommandResult::Handled`], the built-in
    /// handler is skipped entirely.
    fn on_command(&self, _cmd: &str) -> CommandResult {
        CommandResult::NotHandled
    }
}
