//! Registers popup overlay (informational only — no navigation).

#[derive(Debug, Clone)]
pub struct RegisterEntry {
    /// The register name character: `"`, `+`, `-`, `%`, `/`, `0`, `a`-`z`
    pub name: char,
    /// Short label describing the register type.
    pub label: String,
    /// Preview of the register contents (truncated).
    pub preview: String,
}

#[derive(Debug, Clone)]
pub struct RegistersPopup {
    pub entries: Vec<RegisterEntry>,
}

impl RegistersPopup {
    pub fn new(mut entries: Vec<RegisterEntry>) -> Self {
        // Sort: special registers first (in a fixed order), then a-z
        entries.sort_by(|a, b| {
            let rank = |c: char| -> u8 {
                match c {
                    '"' => 0,
                    '0' => 1,
                    '+' => 2,
                    '-' => 3,
                    '%' => 4,
                    '/' => 5,
                    _ => 6,
                }
            };
            let ra = rank(a.name);
            let rb = rank(b.name);
            ra.cmp(&rb).then_with(|| a.name.cmp(&b.name))
        });
        Self { entries }
    }
}
