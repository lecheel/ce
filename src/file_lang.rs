// src/file_lang.rs
//! Map file extensions to LSP language IDs and server commands.

use std::path::Path;

pub struct FileLang {
    pub language_id: &'static str,
    pub lsp_command: Option<&'static str>,
    pub lsp_args: &'static [&'static str],
}

impl FileLang {
    pub fn from_path(path: &Path) -> Self {
        let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");

        match ext {
            "rs" => FileLang {
                language_id: "rust",
                lsp_command: Some("rust-analyzer"),
                lsp_args: &[],
            },
            "py" => FileLang {
                language_id: "python",
                lsp_command: Some("pylsp"),
                lsp_args: &[],
            },
            "ts" => FileLang {
                language_id: "typescript",
                lsp_command: Some("typescript-language-server"),
                lsp_args: &["--stdio"],
            },
            "js" => FileLang {
                language_id: "javascript",
                lsp_command: Some("typescript-language-server"),
                lsp_args: &["--stdio"],
            },
            "go" => FileLang {
                language_id: "go",
                lsp_command: Some("gopls"),
                lsp_args: &[],
            },
            "c" | "h" => FileLang {
                language_id: "c",
                lsp_command: Some("clangd"),
                lsp_args: &[],
            },
            "cpp" | "cc" | "cxx" | "hpp" | "hh" => FileLang {
                language_id: "cpp",
                lsp_command: Some("clangd"),
                lsp_args: &[],
            },
            "lua" => FileLang {
                language_id: "lua",
                lsp_command: Some("lua-language-server"),
                lsp_args: &[],
            },
            "zig" => FileLang {
                language_id: "zig",
                lsp_command: Some("zls"),
                lsp_args: &[],
            },
            _ => FileLang {
                language_id: "plaintext",
                lsp_command: None,
                lsp_args: &[],
            },
        }
    }
}
