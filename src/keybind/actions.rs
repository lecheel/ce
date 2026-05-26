//! Editor action definitions.
//!
//! Every user interaction is converted into an `EditorAction` before being
//! processed by the editor.  The action enum is pure data — it carries no
//! modalkit dependencies — so it can be used freely across all modules.
//!
//! Mode transitions are represented as actions (e.g. `EnterInsert`) so that
//! side-effects (clearing ghost text, pushing undo snapshots) can be triggered
//! during execution.  The **authoritative** mode is tracked by the
//! `ModalMachine` in `keybind::machine`; the `Editor` mirrors it via
//! `set_mode()`.

// ---------------------------------------------------------------------------
// EditorAction — the single enum that drives every editor mutation
// ---------------------------------------------------------------------------
