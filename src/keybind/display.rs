// keybind/display.rs
//! Human-readable representations of actions.

use crate::keybind::actions::Action;

pub fn action_display_name(action: &Action) -> String {
    let raw = format!("{:?}", action);
    let (main, suffix) = match raw.find('(') {
        Some(pos) => (&raw[..pos], Some(&raw[pos..])),
        None => (raw.as_str(), None),
    };
    let mut out = String::with_capacity(main.len() + 8);
    for (i, ch) in main.chars().enumerate() {
        if ch.is_uppercase() && i > 0 {
            out.push(' ');
        }
        out.push(ch);
    }
    if let Some(suf) = suffix {
        out.push_str(suf);
    }
    out
}
