//! Debounce manager for git gutter updates.

use std::collections::HashMap;
use std::time::{Duration, Instant};

const DEBOUNCE_MS: u64 = 500;

#[derive(Debug)]
pub struct DebounceManager {
    last_edits: HashMap<usize, Instant>,
}

impl Default for DebounceManager {
    fn default() -> Self {
        Self::new()
    }
}

impl DebounceManager {
    pub fn new() -> Self {
        Self {
            last_edits: HashMap::new(),
        }
    }

    pub fn notify_edit(&mut self, buffer_id: usize) {
        log::trace!("Debounce: notified edit for buffer_id={}", buffer_id);
        self.last_edits.insert(buffer_id, Instant::now());
    }

    /// Returns a list of buffer IDs that have exceeded the 500ms debounce window.
    pub fn poll_and_dispatch(&mut self) -> Vec<usize> {
        let mut to_dispatch = Vec::new();

        self.last_edits.retain(|&buffer_id, &mut t| {
            if t.elapsed() >= Duration::from_millis(DEBOUNCE_MS) {
                to_dispatch.push(buffer_id);
                false // remove from map
            } else {
                true // keep in map
            }
        });

        if !to_dispatch.is_empty() {
            log::debug!(
                "Debounce: dispatching git diff for buffer_ids={:?}",
                to_dispatch
            );
        }

        to_dispatch
    }
}
