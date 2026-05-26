pub mod buffer_view;
pub mod command_line;
pub mod helpers;
pub mod popup;
pub mod rounded_box;
pub mod statusbar;
pub mod statusbar_state;
pub mod tabs;

use crate::ed::editor::Editor;
use ratatui::{layout::Rect, Frame};

pub fn draw(f: &mut Frame, editor: &mut Editor) {
    let size = f.area();
    let tab_h = 1u16;
    let cmd_h = 1u16;
    let status_h = 1u16;

    tabs::draw_tab_bar(f, Rect::new(0, 0, size.width, tab_h), editor);

    command_line::draw_command_line(
        f,
        Rect::new(0, size.height.saturating_sub(cmd_h), size.width, cmd_h),
        editor,
    );

    statusbar::draw_status_bar(
        f,
        Rect::new(
            0,
            size.height.saturating_sub(status_h.saturating_add(cmd_h)),
            size.width,
            status_h,
        ),
        editor,
    );

    buffer_view::draw_windows(
        f,
        Rect::new(
            0,
            tab_h,
            size.width,
            size.height
                .saturating_sub(tab_h.saturating_add(status_h).saturating_add(cmd_h)),
        ),
        editor,
    );

    // Explicit popups (Config, Scankey) — opened/closed by user actions
    popup::draw_popup(f, editor);

    // Live which-key overlay — derived from pending_keys every frame
    popup::draw_which_key(f, editor);
}
