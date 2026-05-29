// popup/error.rs (or alongside other popup types)

#[derive(Debug, Clone)]
pub struct ErrorPopup {
    pub lines: Vec<String>,
}

impl ErrorPopup {
    pub fn new(message: &str) -> Self {
        let lines: Vec<String> = message.lines().take(5).map(|l| l.to_string()).collect();
        Self { lines }
    }

    pub fn is_open(&self) -> bool {
        !self.lines.is_empty()
    }
}
