//! Rendering helper functions.
//!
//! Character-width calculations, digit counting, and language display names.
//! These are pure functions with no dependency on the editor state.

// ---------------------------------------------------------------------------
// Numeric helpers
// ---------------------------------------------------------------------------

/// Number of decimal digits needed to represent `n`.
pub fn digit_count(mut n: usize) -> usize {
    if n == 0 {
        return 1;
    }
    let mut count = 0;
    while n > 0 {
        count += 1;
        n /= 10;
    }
    count
}

// ---------------------------------------------------------------------------
// Display-width helpers
// ---------------------------------------------------------------------------

/// Visual display width of a string (accounts for wide CJK characters and
/// tabs).
pub fn display_width(s: &str) -> usize {
    s.chars().map(char_width).sum()
}

/// Visual width of a single character.
pub fn char_width(ch: char) -> usize {
    match ch {
        '\t' => 4,
        c if c.is_ascii_control() => 0,
        c if c.is_ascii() => 1,
        '\u{1100}'..='\u{115F}'
        | '\u{2E80}'..='\u{303F}'
        | '\u{3040}'..='\u{33BF}'
        | '\u{3400}'..='\u{4DBF}'
        | '\u{4E00}'..='\u{A4CF}'
        | '\u{AC00}'..='\u{D7AF}'
        | '\u{F900}'..='\u{FAFF}'
        | '\u{FE10}'..='\u{FE1F}'
        | '\u{FE30}'..='\u{FE6F}'
        | '\u{FF00}'..='\u{FF60}'
        | '\u{FFE0}'..='\u{FFE6}'
        | '\u{20000}'..='\u{2FFFD}'
        | '\u{30000}'..='\u{3FFFD}' => 2,
        _ => 1,
    }
}

// ---------------------------------------------------------------------------
// Language display
// ---------------------------------------------------------------------------

/// Human-friendly language name for the status bar.
pub fn detect_display_lang(filename: Option<&str>) -> &'static str {
    match filename.and_then(|p| p.rsplit('.').next()) {
        Some("rs") => "Rust",
        Some("py") => "Python",
        Some("js") => "JavaScript",
        Some("ts") => "TypeScript",
        Some("tsx") => "TSX",
        Some("jsx") => "JSX",
        Some("go") => "Go",
        Some("java") => "Java",
        Some("c") | Some("h") => "C",
        Some("cpp") | Some("hpp") | Some("cc") | Some("cxx") => "C++",
        Some("rb") => "Ruby",
        Some("php") => "PHP",
        Some("swift") => "Swift",
        Some("kt") | Some("kts") => "Kotlin",
        Some("sh") | Some("bash") => "Shell",
        Some("sql") => "SQL",
        Some("html") | Some("htm") => "HTML",
        Some("css") => "CSS",
        Some("scss") => "SCSS",
        Some("json") => "JSON",
        Some("yaml") | Some("yml") => "YAML",
        Some("md") => "Markdown",
        Some("lua") => "Lua",
        Some("vim") => "Vim",
        Some("toml") => "TOML",
        Some("dart") => "Dart",
        _ => "Plain Text",
    }
}

/// Shrinks a path string to "fish style" (e.g., "/home/user/project/src/main.rs" -> "/h/u/p/s/main.rs").
/// Collapses each directory component to its first character (preserving dot prefixes like ".c" for ".config")
/// and keeps the final filename fully intact.
pub fn contract_path_fish(path_str: &str) -> String {
    let normalized = path_str.replace('\\', "/");
    if normalized.is_empty() {
        return String::new();
    }

    let parts: Vec<&str> = normalized.split('/').collect();
    if parts.len() <= 1 {
        return normalized;
    }

    let last_idx = parts.len() - 1;
    let mut contracted = Vec::with_capacity(parts.len());

    for (i, part) in parts.iter().enumerate() {
        if i == last_idx {
            contracted.push(part.to_string());
        } else if part.is_empty() {
            contracted.push(String::new());
        } else if part.starts_with('.') && part.len() > 1 {
            // Keep the dot and the first char for hidden/dot directories
            let first_char = part.chars().nth(1).unwrap_or(' ');
            contracted.push(format!(".{}", first_char));
        } else {
            let first_char = part.chars().next().unwrap_or(' ');
            contracted.push(first_char.to_string());
        }
    }

    contracted.join("/")
}
