//! Git hunk diff popup overlay — shows the unified diff for the hunk under the cursor.

#[derive(Debug, Clone)]
pub struct GitHunkPopup {
    pub lines: Vec<String>,
    pub scroll: usize,
}

impl GitHunkPopup {
    pub fn new(lines: Vec<String>) -> Self {
        Self { lines, scroll: 0 }
    }

    pub fn move_up(&mut self) {
        if self.scroll > 0 {
            self.scroll -= 1;
        }
    }

    pub fn move_down(&mut self) {
        if self.scroll + 1 < self.lines.len() {
            self.scroll += 1;
        }
    }
}
