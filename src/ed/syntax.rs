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
            Some("checkhealth") => {
                return crate::ed::health::style_for_checkhealth_line(line_text);
            }
            None => return char_styles,
            _ => {}
        }

        if let Some(tree) = &self.tree {
            let root = tree.root_node();
            // Pass line_text into collect_highlights
            Self::collect_highlights(root, row, line_text, &mut char_styles);
        }
        char_styles
    }

    fn collect_highlights(
        root: Node,
        row: usize,
        line_text: &str,
        char_styles: &mut Vec<Option<Style>>,
    ) {
        let mut cursor = root.walk();
        let mut done = false;

        while !done {
            let node = cursor.node();

            if node.start_position().row <= row && node.end_position().row >= row {
                // ── Node overlaps the target row ──────────────────────
                if let Some(style) = style_for_kind(node.kind()) {
                    let start_col = if node.start_position().row < row {
                        0
                    } else {
                        let byte = node.start_position().column.min(line_text.len());
                        line_text[..byte].chars().count()
                    };
                    let end_col = if node.end_position().row > row {
                        char_styles.len()
                    } else {
                        let byte = node.end_position().column.min(line_text.len());
                        line_text[..byte].chars().count().min(char_styles.len())
                    };
                    for i in start_col..end_col {
                        char_styles[i] = Some(style);
                    }
                }

                // ONLY descend into children when the parent overlaps
                // the target row — children are always within the parent
                // range, so non-overlapping parents have no relevant children.
                if cursor.goto_first_child() {
                    continue;
                }
            }
            // ── Node does NOT overlap, or has no children ────────────
            // Skip to next sibling (prunes the entire subtree for
            // non-overlapping nodes — this is the missing optimisation).
            while !cursor.goto_next_sibling() {
                if !cursor.goto_parent() {
                    done = true;
                    break;
                }
            }
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
    /// Find the matching bracket for the character at (row, col).
    /// Uses tree-sitter for 100% accurate matching (automatically ignores strings/comments).
    /// Returns (row, col) of the matching bracket.
    pub fn find_matching_bracket(&self, row: usize, col: usize) -> Option<(usize, usize)> {
        let tree = self.tree.as_ref()?;
        let root = tree.root_node();
        let point = Point::new(row, col);

        // Find the node exactly at the cursor
        let node = root.descendant_for_point_range(point, point)?;
        let kind = node.kind();

        // Only trigger on bracket characters
        if !matches!(kind, "{" | "}" | "(" | ")" | "[" | "]") {
            return None;
        }

        // The bracket's parent is the structural node (e.g., 'block', 'parameters')
        let parent = node.parent()?;

        // Opening brackets are always the first child, closing brackets are the last.
        let matching_node = if matches!(kind, "{" | "(" | "[") {
            parent.child(parent.child_count() - 1)?
        } else {
            parent.child(0)?
        };

        // Sanity check: ensure the matching node is actually the opposite bracket
        // (If the code has syntax errors, the tree might be malformed)
        let expected = match kind {
            "{" => "}",
            "}" => "{",
            "(" => ")",
            ")" => "(",
            "[" => "]",
            "]" => "[",
            _ => unreachable!(),
        };

        if matching_node.kind() == expected {
            Some((
                matching_node.start_position().row,
                matching_node.start_position().column,
            ))
        } else {
            None
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

#[rustfmt::skip]
fn style_for_kind(kind: &str) -> Option<Style> {
    match kind {
        // ── Keywords (Mauve #cb6af7, bold) ──────────────────────────────
        "fn" | "let" | "mut" | "if" | "else" | "return"
        | "struct" | "enum" | "impl" | "pub" | "use" | "mod"
        | "match" | "loop" | "while" | "for" | "in"
        | "break" | "continue" | "async" | "await" | "dyn"
        | "trait" | "where" | "ref" | "as" | "type" | "const"
        | "static" | "unsafe" | "extern" | "crate" | "super"
        | "move" | "true" | "false"
        // Python
        | "def" | "class" | "import" | "from" | "try" | "except"
        | "finally" | "with" | "yield" | "lambda" | "pass"
        | "raise" | "global" | "nonlocal" | "assert" | "del"
        | "not" | "and" | "or" | "is"
        // JS/TS
        | "var" | "extends" | "new" | "typeof" | "instanceof"
        | "interface" | "implements" | "readonly" | "declare"
        | "default" => Some(
            Style::default()
                .fg(Color::Rgb(203, 166, 247))
                .add_modifier(Modifier::BOLD),
        ),

        // ── self / Self / this (Pink #f38ba8) ───────────────────────────
        "self" | "Self" | "this" => Some(
            Style::default().fg(Color::Rgb(243, 139, 168))
        ),

        // ── Lifetimes (Light Magenta #f5c2e7) ───────────────────────────
        "lifetime" => Some(
            Style::default().fg(Color::Rgb(245, 194, 231))
        ),

        // ── Macros (Yellow #f9e2af) ──────────────────────────────────────
        "macro_invocation" | "macro_definition" => Some(
            Style::default().fg(Color::Rgb(249, 226, 175))
        ),

        // ── Strings (Green #a6e3a1) ──────────────────────────────────────
        "string"
        | "string_content"
        | "raw_string_literal"
        | "string_literal"
        | "interpreted_string_literal"
        | "char_literal" => Some(
            Style::default().fg(Color::Rgb(166, 227, 161))
        ),

        // ── Escape sequences (Light Pink #eba0ac) ───────────────────────
        "escape_sequence" => Some(
            Style::default().fg(Color::Rgb(235, 160, 172))
        ),

        // ── Numbers (Orange #bf5c26) ─────────────────────────────────────
        "integer_literal" | "float_literal" | "number"
        | "integer" | "float" => Some(
            Style::default().fg(Color::Rgb(191, 92, 38))
        ),

        // ── Booleans (Peach #fab387) ─────────────────────────────────────
        "boolean_literal" => Some(
            Style::default().fg(Color::Rgb(250, 179, 135))
        ),

        // ── Type identifiers (Sapphire #349beb) ─────────────────────────
        "type_identifier"
        | "struct_item"
        | "enum_item"
        | "impl_item"
        | "trait_item"
        | "class_definition"
        | "type_alias" => Some(
            Style::default().fg(Color::Rgb(52, 155, 235))
        ),

        // ── Function / method definitions (Cyan #82d7fa) ─────────────────
        "function_item"
        | "function_definition"
        | "method_definition"
        | "function_signature_item" => Some(
            Style::default().fg(Color::Rgb(130, 215, 250))
        ),

        // ── Function / method calls (Light Cyan #74c7ec) ─────────────────
        "call_expression"
        | "method_call_expression" => Some(
            Style::default().fg(Color::Rgb(116, 199, 236))
        ),

        // ── Constants (Peach #fab387) ────────────────────────────────────
        "const_item"
        | "static_item"
        | "enum_variant" => Some(
            Style::default().fg(Color::Rgb(250, 179, 135))
        ),

        // ── Properties / fields (Off-white #cdd6f4) ──────────────────────
        "field_identifier"
        | "property_identifier"
        | "shorthand_field_identifier"
        | "field_declaration" => Some(
            Style::default().fg(Color::Rgb(205, 214, 244))
        ),

        // ── Operators (Blue #89b4fa) ──────────────────────────────────────
        "operator"
        | "unary_operator"
        | "binary_operator"
        | "assignment_operator" => Some(
            Style::default().fg(Color::Rgb(137, 180, 250))
        ),

        // ── Delimiters / punctuation (Grayish Blue #9399b2) ──────────────
        "{" | "}" | "(" | ")" | "[" | "]"
        | "," | "." | ";" | ":" | "::"
        | "->" | "=>" | "|" => Some(
            Style::default().fg(Color::Rgb(147, 153, 178))
        ),

        // ── Attributes (Muted Gray #6c7086) ──────────────────────────────
        "attribute_item"
        | "inner_attribute_item"
        | "attribute" => Some(
            Style::default().fg(Color::Rgb(108, 112, 134))
        ),

        // ── Labels (Warm Yellow #fadc96) ─────────────────────────────────
        "label" => Some(
            Style::default().fg(Color::Rgb(250, 220, 150))
        ),

        // ── Comments (Overlay0 #5e6978, italic) ──────────────────────────
        "comment"
        | "line_comment"
        | "block_comment" => Some(
            Style::default()
                .fg(Color::Rgb(94, 105, 120))
                .add_modifier(Modifier::ITALIC),
        ),

        // ── Doc comments (Lighter slate #73718d, italic) ─────────────────
        "doc_comment" => Some(
            Style::default()
                .fg(Color::Rgb(115, 125, 145))
                .add_modifier(Modifier::ITALIC),
        ),

        // ── Git diff kinds (unchanged from before) ───────────────────────
        "added_line" | "addition" => Some(
            Style::default().fg(Color::Rgb(166, 227, 161))
        ),
        "deleted_line" | "deletion" => Some(
            Style::default().fg(Color::Rgb(243, 139, 168))
        ),
        "hunk_header" => Some(
            Style::default().fg(Color::Rgb(203, 166, 247))
        ),
        "command" | "index" | "old_file" | "new_file"
        | "file_change" | "header" => Some(
            Style::default()
                .fg(Color::Rgb(137, 180, 250))
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
                .fg(Color::Rgb(203, 166, 247)) // Mauve
                .add_modifier(Modifier::BOLD),
        )
    } else if trimmed.starts_with("Author:") {
        Some(Style::default().fg(Color::Rgb(137, 180, 250))) // Blue
    } else if trimmed.starts_with("Date:") {
        Some(
            Style::default()
                .fg(Color::Rgb(94, 105, 120)) // Overlay0
                .add_modifier(Modifier::ITALIC),
        )
    } else if trimmed.starts_with("Merge:") {
        Some(Style::default().fg(Color::Rgb(243, 139, 168))) // Pink/Red
    } else if trimmed.starts_with("diff --git")
        || trimmed.starts_with("---")
        || trimmed.starts_with("+++")
    {
        Some(Style::default().fg(Color::Rgb(52, 155, 235))) // Sapphire
    } else if trimmed.starts_with('+') && !trimmed.starts_with("+++") {
        Some(Style::default().fg(Color::Rgb(166, 227, 161))) // Green
    } else if trimmed.starts_with('-') && !trimmed.starts_with("---") {
        Some(Style::default().fg(Color::Rgb(243, 139, 168))) // Red
    } else if trimmed.starts_with('~') {
        Some(Style::default().fg(Color::Rgb(128, 135, 162))) // Overlay1
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
            .fg(Color::Rgb(94, 105, 120)) // Overlay0
            .add_modifier(Modifier::ITALIC);
        styles.fill(Some(comment_style));
        return styles;
    }

    // Bold Blue for File Headers
    if line.ends_with(':') {
        let path_style = Style::default()
            .fg(Color::Rgb(137, 180, 250)) // Blue
            .add_modifier(Modifier::BOLD);
        for i in 0..chars.len().saturating_sub(1) {
            styles[i] = Some(path_style);
        }
        if !chars.is_empty() {
            styles[chars.len() - 1] = Some(Style::default().fg(Color::Rgb(94, 105, 120)));
            // Overlay0
        }
        return styles;
    }

    // Yellow/Orange for Line Numbers preceding ": "
    if let Some(colon_pos) = line.find(": ") {
        let prefix = &line[..colon_pos];
        if prefix.chars().all(|c| c.is_ascii_digit()) {
            let line_num_style = Style::default().fg(Color::Rgb(249, 226, 175)); // Yellow
            for i in 0..colon_pos {
                styles[i] = Some(line_num_style);
            }
            let separator_style = Style::default().fg(Color::Rgb(94, 105, 120)); // Overlay0
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
        let mute_style = Style::default().fg(Color::Rgb(94, 105, 120)); // Overlay0
        styles.fill(Some(mute_style));
        return styles;
    }

    // 2. Bold/colored Section Headers
    if trimmed.starts_with("Stage Changes") {
        let header_style = Style::default()
            .fg(Color::Rgb(166, 227, 161)) // Green
            .add_modifier(Modifier::BOLD);
        styles.fill(Some(header_style));
        return styles;
    }
    if trimmed.starts_with("Unstage Changes") {
        let header_style = Style::default()
            .fg(Color::Rgb(249, 226, 175)) // Yellow
            .add_modifier(Modifier::BOLD);
        styles.fill(Some(header_style));
        return styles;
    }
    if trimmed.starts_with("Untracked Files") {
        let header_style = Style::default()
            .fg(Color::Rgb(243, 139, 168)) // Red
            .add_modifier(Modifier::BOLD);
        styles.fill(Some(header_style));
        return styles;
    }
    if trimmed.starts_with("------") {
        let sep_style = Style::default()
            .fg(Color::Rgb(137, 180, 250)) // Blue
            .add_modifier(Modifier::BOLD);
        styles.fill(Some(sep_style));
        return styles;
    }

    // 3. Staged items (Green)
    if line.starts_with("   ") && !line.starts_with("    ") && !trimmed.is_empty() {
        let file_style = Style::default().fg(Color::Rgb(166, 227, 161)); // Green
        for i in 3..chars.len() {
            styles[i] = Some(file_style);
        }
        return styles;
    }

    // 4. Unstaged items (Yellow)
    if line.starts_with("  [") && line.ends_with(']') {
        let file_style = Style::default().fg(Color::Rgb(249, 226, 175)); // Yellow
        let bracket_style = Style::default().fg(Color::Rgb(128, 135, 162)); // Overlay1
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

    // 5. Untracked files (Red)
    if line.starts_with("    ")
        && !line.starts_with("      ")
        && !trimmed.starts_with('*')
        && !trimmed.starts_with("stash@{")
    {
        let file_style = Style::default().fg(Color::Rgb(243, 139, 168)); // Red
        for i in 4..chars.len() {
            styles[i] = Some(file_style);
        }
        return styles;
    }

    // 6. Active branch vs normal branch
    if line.starts_with("    * ") {
        let active_style = Style::default()
            .fg(Color::Rgb(166, 227, 161)) // Green
            .add_modifier(Modifier::BOLD);
        let date_style = Style::default()
            .fg(Color::Rgb(94, 105, 120)) // Overlay0
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
        // Regular branch list
        let branch_style = Style::default().fg(Color::Rgb(137, 180, 250)); // Blue
        let date_style = Style::default()
            .fg(Color::Rgb(94, 105, 120)) // Overlay0
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
    if trimmed.starts_with("stash@{") {
        let stash_ref_style = Style::default()
            .fg(Color::Rgb(203, 166, 247)) // Mauve
            .add_modifier(Modifier::BOLD);
        let stash_msg_style = Style::default().fg(Color::Rgb(205, 214, 244)); // Text

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

    // 8. Help/footer hotkeys
    let mut in_bracket = false;
    let bracket_style = Style::default().fg(Color::Rgb(128, 135, 162)); // Overlay1
    let key_style = Style::default()
        .fg(Color::Rgb(137, 180, 250)) // Blue
        .add_modifier(Modifier::BOLD);
    let text_style = Style::default().fg(Color::Rgb(205, 214, 244)); // Text

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
