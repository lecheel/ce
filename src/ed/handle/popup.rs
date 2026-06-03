//! Generic popup key handlers (scankey diagnostics, config toggles).

use crate::event::{KeyCode, KeyEvent, KeyModifiers};
use crate::popup::PopupContent;
use crate::Editor;

impl Editor {
    fn handle_config_popup(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Up | KeyCode::Char('k') => {
                if let Some(PopupContent::Config { selected, .. }) = &mut self.popup.content {
                    if *selected > 0 {
                        *selected -= 1;
                    }
                }
            }
            KeyCode::Down | KeyCode::Char('j') => {
                if let Some(PopupContent::Config {
                    selected, items, ..
                }) = &mut self.popup.content
                {
                    if *selected + 1 < items.len() {
                        *selected += 1;
                    }
                }
            }
            KeyCode::Enter | KeyCode::Char(' ') => {
                self.toggle_config_item();
            }
            KeyCode::Esc | KeyCode::Char('q') => {
                self.popup.close();
            }
            _ => {}
        }
    }

    // ── Completion popup key handler ─────────────────────────────────────────
    //
    // The popup is for LSP / buffer / vocab word selection.
    // When an AI ghost is active the popup must NOT be shown; Tab/→ should
    // accept the ghost instead (handled in handle_key's ghost intercept block).

    pub fn handle_popup_key(&mut self, key: crossterm::event::KeyEvent) {
        use crossterm::event::KeyCode;

        // ── Guard: AI ghost takes priority over LSP popup ───────────────
        // If a multi-line ghost is displayed, swallow popup-navigation keys
        // silently so they don't interfere.  Single-key ghost dismissal is
        // already handled in the ghost-intercept block in handle_key().
        if self.comp.has_ghost() {
            // Let the ghost-intercept block in handle_key() deal with it.
            return;
        }
        if key.kind != crossterm::event::KeyEventKind::Press {
            return;
        }

        let popup_kind = self.popup.kind.clone();
        match popup_kind {
            Some(crate::popup::PopupKind::Completion) => {
                self.handle_completion_popup_key(key);
            }
            Some(crate::popup::PopupKind::Config) => {
                self.handle_config_popup(key);
            }
            Some(crate::popup::PopupKind::Scankey) => {
                self.handle_scankey(key);
            }
            _ => {
                // Default: Esc closes any unrecognised popup.
                if key.code == KeyCode::Esc {
                    self.popup.close();
                }
            }
        }
    }

    /// Internal handler for the word-completion popup (LSP / buffer / vocab).
    fn handle_completion_popup_key(&mut self, key: crossterm::event::KeyEvent) {
        use crossterm::event::{KeyCode, KeyModifiers};

        match key.code {
            // ── Navigation ──────────────────────────────────────────────
            KeyCode::Down | KeyCode::Char('n') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.cycle_completion(1);
            }
            KeyCode::Up | KeyCode::Char('p') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.cycle_completion(-1);
            }

            // ── Accept ───────────────────────────────────────────────────
            // Tab and → accept the ghost / selected item.
            // This mirrors the ghost-text intercept in handle_key() but
            // is reached when the popup is visually open.
            KeyCode::Tab | KeyCode::Right if key.modifiers.is_empty() => {
                crate::keybind::bindings::execute_action(
                    self,
                    crate::keybind::bindings::Action::AcceptCompletion,
                );
            }
            KeyCode::Enter => {
                crate::keybind::bindings::execute_action(
                    self,
                    crate::keybind::bindings::Action::AcceptCompletion,
                );
            }

            // ── Dismiss ──────────────────────────────────────────────────
            KeyCode::Esc => {
                self.comp.reset_to_idle();
                self.popup.close();
            }

            // ── Any other key: close popup and fall through ──────────────
            _ => {
                self.popup.close();
                // Re-dispatch so the character is actually inserted.
                self.handle_key(key);
            }
        }
    }

    fn handle_scankey(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Esc | KeyCode::Char('q') => {
                self.popup.close();
                self.scankey_info = None;
                self.clear_status_msg();
            }
            _ => {
                let formatted = crate::keybind::binding_ex::format_key(key);
                let implicit_shift = matches!(key.code, KeyCode::Char(c) if c.is_ascii_uppercase())
                    && !key.modifiers.contains(KeyModifiers::SHIFT);

                let display_key = if implicit_shift {
                    format!("Shift+{}", formatted)
                } else {
                    formatted.clone()
                };

                let char_display = match key.code {
                    KeyCode::Char(c) => format!("'{}'", c),
                    KeyCode::F(n) => format!("F{}", n),
                    KeyCode::Enter => "Enter".into(),
                    KeyCode::Esc => "Esc".into(),
                    other => format!("{:?}", other),
                };

                let mut mods = Vec::new();
                if key.modifiers.contains(KeyModifiers::SHIFT) || implicit_shift {
                    mods.push("Shift");
                }
                if key.modifiers.contains(KeyModifiers::ALT) {
                    mods.push("Alt");
                }
                if key.modifiers.contains(KeyModifiers::CONTROL) {
                    mods.push("Ctrl");
                }
                let mods_display = if mods.is_empty() {
                    "None".into()
                } else {
                    mods.join(" + ")
                };
                let raw_info = format!("Mods: {} | Char: {}", mods_display, char_display);

                let mut action_str = crate::keybind::binding_ex::lookup_key_action(
                    &self.config,
                    &formatted,
                    self.mode,
                    key,
                );
                if action_str == "No binding" {
                    action_str = "NONE".into();
                }

                self.scankey_info =
                    Some((display_key.clone(), action_str.clone(), raw_info.clone()));
                self.popup.open_scankey(display_key, action_str, raw_info);
            }
        }
    }

    fn toggle_config_item(&mut self) {
        // 1. Save the cursor position *before* we rebuild the popup
        let saved_idx = match &self.popup.content {
            Some(PopupContent::Config { selected, .. }) => *selected,
            _ => return,
        };

        // 2. Figure out which config key this item refers to
        let key_idx = match &self.popup.content {
            Some(PopupContent::Config { items, .. }) => items.get(saved_idx).map(|item| item.data),
            _ => None,
        };

        let Some(data) = key_idx else { return };

        // 3. Check if this is a boolean key
        let bool_offset = self.config_bool_keys.len();

        if data < bool_offset {
            // ── Boolean toggle ─────────────────────────────────────────
            if let Some(key) = self.config_bool_keys.get(data).cloned() {
                if let Ok(mut json_val) = serde_json::to_value(&self.config) {
                    if let Some(field) = json_val.get_mut(&key) {
                        if let serde_json::Value::Bool(ref mut v) = field {
                            *v = !*v;
                        }
                    }
                    if let Ok(updated) = serde_json::from_value(json_val) {
                        self.config = updated;
                        let _ = self.config.save();
                    }
                }

                self.open_config_popup();

                if let Some(PopupContent::Config { selected, .. }) = &mut self.popup.content {
                    *selected = saved_idx;
                }
            }
            return;
        }

        // 4. Check if this is a cycle key
        let cycle_idx = data - bool_offset;
        if let Some((key, options)) = self.config_cycle_keys.get(cycle_idx).cloned() {
            if let Ok(mut json_val) = serde_json::to_value(&self.config) {
                if let Some(field) = json_val.get_mut(&key) {
                    let current = field.as_str().unwrap_or("").to_string();
                    let next = match options.iter().position(|o| o == &current) {
                        Some(idx) => options[(idx + 1) % options.len()].clone(),
                        None => options[0].clone(),
                    };
                    *field = serde_json::Value::String(next);
                }
                if let Ok(updated) = serde_json::from_value(json_val) {
                    self.config = updated;
                    let _ = self.config.save();
                }
            }

            self.open_config_popup();

            if let Some(PopupContent::Config { selected, .. }) = &mut self.popup.content {
                *selected = saved_idx;
            }
        }
    }
}
