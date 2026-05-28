// File: src/popup/function_list.rs
//! Function list popup overlay for quick navigation of functions/methods in a buffer.

use crate::ed::buffer::Buffer;
use crate::popup::filtered_list::{EntryFilter, FilteredList};
use crate::popup::Scrollable;
use ropey::Rope;

impl EntryFilter for FunctionEntry {
    fn match_query(&self, query: &str) -> Option<Vec<usize>> {
        let q = query.to_lowercase();
        if self.name.to_lowercase().contains(&q)
            || self.kind.to_lowercase().contains(&q)
            || self.signature.to_lowercase().contains(&q)
        {
            Some(Vec::new()) // matched, no highlight indices
        } else {
            None
        }
    }
}
/// A single function/method entry found in the buffer.
#[derive(Debug, Clone)]
pub struct FunctionEntry {
    /// Function/method name (shown first, max 20 chars)
    pub name: String,
    /// Short keyword prefix: "pub fn", "fn", "async fn", "def", etc. (shown after name)
    pub kind: String,
    /// Brief signature snippet (args + return) for the popup detail column.
    pub signature: String,
    /// 0-indexed line where the function begins.
    pub line: usize,
    /// Whether another entry in the buffer shares the exact same name.
    pub is_duplicate: bool,
}

impl FunctionEntry {
    /// Display kind with fixed 20 characters, **right‑aligned**.
    pub fn display_kind(&self) -> String {
        if self.kind.len() > 12 {
            format!("{}…", &self.kind[..10])
        } else {
            format!("{:>12} ", self.kind) // ← right alignment
        }
    }

    /// Display name with fixed 50 characters, left‑aligned (unchanged).
    pub fn display_name(&self) -> String {
        if self.name.len() > 50 {
            format!("{}…", &self.name[..47])
        } else {
            format!("{:<50}", self.name)
        }
    }
}

#[derive(Debug, Clone)]
pub struct FunctionListPopup {
    pub list: FilteredList<FunctionEntry>,
}

impl FunctionListPopup {
    pub fn new(mut entries: Vec<FunctionEntry>) -> Self {
        use std::collections::HashMap;
        let mut counts = HashMap::new();
        for entry in &entries {
            *counts.entry(entry.name.clone()).or_insert(0) += 1;
        }
        for entry in &mut entries {
            if let Some(&count) = counts.get(&entry.name) {
                entry.is_duplicate = count > 1;
            }
        }
        Self {
            list: FilteredList::new(entries),
        }
    }

    pub fn selected_entry(&self) -> Option<&FunctionEntry> {
        self.list.selected_entry()
    }

    pub fn filter_push(&mut self, c: char) {
        self.list.filter_push(c);
    }
    pub fn filter_pop(&mut self) {
        self.list.filter_pop();
    }
    pub fn filter_clear(&mut self) {
        self.list.filter_clear();
    }
    pub fn filter_is_empty(&self) -> bool {
        self.list.filter_is_empty()
    }
    pub fn move_up(&mut self) {
        self.list.move_up();
    }
    pub fn move_down(&mut self) {
        self.list.move_down();
    }
}

impl Scrollable for FunctionListPopup {
    fn selected(&self) -> usize {
        self.list.selected()
    }
    fn selected_mut(&mut self) -> &mut usize {
        self.list.selected_mut()
    }
    fn scroll_mut(&mut self) -> &mut usize {
        self.list.scroll_mut()
    }
    fn len(&self) -> usize {
        self.list.len()
    }
    fn visible_rows(&self) -> usize {
        self.list.visible_rows()
    }
}

/// Calculate optimal width based on content (but respect max_width)
pub fn calculate_popup_width(popup: &FunctionListPopup, max_width: usize) -> usize {
    if popup.list.filtered.is_empty() {
        return 80;
    }

    let mut max_kind_len = 0;
    let mut max_name_len = 0;
    for &idx in popup.list.filtered.iter() {
        if let Some(entry) = popup.list.entries.get(idx) {
            max_kind_len = max_kind_len.max(entry.kind.len().min(20));
            max_name_len = max_name_len.max(entry.name.len().min(50));
        }
    }
    let width = 20 + 2 + 50 + 2 + 10 + 2 + 30;
    width.min(max_width)
}

/// Traverses the Tree-sitter AST of the active buffer and extracts all function/method declarations.
pub fn extract_functions(buf: &Buffer) -> Vec<FunctionEntry> {
    let mut entries = Vec::new();
    let tree = match &buf.syntax.tree {
        Some(t) => t,
        None => return entries,
    };
    let root = tree.root_node();
    let rope = &buf.rope;

    fn traverse(node: tree_sitter::Node, rope: &Rope, entries: &mut Vec<FunctionEntry>) {
        let kind_str = node.kind();

        let is_fn = kind_str == "function_item"
            || kind_str == "function_definition"
            || kind_str == "function_declaration"
            || kind_str == "method_definition";

        if is_fn {
            let line = node.start_position().row;

            // Extract kind prefix with "pub" handling
            let kind = if kind_str == "function_item" {
                let mut mods = Vec::new();
                let mut cursor = node.walk();
                let mut has_pub = false;

                for child in node.children(&mut cursor) {
                    let child_kind = child.kind();
                    if child_kind == "visibility_modifier" {
                        let text = extract_node_text(rope, &child);
                        if text == "pub" {
                            has_pub = true;
                            mods.push("pub".to_string());
                        }
                    } else if child_kind == "async" {
                        mods.push("async".to_string());
                    } else if child_kind == "unsafe" {
                        mods.push("unsafe".to_string());
                    } else if child_kind == "fn" {
                        if !has_pub {
                            mods.push("fn".to_string());
                        } else {
                            mods.push("fn".to_string());
                        }
                        break;
                    }
                }

                if mods.is_empty() {
                    "fn".to_string()
                } else if has_pub && mods.len() == 1 {
                    "pub fn".to_string()
                } else {
                    mods.join(" ")
                }
            } else if kind_str == "method_definition" {
                "fn".to_string()
            } else if kind_str == "function_definition" {
                "def".to_string() // Python
            } else {
                "function".to_string() // JS/TS
            };

            // Extract function/method name
            let mut name = "anonymous".to_string();
            if let Some(name_node) = node.child_by_field_name("name") {
                name = extract_node_text(rope, &name_node);
            } else {
                let mut cursor = node.walk();
                for child in node.children(&mut cursor) {
                    if child.kind() == "identifier" || child.kind() == "field_identifier" {
                        name = extract_node_text(rope, &child);
                        break;
                    }
                }
            }

            // Extract signature snippet
            let mut signature = String::new();
            if let Some(params_node) = node.child_by_field_name("parameters") {
                signature = extract_node_text(rope, &params_node);
            } else {
                let mut cursor = node.walk();
                for child in node.children(&mut cursor) {
                    let k = child.kind();
                    if k == "parameters" || k == "formal_parameters" || k == "parameter_list" {
                        signature = extract_node_text(rope, &child);
                        break;
                    }
                }
            }

            // Store with name first, kind second
            entries.push(FunctionEntry {
                name,
                kind,
                signature,
                line,
                is_duplicate: false, // will be computed in FunctionListPopup::new
            });
        }

        // Recurse down
        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            traverse(child, rope, entries);
        }
    }

    fn extract_node_text(rope: &Rope, node: &tree_sitter::Node) -> String {
        let start = rope.byte_to_char(node.start_byte());
        let end = rope.byte_to_char(node.end_byte());
        rope.slice(start..end).to_string()
    }

    traverse(root, rope, &mut entries);
    entries
}
