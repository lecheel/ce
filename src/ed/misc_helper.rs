// ---------------------------------------------------------------------------
// Standalone helpers
// ---------------------------------------------------------------------------

/// Group a sorted list of 0-based row numbers into contiguous runs.
///
/// `[1,2,3, 12,13, 24]` → `[[1,2,3], [12,13], [24]]`
pub fn group_signed_rows(signed_rows: &[usize]) -> Vec<Vec<usize>> {
    if signed_rows.is_empty() {
        return Vec::new();
    }
    let mut hunks = vec![vec![signed_rows[0]]];
    let mut prev = signed_rows[0];
    for &row in &signed_rows[1..] {
        if row == prev + 1 {
            hunks.last_mut().unwrap().push(row);
        } else {
            hunks.push(vec![row]);
        }
        prev = row;
    }
    hunks
}

/// Extract a single hunk from a unified diff that covers `target_row_0based`.
///
/// Returns the patch header lines (`diff`, `index`, `---`, `+++`) plus the
/// matching `@@ … @@` hunk and its body, suitable for `git apply -R`.
pub fn extract_hunk_patch(diff_text: &str, target_row_0based: usize) -> Option<String> {
    let target_1based = target_row_0based + 1;
    let mut result = String::new();
    let mut header_done = false;
    let mut in_target_hunk = false;

    for line in diff_text.lines() {
        if !header_done {
            if line.starts_with("diff ")
                || line.starts_with("index ")
                || line.starts_with("--- ")
                || line.starts_with("+++ ")
            {
                result.push_str(line);
                result.push('\n');
                continue;
            }
            if line.starts_with("@@") {
                header_done = true;
            } else {
                continue;
            }
        }

        if line.starts_with("@@") {
            if in_target_hunk {
                break; // collected our hunk — stop
            }

            // Parse: @@ -old_start[,old_count] +new_start[,new_count] @@
            let plus_pos = line.find('+')?;
            let after_plus = &line[plus_pos + 1..];
            let space_pos = after_plus.find(' ')?;
            let new_info = &after_plus[..space_pos];

            let (new_start, new_count): (usize, usize) = if let Some(comma) = new_info.find(',') {
                (
                    new_info[..comma].parse().ok()?,
                    new_info[comma + 1..].parse().ok()?,
                )
            } else {
                (new_info.parse().ok()?, 1)
            };

            // Does this hunk cover the target row?
            let covers = if new_count > 0 {
                target_1based >= new_start && target_1based < new_start + new_count
            } else {
                // Pure-deletion hunk (new_count == 0): new_start is the
                // anchor line.  Match if cursor is within ±1 of it.
                target_1based >= new_start.saturating_sub(1) && target_1based <= new_start + 1
            };

            if covers {
                in_target_hunk = true;
            }

            if in_target_hunk {
                result.push_str(line);
                result.push('\n');
            }
            continue;
        }

        if in_target_hunk {
            result.push_str(line);
            result.push('\n');
        }
    }

    if in_target_hunk {
        Some(result)
    } else {
        None
    }
}

/// Read the content of a file from the HEAD tree using git2.
///
/// Reads from the git object database — no shell spawn, no working-tree
/// access, no disk writes.
pub fn get_head_file_content(repo: &git2::Repository, rel_path: &str) -> Option<String> {
    let head = repo.head().ok()?;
    let tree = head.peel_to_tree().ok()?;
    let entry = tree.get_path(std::path::Path::new(rel_path)).ok()?;
    let blob = entry.to_object(repo).ok()?;
    let content = blob.as_blob()?.content();
    Some(String::from_utf8_lossy(content).to_string())
}

/// Standalone helper: number of decimal digits in `n`.
pub fn digit_count(n: usize) -> usize {
    if n == 0 {
        return 1;
    }
    let mut count = 0;
    let mut val = n;
    while val > 0 {
        val /= 10;
        count += 1;
    }
    count
}

/// Returns the line-comment prefix for a given language ID.
pub fn comment_prefix_for_lang(lang: &str) -> &'static str {
    match lang {
        "rust" | "javascript" | "typescript" | "go" | "java" | "c" | "cpp" | "cs" | "swift"
        | "kotlin" => "//",
        "python" | "sh" | "bash" | "zsh" | "ruby" | "yaml" | "toml" | "perl" | "r" => "#",
        "lua" | "sql" | "haskell" | "ada" | "elm" => "--",
        "vim" => "\"",
        "clojure" | "lisp" | "scheme" => ";;",
        _ => "//", // Default fallback
    }
}
/// Recurse into `node`'s *direct* children, counting function-like nodes.
/// Does NOT recurse into those children's bodies (so a doubly-nested fn
/// inside a nested fn is NOT counted as a separate top-level nested fn
/// — it is part of the first nested fn).
pub fn count_nested_fns(node: tree_sitter::Node) -> usize {
    let mut count: usize = 0;
    let mut stack: Vec<tree_sitter::Node> = Vec::new();

    // ── FIX: Explicit `usize` annotation prevents E0282 type inference error ──
    let child_count: usize = node.child_count();
    for i in 0..child_count {
        if let Some(child) = node.child(i) {
            stack.push(child);
        }
    }

    while let Some(n) = stack.pop() {
        if is_fn_kind(n.kind()) {
            count += 1;
            // Do NOT recurse into nested functions — their own inner fns
            // belong to them, not to the outer function we're measuring.
        } else {
            let cc: usize = n.child_count();
            for i in 0..cc {
                if let Some(child) = n.child(i) {
                    stack.push(child);
                }
            }
        }
    }

    count
}

pub fn is_fn_kind(kind: &str) -> bool {
    matches!(
        kind,
        "function_item"
            | "function_definition"
            | "method_definition"
            | "arrow_function"
            | "function_declaration"
            | "method_declaration"
    )
}
