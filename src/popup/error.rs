// popup/error.rs (or alongside other popup types)

#[derive(Debug, Clone)]
pub struct ErrorPopup {
    pub lines: Vec<String>,
    pub title: String,
    pub is_error: bool,
}

impl ErrorPopup {
    pub fn new(message: &str) -> Self {
        let lines: Vec<String> = message.lines().take(5).map(|l| l.to_string()).collect();
        Self {
            lines,
            title: " Error ".to_string(),
            is_error: true,
        }
    }

    pub fn new_info(message: &str, title: &str) -> Self {
        let lines: Vec<String> = message.lines().map(|l| l.to_string()).collect();
        Self {
            lines,
            title: format!(" {} ", title),
            is_error: false,
        }
    }

    pub fn is_open(&self) -> bool {
        !self.lines.is_empty()
    }
}
