//! Shared fuzzy-match logic for all popup lists.

/// Subsequence fuzzy match: returns the char indices in `text` that match
/// each character of `query` (case-insensitive, in order).
/// Returns `None` if not every query character was found.
pub fn fuzzy_match(text: &str, query: &str) -> Option<Vec<usize>> {
    if query.is_empty() {
        return Some(Vec::new());
    }

    let text_lower: Vec<char> = text.to_lowercase().chars().collect();
    let query_chars: Vec<char> = query.to_lowercase().chars().collect();

    let mut indices = Vec::with_capacity(query_chars.len());
    let mut qi = 0;

    for (ti, tc) in text_lower.iter().enumerate() {
        if qi < query_chars.len() && *tc == query_chars[qi] {
            indices.push(ti);
            qi += 1;
        }
    }

    if qi == query_chars.len() {
        Some(indices)
    } else {
        None
    }
}

/// Case-insensitive substring search. Returns the char-index range
/// `(start, end)` of the first occurrence, or `None`.
pub fn substring_find(text: &str, needle: &str) -> Option<(usize, usize)> {
    if needle.is_empty() {
        return None;
    }
    let text_lower = text.to_lowercase();
    let needle_lower = needle.to_lowercase();

    let byte_start = text_lower.find(&needle_lower)?;
    let char_start = text[..byte_start].chars().count();
    let char_len = needle.chars().count();
    Some((char_start, char_start + char_len))
}
