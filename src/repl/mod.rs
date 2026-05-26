//! Command / search REPL module.
//!
//! - [`command`] — `:` command parsing and execution

pub mod command;

#[allow(unused_imports)]
pub use command::{complete_command, execute};
