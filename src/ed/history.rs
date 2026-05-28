//! Command history persistence and prefix-search navigation.

use crate::Config;
use crate::Editor;

impl Editor {
    pub fn load_history() -> Vec<String> {
        Config::config_dir()
            .ok()
            .and_then(|dir| std::fs::read_to_string(dir.join("history.txt")).ok())
            .map(|content| {
                content
                    .lines()
                    .map(String::from)
                    .filter(|s| !s.is_empty())
                    .collect()
            })
            .unwrap_or_default()
    }

    pub fn save_history(history: &[String]) {
        if let Ok(dir) = Config::config_dir() {
            let start = history.len().saturating_sub(500);
            let _ = std::fs::write(dir.join("history.txt"), history[start..].join("\n"));
        }
    }

    pub fn append_and_save_history(&mut self, cmd: &str) {
        let trimmed = cmd.trim();
        if trimmed.is_empty() {
            return;
        }
        if self.cmd_history.last().map(|s| s.as_str()) != Some(trimmed) {
            self.cmd_history.push(trimmed.to_string());
            Self::save_history(&self.cmd_history);
        }
    }

    pub fn history_prev(&mut self) {
        if self.cmd_history.is_empty() {
            return;
        }

        if self.cmd_history_idx.is_none() {
            self.cmd_temp_input = self.command.clone();
            self.history_search_prefix = Some(self.command.clone());
        }

        let prefix = self.history_search_prefix.as_deref().unwrap_or("");
        let start_idx = self
            .cmd_history_idx
            .map_or(self.cmd_history.len().saturating_sub(1), |i| {
                i.saturating_sub(1)
            });

        if let Some(idx) = (0..=start_idx)
            .rev()
            .find(|&i| self.cmd_history[i].starts_with(prefix))
        {
            self.cmd_history_idx = Some(idx);
            self.command = self.cmd_history[idx].clone();
            self.command_cursor = self.command.len();
        }
    }

    pub fn history_next(&mut self) {
        if self.cmd_history.is_empty() || self.cmd_history_idx.is_none() {
            return;
        }

        let prefix = self.history_search_prefix.as_deref().unwrap_or("");
        let start_idx = self.cmd_history_idx.unwrap().saturating_add(1);

        if let Some(idx) =
            (start_idx..self.cmd_history.len()).find(|&i| self.cmd_history[i].starts_with(prefix))
        {
            self.cmd_history_idx = Some(idx);
            self.command = self.cmd_history[idx].clone();
        } else {
            self.cmd_history_idx = None;
            self.command = self.cmd_temp_input.clone();
        }
        self.command_cursor = self.command.len();
    }
}
