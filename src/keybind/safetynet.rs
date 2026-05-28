// keybind/safetynet.rs
//! Safety checks for around-function operations.

use crate::Editor;

pub const AROUND_FN_MAX_LINES: usize = 500;

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
pub fn check_around_function_safetynet(editor: &Editor) -> Result<(), String> {
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
