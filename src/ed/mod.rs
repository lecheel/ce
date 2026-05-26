pub mod buffer;
pub mod clipboard;
pub mod diff_align;
pub mod editing;
pub mod editor;
pub mod ext;
pub mod handle_git;
pub mod handle_key;
pub mod handle_mru;
pub mod health;
pub mod implex;
pub mod misc_helper;
pub mod mode;
pub mod movement;
pub mod ripgrep;
pub mod syntax;
pub mod window;

// Register the new repeat and gutter modules in the compiler tree
pub mod gutter;
pub mod repeat;

#[allow(unused_imports)]
pub use buffer::{detect_language, Buffer, UndoSnapshot};
pub use editor::Editor;
#[allow(unused_imports)]
pub use ext::{CommandResult, EditorExt};
#[allow(unused_imports)]
pub use mode::{MessageKind, Mode};
