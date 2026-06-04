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

    /// Unified dispatcher for ALL popup types.
    /// Called from handle_key when any popup or overlay is active.
    pub fn handle_any_popup_key(&mut self, key: crossterm::event::KeyEvent) {
        use crossterm::event::KeyCode;

        // ── Error popup takes absolute priority ────────────────────
        if self.popup.error.is_some() {
            if key.code == KeyCode::Esc || key.code == KeyCode::Enter {
                self.popup.error = None;
                self.popup.kind = None;
            }
            return;
        }

        if key.kind != crossterm::event::KeyEventKind::Press {
            return;
        }

        // ── Ghost text takes priority over popup navigation ───────
        if self.comp.has_ghost() {
            return;
        }

        // ── Dispatch to the specific popup handler ────────────────
        // Ordered by frequency of use for slight perf benefit

        if self.popup.command_palette.is_some() {
            self.handle_command_palette_key(key);
            return;
        }
        if self.popup.file_picker.is_some() {
            self.handle_file_picker_key(key);
            return;
        }
        if self.popup.fd.is_some() {
            self.handle_fd_key(key);
            return;
        }
        if self.popup.quickfix.is_some() {
            self.handle_quickfix_key(key);
            return;
        }
        if self.popup.buffer_list.is_some() {
            self.handle_buffer_list_key(key);
            return;
        }
        if self.popup.function_list.is_some() {
            self.handle_function_list_key(key);
            return;
        }
        if self.popup.mru.is_some() {
            self.handle_mru_key(key);
            return;
        }
        if self.popup.registers.is_some() {
            self.handle_registers_key(key);
            return;
        }
        if self.popup.marks.is_some() {
            self.handle_marks_key(key);
            return;
        }
        if self.popup.guide.is_some() {
            self.handle_guide_popup_key(key);
            return;
        }
        if self.popup.git_hunk.is_some() {
            self.handle_git_hunk_popup_key(key);
            return;
        }
        if self.popup.tag_candidates.is_some() {
            self.handle_tag_candidates_key(key);
            return;
        }
        if self.popup.workspace_symbols.is_some() {
            self.handle_workspace_symbols_key(key);
            return;
        }

        // ── Config / Scankey / Completion (handled by kind) ───────
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
            Some(crate::popup::PopupKind::Whichkey) => {
                // WhichKey is a passive overlay — don't intercept keys!
            }
            _ => {
                // Fallback: Esc closes any unrecognised popup
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
