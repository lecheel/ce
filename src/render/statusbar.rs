//! Status bar rendering — Powerline arrow segments.

use ratatui::{
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::Paragraph,
    Frame,
};

use crate::ed::editor::Editor;
use crate::ed::mode::Mode;

const PL_RIGHT: &str = "\u{E0B0}";
const PL_LEFT: &str = "\u{E0B2}";

// ── simple dark palette ────────────────────────────────────────────────────
const BG_FILL: Color = Color::Rgb(36, 36, 44); // Deep slate base
const BG_FILE: Color = Color::Rgb(50, 50, 60); // Slightly lighter slate
const R_POS: Color = Color::Rgb(60, 70, 95); // Dark steel blue
const R_SCOPE: Color = Color::Rgb(55, 55, 68); // Muted purple-gray
const R_LANG: Color = Color::Rgb(45, 65, 75); // Dark teal
const R_BUF: Color = Color::Rgb(65, 55, 80); // Dark indigo
const BG_GIT: Color = Color::Rgb(70, 60, 50); // Dark muted brown

const MAX_SCOPE: usize = 24;

// ── segment type ───────────────────────────────────────────────────────────

struct Seg {
    text: String,
    fg: Color,
    bg: Color,
}

impl Seg {
    fn new(text: impl Into<String>, fg: Color, bg: Color) -> Self {
        Self {
            text: text.into(),
            fg,
            bg,
        }
    }
}

// ── span builders ──────────────────────────────────────────────────────────

/// `[body, PL_RIGHT arrow]` — left-to-right segment.
fn left_span(seg: &Seg, next_bg: Color) -> [Span<'static>; 2] {
    [
        Span::styled(
            seg.text.clone(),
            Style::default()
                .fg(seg.fg)
                .bg(seg.bg)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(PL_RIGHT, Style::default().fg(seg.bg).bg(next_bg)),
    ]
}

/// `[PL_LEFT arrow, body]` — right-side segment.
/// `left_bg` is the background of whatever sits immediately to the left.
fn right_span(seg: &Seg, left_bg: Color) -> [Span<'static>; 2] {
    [
        Span::styled(PL_LEFT, Style::default().fg(seg.bg).bg(left_bg)),
        Span::styled(
            seg.text.clone(),
            Style::default()
                .fg(seg.fg)
                .bg(seg.bg)
                .add_modifier(Modifier::BOLD),
        ),
    ]
}

// ── scope trimming ─────────────────────────────────────────────────────────

/// Trim a scope string to at most `MAX_SCOPE` chars safely.
/// Prefers the last `::` or `.` component so "impl Foo::bar" → "bar".
fn trim_scope(scope: &str) -> String {
    let char_count = scope.chars().count();
    if char_count <= MAX_SCOPE {
        return scope.to_string();
    }
    // Try last Rust/C++ component.
    if let Some(s) = scope.rsplit("::").next() {
        if s.chars().count() <= MAX_SCOPE {
            return s.to_string();
        }
    }
    // Try last dot component (Python / Lua / etc.).
    if let Some(s) = scope.rsplit('.').next() {
        if s.chars().count() <= MAX_SCOPE {
            return s.to_string();
        }
    }
    // Hard truncate safely via character iteration.
    scope.chars().take(MAX_SCOPE).collect()
}

pub fn draw_status_bar(f: &mut Frame, area: Rect, editor: &mut Editor) {
    editor
        .status_state
        .tick(editor.active_row(), editor.active_col());

    let state = &editor.status_state;
    // let lang = detect_display_lang(editor.active_filename());
    let lang = editor.buf().language_display_name();
    let scope = editor.current_scope().unwrap_or_default();

    // ── mode (leftmost) ────────────────────────────────────────────────────
    // Muted, dark-mode-friendly colors with soft off-white text
    let (mode_text, mode_bg, mode_fg) = match editor.mode() {
        Mode::Normal => (
            " NORMAL ",
            Color::Rgb(40, 70, 85),
            Color::Rgb(220, 235, 245),
        ),
        Mode::Insert => (
            " INSERT ",
            Color::Rgb(45, 75, 55),
            Color::Rgb(220, 245, 230),
        ),
        Mode::Command => (
            " COMMAND ",
            Color::Rgb(85, 80, 45),
            Color::Rgb(245, 240, 220),
        ),
        Mode::Brief => (" BRIEF ", Color::Rgb(75, 50, 85), Color::Rgb(240, 220, 245)),
        Mode::Visual => (
            " VISUAL ",
            Color::Rgb(100, 75, 45), // Soft visual amber
            Color::Rgb(245, 235, 220),
        ),
        Mode::VisualLine => (
            " V-LINE ",
            Color::Rgb(100, 65, 45), // Soft v-line dark orange
            Color::Rgb(245, 230, 220),
        ),
        Mode::VisualBlock => (
            " V-BLOCK ",
            Color::Rgb(100, 65, 45), // Soft v-line dark orange
            Color::Rgb(245, 230, 220),
        ),
        Mode::Search => (
            " SEARCH ",
            Color::Rgb(100, 75, 45),
            Color::Rgb(245, 235, 220),
        ),
        Mode::LlmPrompt => (
            " LLM PROMPT ",
            Color::Rgb(85, 45, 85),
            Color::Rgb(240, 220, 245),
        ),
    };
    let mode_seg = Seg::new(mode_text, mode_fg, mode_bg);

    // ── filename ───────────────────────────────────────────────────────────
    let filename_raw = editor.active_filename();
    let filename = if let Some(raw) = filename_raw {
        // Resolve relative paths to absolute first to show the full contracted path
        let abs_path = if std::path::Path::new(raw).is_absolute() {
            raw.to_string()
        } else if let Ok(cwd) = std::env::current_dir() {
            cwd.join(raw).to_string_lossy().to_string()
        } else {
            raw.to_string()
        };
        crate::render::helpers::contract_path_fish(&abs_path)
    } else {
        "[No Name]".to_string()
    };

    let file_text = if editor.active_modified() {
        format!(" {} [+] ", filename)
    } else {
        format!(" {} ", filename)
    };
    let file_seg = Seg::new(&file_text, Color::Rgb(210, 215, 225), BG_FILE);

    // ── Git Branch & Diff Stats segment ────────────────────────────────────
    let (added, modified, removed) = editor.git_diff_stats();

    let git_seg: Option<Seg> = if let Some(branch) = editor.current_git_branch() {
        let mut git_text = format!(" {} ", branch);

        if added > 0 || modified > 0 || removed > 0 {
            let mut parts = Vec::new();
            if added > 0 {
                parts.push(format!("+{}", added));
            }
            if modified > 0 {
                parts.push(format!("~{}", modified));
            }
            if removed > 0 {
                parts.push(format!("-{}", removed));
            }
            git_text = format!(" {} {} ", branch, parts.join(" "));
        }

        Some(Seg::new(git_text, Color::Rgb(200, 215, 225), BG_GIT))
    } else {
        None
    };

    // ── LSP / Codeium indicator ────────────────────────────────────────────
    // Muted, cool-toned slate colors instead of harsh red/brown
    let lsp_bg = if !editor.config.codeium_enabled {
        Color::Rgb(65, 50, 60) // Muted dark mauve/rose for OFF
    } else if editor.lsp_loading() {
        Color::Rgb(60, 60, 45) // Muted dark olive/amber for LOADING
    } else {
        Color::Rgb(30, 60, 50) // Muted dark teal for OK
    };
    let lsp_text = if !editor.config.codeium_enabled {
        " Codeium OFF ".to_string()
    } else if editor.lsp_loading() {
        let frames = [
            '\u{2807}', '\u{2819}', '\u{2839}', '\u{2838}', '\u{283C}', '\u{2834}', '\u{2826}',
            '\u{2827}', '\u{2807}', '\u{280F}',
        ];
        format!(
            " {} Codeium… ",
            frames[editor.spinner_frame() % frames.len()]
        )
    } else {
        " Codeium OK ".to_string()
    };

    // ── right side segments (rightmost first, then reversed) ───────────────
    let mut right_segs: Vec<Seg> = Vec::new();

    // Rightmost: line:col (fixed width 10 chars)
    let row = editor.active_row().saturating_add(1);
    let col = editor.active_col().saturating_add(1);
    right_segs.push(Seg::new(
        format!(" {:>4}:{:<3}", row, col), // exactly 10 chars
        Color::Rgb(200, 210, 230),
        R_POS,
    ));

    // LSP status
    right_segs.push(Seg::new(lsp_text, Color::Rgb(210, 215, 225), lsp_bg));

    // Language
    right_segs.push(Seg::new(
        format!(" {} ", lang),
        Color::Rgb(200, 220, 225),
        R_LANG,
    ));

    // Scope (function / method) – if not empty
    if !scope.is_empty() {
        let trimmed = trim_scope(&scope);
        right_segs.push(Seg::new(
            format!(" {} ", trimmed),
            Color::Rgb(180, 180, 195),
            R_SCOPE,
        ));
    }

    // Buffer counter (optional)
    if editor.buffer_count() > 1 {
        right_segs.push(Seg::new(
            format!(" {}/{} ", editor.active_idx() + 1, editor.buffer_count()),
            Color::Rgb(210, 205, 230),
            R_BUF,
        ));
    }

    // Reverse to obtain visual left‑to‑right order
    right_segs.reverse();

    // ── build spans ────────────────────────────────────────────────────────
    let mut spans: Vec<Span> = Vec::new();

    // Left side: mode → filename → (optional git) → filler
    let [ms, ma] = left_span(&mode_seg, BG_FILE);
    spans.push(ms);
    spans.push(ma);

    // FIX: Properly transition the filename arrow into the git segment if it exists
    if let Some(ref git) = git_seg {
        let [fs, fa] = left_span(&file_seg, git.bg);
        spans.push(fs);
        spans.push(fa);

        let [gs, ga] = left_span(&git, BG_FILL);
        spans.push(gs);
        spans.push(ga);
    } else {
        let [fs, fa] = left_span(&file_seg, BG_FILL);
        spans.push(fs);
        spans.push(fa);
    }

    // Compute filler width safely
    let left_used = mode_text
        .len()
        .saturating_add(1)
        .saturating_add(file_text.len())
        .saturating_add(1)
        .saturating_add(
            git_seg
                .as_ref()
                .map(|g| g.text.len().saturating_add(1))
                .unwrap_or(0),
        );

    let right_used: usize = right_segs
        .iter()
        .map(|s| s.text.len().saturating_add(1))
        .sum();
    let pad = (area.width as usize).saturating_sub(left_used.saturating_add(right_used));
    spans.push(Span::styled(" ".repeat(pad), Style::default().bg(BG_FILL)));

    // Right side spans (each segment provides its own left‑pointing arrow)
    let right_spans: Vec<Span> = right_segs
        .iter()
        .enumerate()
        .flat_map(|(i, seg)| {
            let left_bg = if i == 0 {
                BG_FILL
            } else {
                right_segs[i - 1].bg
            };
            right_span(seg, left_bg).into_iter()
        })
        .collect();
    spans.extend(right_spans);

    f.render_widget(Paragraph::new(Line::from(spans)), area);
}
