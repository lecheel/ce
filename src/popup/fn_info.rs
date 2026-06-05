// popup/fn_info.rs
//! F-key quick-reference popup (compact 2-line overlay).

use crate::config::app_config::Config;
use crate::ed::mode::Mode;

#[derive(Debug, Clone)]
pub struct FnInfoPopup {
    /// One entry per F-key: ("F1", "Window Nav") or ("F3", "—")
    pub entries: Vec<(String, String)>,
    pub title: String,
}

impl FnInfoPopup {
    /// Build the popup content for the given mode and config.
    pub fn build(mode: Mode, config: &Config) -> Self {
        let mut entries = Vec::with_capacity(12);

        for n in 1u8..=12 {
            let key_str = format!("f{}", n);

            // 1. Check user config overrides first
            let action_opt =
                crate::keybind::config_keys::find_custom_action(config, &key_str, mode);

            // 2. Fall back to hardcoded defaults
            let action_name = if let Some(action) = action_opt {
                crate::keybind::display::action_display_name(&action)
            } else {
                resolve_default_fn_action(n, mode)
            };

            entries.push((format!("F{}", n), action_name));
        }

        Self {
            entries,
            title: " F-Keys ".into(),
        }
    }
}

/// Resolve the default (hardcoded) action for an F-key in the given mode.
fn resolve_default_fn_action(n: u8, mode: Mode) -> String {
    match mode {
        Mode::Brief => match n {
            1 => "Window Nav".into(),
            2 => "Close Win".into(),
            9 => "Command".into(),
            _ => "—".into(),
        },
        Mode::Insert => match n {
            _ => "—".into(),
        },
        _ => "—".into(),
    }
}
