//! Tree-sitter syntax parsing, highlighting, and text objects.

use ratatui::style::{Color, Modifier, Style};
use ropey::Rope;
use tree_sitter::{Node, Point, Tree};

// ---------------------------------------------------------------------------
// Syntax State
// ---------------------------------------------------------------------------

pub struct SyntaxState {
    pub tree: Option<Tree>,
    parser: Option<tree_sitter::Parser>,
    pub language_id: Option<String>,
}

impl std::fmt::Debug for SyntaxState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SyntaxState")
            .field("tree", &self.tree.is_some())
            .finish()
    }
}

impl Clone for SyntaxState {
    fn clone(&self) -> Self {
        Self {
            tree: self.tree.clone(),
            parser: None, // Parser doesn't impl Clone; it gets recreated on next parse
            language_id: self.language_id.clone(),
        }
    }
}

impl SyntaxState {
    pub fn new() -> Self {
        Self {
            tree: None,
            parser: Some(tree_sitter::Parser::new()),
            language_id: None,
        }
    }

    /// Way 1: Force a full parse from scratch.
    /// Ideal for large blocks, pastes, loading, saving, or structural deletes.
    pub fn parse_full(&mut self, rope: &Rope, language_id: Option<&str>) {
        let lang_id = language_id.unwrap_or("unknown");
        self.language_id = Some(lang_id.to_string()); // ← must be before early return

        if matches!(lang_id, "gitlog" | "gitstatus") {
            self.tree = None;
            return; // line-based only, no tree-sitter needed
        }

        if let Some(parser) = &mut self.parser {
            match get_language(lang_id) {
                Some(lang) => {
                    let _ = parser.set_language(&lang);
                    let text = rope.to_string();
                    self.tree = parser.parse(&text, None);
                }
                None => {
                    self.tree = None;
                }
            }
        }
    }

    /// Way 2: Perform an incremental parse using `InputEdit`.
    /// Ideal for lightweight edits like single-character typing, backspaces, and single deletes.
    pub fn parse_incremental(
        &mut self,
        rope: &Rope,
        language_id: Option<&str>,
        edit: tree_sitter::InputEdit,
    ) {
        let lang_id = language_id.unwrap_or("unknown");

        // Fall back to a full parse if the language changed
        if self.language_id.as_deref() != Some(lang_id) {
            self.parse_full(rope, language_id);
            return;
        }

        if let Some(parser) = &mut self.parser {
            if let Some(mut tree) = self.tree.take() {
                // Apply the edit to the current AST first
                tree.edit(&edit);

                let text = rope.to_string();
                // Pass the edited tree to parser.parse to perform incremental updates
                self.tree = parser.parse(&text, Some(&tree));
            } else {
                // No existing tree to update; fall back to a full parse
                self.parse_full(rope, language_id);
            }
        }
    }

    /// Parse or incrementally update the syntax tree.
    pub fn parse(&mut self, rope: &Rope, language_id: Option<&str>) {
        let lang_id = language_id.unwrap_or("unknown");

        // Only re-parse if the language changed or we have a parser
        if self.language_id.as_deref() != Some(lang_id) {
            self.language_id = Some(lang_id.to_string());
            if matches!(lang_id, "gitlog" | "gitstatus") {
                self.tree = None;
                return;
            }
            if let Some(parser) = &mut self.parser {
                if let Some(lang) = get_language(lang_id) {
                    let _ = parser.set_language(&lang);
                } else {
                    self.tree = None;
                    return;
                }
            }
        }

        if matches!(lang_id, "gitlog" | "gitstatus") {
            self.tree = None;
            return;
        }

        if let Some(parser) = &mut self.parser {
            let text = rope.to_string();
            let tree = parser.parse(&text, self.tree.as_ref());
            self.tree = tree;
        }
    }

    /// Get syntax styles for a specific line.
    /// Accept raw line text to handle non-AST formats like "gitlog".
    pub fn get_line_highlights(&self, row: usize, line_text: &str) -> Vec<Option<Style>> {
        let line_len = line_text.chars().count();
        let mut char_styles = vec![None; line_len];

        match self.language_id.as_deref() {
            Some("gitlog") => {
                if let Some(style) = style_for_git_log_line(line_text) {
                    char_styles.fill(Some(style));
                }
                return char_styles; // never touches tree
            }
            Some("gitstatus") => {
                return style_for_git_status_line(line_text);
            }
            Some("rg") => {
                return style_for_rg_line(line_text);
            }
            None => return char_styles,
            _ => {}
        }

        if let Some(tree) = &self.tree {
            let root = tree.root_node();
            Self::collect_highlights(root, row, &mut char_styles);
        }
        char_styles
    }

    fn collect_highlights(node: Node, row: usize, char_styles: &mut Vec<Option<Style>>) {
        // Prune branches that don't intersect the target row
        if node.start_position().row > row || node.end_position().row < row {
            return;
        }

        if let Some(style) = style_for_kind(node.kind()) {
            let start_col = if node.start_position().row < row {
                0
            } else {
                node.start_position().column
            };
            let end_col = if node.end_position().row > row {
                char_styles.len()
            } else {
                node.end_position().column.min(char_styles.len())
            };

            for i in start_col..end_col {
                char_styles[i] = Some(style);
            }
        }

        // Recurse into children. Child styles will overwrite parent styles,
        // giving us correct "most specific node wins" behavior.
        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            Self::collect_highlights(child, row, char_styles);
        }
    }

    // -----------------------------------------------------------------------
    // Text Objects
    // -----------------------------------------------------------------------

    /// Find the range of a text object enclosing the cursor.
    pub fn text_object_range(
        &self,
        row: usize,
        col: usize,
        obj: TextObject,
        inside: bool,
    ) -> Option<(usize, usize, usize, usize)> {
        // (start_row, start_col, end_row, end_col)
        let tree = self.tree.as_ref()?;
        let root = tree.root_node();
        let point = Point::new(row, col);

        let mut node = root.descendant_for_point_range(point, point)?;

        match obj {
            TextObject::Function => loop {
                let kind = node.kind();
                if kind.contains("function") || kind.contains("method") {
                    if inside {
                        if node.child_count() == 0 {
                            return None;
                        }
                        let body = node.child(node.child_count() - 1)?;
                        return Some((
                            body.start_position().row,
                            body.start_position().column,
                            body.end_position().row,
                            body.end_position().column,
                        ));
                    } else {
                        return Some((
                            node.start_position().row,
                            node.start_position().column,
                            node.end_position().row,
                            node.end_position().column,
                        ));
                    }
                }
                node = node.parent()?;
            },
            TextObject::Class => loop {
                let kind = node.kind();
                if kind.contains("class") || kind.contains("struct") || kind.contains("impl") {
                    if inside {
                        let body = node.child(node.child_count() - 1)?;
                        return Some((
                            body.start_position().row,
                            body.start_position().column,
                            body.end_position().row,
                            body.end_position().column,
                        ));
                    } else {
                        return Some((
                            node.start_position().row,
                            node.start_position().column,
                            node.end_position().row,
                            node.end_position().column,
                        ));
                    }
                }
                node = node.parent()?;
            },
            TextObject::Word => None,
            TextObject::Quotes => loop {
                let kind = node.kind();
                if kind.contains("string") {
                    if inside {
                        if node.child_count() >= 2 {
                            let start_n = node.child(1)?;
                            let end_n = node.child(node.child_count() - 2)?;
                            return Some((
                                start_n.start_position().row,
                                start_n.start_position().column,
                                end_n.end_position().row,
                                end_n.end_position().column,
                            ));
                        }
                    } else {
                        return Some((
                            node.start_position().row,
                            node.start_position().column,
                            node.end_position().row,
                            node.end_position().column,
                        ));
                    }
                }
                node = node.parent()?;
            },
            TextObject::Parens => loop {
                let kind = node.kind();
                if kind.contains("parenthesized")
                    || kind.contains("arguments")
                    || kind.contains("parameters")
                {
                    if inside {
                        if node.child_count() >= 2 {
                            let start_n = node.child(1)?;
                            let end_n = node.child(node.child_count() - 2)?;
                            return Some((
                                start_n.start_position().row,
                                start_n.start_position().column,
                                end_n.end_position().row,
                                end_n.end_position().column,
                            ));
                        }
                    } else {
                        return Some((
                            node.start_position().row,
                            node.start_position().column,
                            node.end_position().row,
                            node.end_position().column,
                        ));
                    }
                }
                node = node.parent()?;
            },
            TextObject::Braces => loop {
                let kind = node.kind();
                if kind.contains("block")
                    || kind.contains("body")
                    || kind == "object"
                    || kind == "initializer_list"
                    || kind == "field_declaration_list"
                {
                    if inside {
                        if node.child_count() >= 2 {
                            let start_n = node.child(1)?;
                            let end_n = node.child(node.child_count() - 2)?;
                            return Some((
                                start_n.start_position().row,
                                start_n.start_position().column,
                                end_n.end_position().row,
                                end_n.end_position().column,
                            ));
                        }
                    } else {
                        return Some((
                            node.start_position().row,
                            node.start_position().column,
                            node.end_position().row,
                            node.end_position().column,
                        ));
                    }
                }
                node = node.parent()?;
            },
            TextObject::Brackets => loop {
                let kind = node.kind();
                if kind.contains("array")
                    || kind.contains("subscript")
                    || kind.contains("index")
                    || kind.contains("bracket")
                {
                    if inside {
                        if node.child_count() >= 2 {
                            let start_n = node.child(1)?;
                            let end_n = node.child(node.child_count() - 2)?;
                            return Some((
                                start_n.start_position().row,
                                start_n.start_position().column,
                                end_n.end_position().row,
                                end_n.end_position().column,
                            ));
                        }
                    } else {
                        return Some((
                            node.start_position().row,
                            node.start_position().column,
                            node.end_position().row,
                            node.end_position().column,
                        ));
                    }
                }
                node = node.parent()?;
            },
        }
    }
    /// Extract the text of a tree-sitter Node from the Rope.
    fn extract_text(rope: &Rope, node: &tree_sitter::Node) -> String {
        let start_byte = node.start_byte();
        let end_byte = node.end_byte();

        // Clamp to rope bounds
        let rope_len = rope.len_bytes();
        if start_byte >= rope_len || end_byte > rope_len || start_byte > end_byte {
            return String::new();
        }

        let start = rope.byte_to_char(start_byte);
        let end = rope.byte_to_char(end_byte);

        if end <= start {
            return String::new();
        }

        rope.slice(start..end).to_string()
    }

    /// Find the current scope (impl/struct/class + function/method) at the cursor.
    pub fn current_scope(&self, rope: &Rope, row: usize, col: usize) -> Option<String> {
        let tree = self.tree.as_ref()?;
        let root = tree.root_node();
        let point = Point::new(row, col);
        let node = root.descendant_for_point_range(point, point)?;

        let mut fn_name = None;
        let mut ctx_name = None; // impl / struct / class / trait

        let mut current = Some(node);
        while let Some(n) = current {
            let kind = n.kind();

            // Functions / Methods
            let is_fn = kind == "function_item"
                || kind == "function_signature_item"
                || kind == "function_definition"
                || kind == "function_declaration"
                || kind == "arrow_function"
                || kind == "method_definition"
                || kind.contains("method"); // catches instance_method, class_method, etc.

            if fn_name.is_none() && is_fn {
                if let Some(name_node) = n.child_by_field_name("name") {
                    fn_name = Some(Self::extract_text(rope, &name_node));
                } else if let Some(child) = n.child(1) {
                    if child.is_named() {
                        fn_name = Some(Self::extract_text(rope, &child));
                    }
                }
            }

            // Context (impl, struct, enum, trait, class)
            let is_ctx = kind == "enum_item"
                || kind == "enum_specifier"
                || kind == "enum_declaration"
                || kind == "struct_item"
                || kind == "struct_specifier"
                || kind == "impl_item"
                || kind == "trait_item"
                || kind.contains("class"); // catches class, class_definition, class_declaration

            if ctx_name.is_none() && is_ctx {
                if let Some(name_node) = n.child_by_field_name("name") {
                    ctx_name = Some(Self::extract_text(rope, &name_node));
                } else if let Some(child) = n.child(1) {
                    if child.is_named() {
                        ctx_name = Some(Self::extract_text(rope, &child));
                    }
                }
            }

            current = n.parent();
        }

        match (ctx_name, fn_name) {
            (Some(ctx), Some(func)) => Some(format!("{}::{}", ctx, func)),
            (Some(ctx), None) => Some(ctx),
            (None, Some(func)) => Some(func),
            (None, None) => None,
        }
    }
}

// ---------------------------------------------------------------------------
// Text Object & Style Maps
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TextObject {
    Function,
    Class,
    Word,
    Quotes,
    Parens,
    Braces,
    Brackets,
}

fn get_language(id: &str) -> Option<tree_sitter::Language> {
    match id {
        "rust" => Some(tree_sitter_rust::LANGUAGE.into()),
        "python" => Some(tree_sitter_python::LANGUAGE.into()),
        "javascript" | "typescript" => Some(tree_sitter_javascript::LANGUAGE.into()),
        "diff" => Some(tree_sitter_diff::LANGUAGE.into()),
        _ => None,
    }
}

// ed/syntax.rs

#[rustfmt::skip]
fn style_for_kind(kind: &str) -> Option<Style> {
    match kind {
        // Keywords
        "fn" | "let" | "mut" | "if" | "else" | "return" | "struct" | "enum" | "impl" | "pub"
        | "use" | "mod" | "match" | "loop" | "while" | "for" | "in" | "break" | "continue"
        | "async" | "await" | "dyn" | "trait" | "where" | "ref" | "as" | "type" | "const"
        | "static" | "unsafe" | "extern" | "crate" | "super" | "self" | "true" | "false"
        | "def" | "class" | "import" | "from" | "try" | "except" | "finally" | "with" | "yield"
        | "lambda" | "pass" | "raise" | "global" | "nonlocal" | "assert" | "del" | "not"
        | "and" | "or" | "is" => Some(
            Style::default()
                .fg(Color::Rgb(198, 120, 221))
                .add_modifier(Modifier::BOLD),
        ),
        // Strings
        "string"
        | "string_content"
        | "raw_string_literal"
        | "string_literal"
        | "interpreted_string_literal" => Some(Style::default().fg(Color::Rgb(152, 195, 121))),
        // Types/Structs/Enums
        "type_identifier" | "struct_item" | "enum_item" | "impl_item" | "class_definition" => {
            Some(Style::default().fg(Color::Rgb(229, 192, 123)))
        }
        // Comments
        "comment" | "line_comment" | "block_comment" => Some(
            Style::default()
                .fg(Color::Rgb(92, 99, 112))
                .add_modifier(Modifier::ITALIC),
        ),
        // Numbers
        "integer_literal" | "float_literal" | "number" => {
            Some(Style::default().fg(Color::Rgb(209, 154, 102)))
        }
        // Properties/Fields
        "field_identifier" | "property_identifier" | "shorthand_field_identifier" => {
            Some(Style::default().fg(Color::Rgb(86, 182, 194)))
        }

        // ── Git Diff / Patch Kinds ──────────────────────────────────────────
        // Plus lines/additions (typically styled green)
        "added_line" | "addition" => Some(
            Style::default().fg(Color::Rgb(152, 195, 121))
        ),
        // Minus lines/deletions (typically styled red)
        "deleted_line" | "deletion" => Some(
            Style::default().fg(Color::Rgb(224, 108, 117))
        ),
        // Chunk range/hunk headers (e.g. @@ -1,4 +1,5 @@)
        "hunk_header" => Some(
            Style::default().fg(Color::Rgb(198, 120, 221))
        ),
        // Diff commands, indexes, file paths, and general metadata headers
        "command" | "index" | "old_file" | "new_file" | "file_change" | "header" => Some(
            Style::default()
                .fg(Color::Rgb(97, 175, 239))
                .add_modifier(Modifier::BOLD)
        ),

        _ => None,
    }
}

fn style_for_git_log_line(line: &str) -> Option<Style> {
    let trimmed = line.trim_start();

    if trimmed.starts_with("commit ") {
        Some(
            Style::default()
                .fg(Color::Rgb(229, 192, 123))
                .add_modifier(Modifier::BOLD),
        ) // Bold Yellow
    } else if trimmed.starts_with("Author:") {
        Some(Style::default().fg(Color::Rgb(97, 175, 239))) // Blue
    } else if trimmed.starts_with("Date:") {
        Some(
            Style::default()
                .fg(Color::Rgb(120, 130, 140))
                .add_modifier(Modifier::ITALIC),
        ) // Muted Gray/Italic
    } else if trimmed.starts_with("Merge:") {
        Some(Style::default().fg(Color::Rgb(198, 120, 221))) // Magenta
    } else if trimmed.starts_with("diff --git")
        || trimmed.starts_with("---")
        || trimmed.starts_with("+++")
    {
        Some(Style::default().fg(Color::Rgb(86, 182, 194))) // Cyan (for logs containing inline patches)
    } else if trimmed.starts_with('+') && !trimmed.starts_with("+++") {
        Some(Style::default().fg(Color::Rgb(152, 195, 121))) // Green patch addition
    } else if trimmed.starts_with('-') && !trimmed.starts_with("---") {
        Some(Style::default().fg(Color::Rgb(224, 108, 117))) // Red patch deletion
    } else if trimmed.starts_with('~') {
        Some(Style::default().fg(Color::Rgb(198, 120, 100)))
    } else {
        None
    }
}

// Style formatting function added at the bottom of the file:
fn style_for_rg_line(line: &str) -> Vec<Option<Style>> {
    let chars: Vec<char> = line.chars().collect();
    let mut styles = vec![None; chars.len()];

    // Mute comment and config lines
    if line.trim_start().starts_with('#') || line.starts_with("  [RG]") || line.starts_with("  ───")
    {
        let comment_style = Style::default()
            .fg(Color::Rgb(92, 99, 112))
            .add_modifier(Modifier::ITALIC);
        styles.fill(Some(comment_style));
        return styles;
    }

    // Bold Blue for File Headers
    if line.ends_with(':') {
        let path_style = Style::default()
            .fg(Color::Rgb(97, 175, 239))
            .add_modifier(Modifier::BOLD);
        for i in 0..chars.len().saturating_sub(1) {
            styles[i] = Some(path_style);
        }
        if !chars.is_empty() {
            styles[chars.len() - 1] = Some(Style::default().fg(Color::Rgb(92, 99, 112)));
        }
        return styles;
    }

    // Yellow/Orange for Line Numbers preceding ": "
    if let Some(colon_pos) = line.find(": ") {
        let prefix = &line[..colon_pos];
        if prefix.chars().all(|c| c.is_ascii_digit()) {
            let line_num_style = Style::default().fg(Color::Rgb(229, 192, 123));
            for i in 0..colon_pos {
                styles[i] = Some(line_num_style);
            }
            let separator_style = Style::default().fg(Color::Rgb(92, 99, 112));
            styles[colon_pos] = Some(separator_style);
            if colon_pos + 1 < chars.len() {
                styles[colon_pos + 1] = Some(separator_style);
            }
        }
    }

    styles
}

fn style_for_git_status_line(line: &str) -> Vec<Option<Style>> {
    let chars: Vec<char> = line.chars().collect();
    let mut styles = vec![None; chars.len()];
    let trimmed = line.trim();

    // 1. Muted dividers and "(none)" lines
    if trimmed.starts_with('─') || trimmed == "(none)" {
        let mute_style = Style::default().fg(Color::Rgb(92, 99, 112));
        styles.fill(Some(mute_style));
        return styles;
    }

    // 2. Bold/colored Section Headers
    if trimmed.starts_with("Stage Changes") {
        let header_style = Style::default()
            .fg(Color::Rgb(152, 195, 121))
            .add_modifier(Modifier::BOLD); // Green
        styles.fill(Some(header_style));
        return styles;
    }
    if trimmed.starts_with("Unstage Changes") {
        let header_style = Style::default()
            .fg(Color::Rgb(229, 192, 123))
            .add_modifier(Modifier::BOLD); // Yellow
        styles.fill(Some(header_style));
        return styles;
    }
    if trimmed.starts_with("Untracked Files") {
        let header_style = Style::default()
            .fg(Color::Rgb(224, 108, 117))
            .add_modifier(Modifier::BOLD); // Red
        styles.fill(Some(header_style));
        return styles;
    }
    if trimmed.starts_with("------") {
        let sep_style = Style::default()
            .fg(Color::Rgb(97, 175, 239))
            .add_modifier(Modifier::BOLD); // Blue
        styles.fill(Some(sep_style));
        return styles;
    }

    // 3. Staged items (Green)
    // Formatted as "   {path}" (exactly 3 leading spaces, no bracket)
    if line.starts_with("   ") && !line.starts_with("    ") && !trimmed.is_empty() {
        let file_style = Style::default().fg(Color::Rgb(152, 195, 121)); // Green
        for i in 3..chars.len() {
            styles[i] = Some(file_style);
        }
        return styles;
    }

    // 4. Unstaged items (Yellow)
    // Formatted as "  [path]"
    if line.starts_with("  [") && line.ends_with(']') {
        let file_style = Style::default().fg(Color::Rgb(229, 192, 123)); // Yellow
        let bracket_style = Style::default().fg(Color::Rgb(92, 99, 112)); // Muted gray brackets
        if chars.len() > 2 {
            styles[2] = Some(bracket_style);
        }
        for i in 3..chars.len().saturating_sub(1) {
            styles[i] = Some(file_style);
        }
        if chars.len() > 3 {
            styles[chars.len() - 1] = Some(bracket_style);
        }
        return styles;
    }

    // 5. Untracked files (Muted Red)
    // Formatted as "    {path}" (exactly 4 spaces)
    if line.starts_with("    ")
        && !line.starts_with("      ")
        && !trimmed.starts_with('*')
        && !trimmed.starts_with("stash@{")
    {
        let file_style = Style::default().fg(Color::Rgb(224, 108, 117)); // Muted Red
        for i in 4..chars.len() {
            styles[i] = Some(file_style);
        }
        return styles;
    }

    // 6. Active branch vs normal branch
    // Formatted as "    * branch_name relative_date" or "      branch_name relative_date"
    if line.starts_with("    * ") {
        let active_style = Style::default()
            .fg(Color::Rgb(152, 195, 121))
            .add_modifier(Modifier::BOLD); // Green and bold
        let date_style = Style::default()
            .fg(Color::Rgb(120, 130, 140))
            .add_modifier(Modifier::ITALIC);

        if chars.len() > 4 {
            styles[4] = Some(active_style); // '*'
        }

        let words: Vec<&str> = trimmed.split_whitespace().collect();
        if words.len() >= 2 {
            let branch_name = words[1];
            if let Some(pos) = line.find(branch_name) {
                for i in pos..(pos + branch_name.len()).min(chars.len()) {
                    styles[i] = Some(active_style);
                }
                for i in (pos + branch_name.len()).min(chars.len())..chars.len() {
                    styles[i] = Some(date_style);
                }
            }
        }
        return styles;
    } else if line.starts_with("    ") && !trimmed.starts_with("stash@{") {
        // Regular branch list: "    branch_name relative_date"
        let branch_style = Style::default().fg(Color::Rgb(97, 175, 239)); // Blue branch name
        let date_style = Style::default()
            .fg(Color::Rgb(120, 130, 140))
            .add_modifier(Modifier::ITALIC);

        let words: Vec<&str> = trimmed.split_whitespace().collect();
        if !words.is_empty() {
            let branch_name = words[0];
            if let Some(pos) = line.find(branch_name) {
                for i in pos..(pos + branch_name.len()).min(chars.len()) {
                    styles[i] = Some(branch_style);
                }
                for i in (pos + branch_name.len()).min(chars.len())..chars.len() {
                    styles[i] = Some(date_style);
                }
            }
        }
        return styles;
    }

    // 7. Stash entries
    // Formatted as "    stash@{0}: ..."
    if trimmed.starts_with("stash@{") {
        let stash_ref_style = Style::default()
            .fg(Color::Rgb(198, 120, 221))
            .add_modifier(Modifier::BOLD); // Magenta
        let stash_msg_style = Style::default().fg(Color::Rgb(171, 178, 191)); // Default foreground

        if let Some(colon_pos) = line.find(':') {
            for i in 0..=colon_pos {
                styles[i] = Some(stash_ref_style);
            }
            for i in (colon_pos + 1)..chars.len() {
                styles[i] = Some(stash_msg_style);
            }
        }
        return styles;
    }

    // 8. Help/footer hotkeys: "[c] Commit", "[s] Toggle staged"
    let mut in_bracket = false;
    let bracket_style = Style::default().fg(Color::Rgb(92, 99, 112));
    let key_style = Style::default()
        .fg(Color::Rgb(86, 182, 194))
        .add_modifier(Modifier::BOLD); // Cyan
    let text_style = Style::default().fg(Color::Rgb(171, 178, 191));

    for i in 0..chars.len() {
        if chars[i] == '[' {
            in_bracket = true;
            styles[i] = Some(bracket_style);
        } else if chars[i] == ']' {
            in_bracket = false;
            styles[i] = Some(bracket_style);
        } else if in_bracket {
            styles[i] = Some(key_style);
        } else {
            styles[i] = Some(text_style);
        }
    }

    styles
}
