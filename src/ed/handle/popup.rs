//! Generic popup key handlers (scankey diagnostics, config toggles).

use crate::event::{KeyCode, KeyEvent, KeyModifiers};
use crate::popup::PopupContent;
use crate::Editor;

impl Editor {
    pub fn handle_popup_key(&mut self, key: KeyEvent) {
        // ── Scankey mode ──────────────────────────────────────────────
        if matches!(self.popup.content, Some(PopupContent::Scankey { .. })) {
            self.handle_scankey(key);
            return;
        }

        // ── Config / generic popup ────────────────────────────────────
        match key.code {
            KeyCode::Esc => {
                self.popup.close();
            }
            KeyCode::Char('j') | KeyCode::Down => {
                self.popup.config_next();
            }
            KeyCode::Char('k') | KeyCode::Up => {
                self.popup.config_prev();
            }
            KeyCode::Char(' ') => self.toggle_config_item(),
            _ => {}
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

        // 2. Figure out which config key to flip
        let key_idx = match &self.popup.content {
            Some(PopupContent::Config { items, .. }) => items.get(saved_idx).map(|item| item.data),
            _ => None,
        };

        if let Some(data) = key_idx {
            if let Some(key) = self.config_bool_keys.get(data).cloned() {
                // 3. Toggle the boolean value
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

                // 4. Rebuild popup (this resets selected → 0)
                self.open_config_popup();

                // 5. Restore the cursor position
                if let Some(PopupContent::Config { selected, .. }) = &mut self.popup.content {
                    *selected = saved_idx;
                }
            }
        }
    }
}
