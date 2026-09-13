use ratatui::style::{Color, Modifier, Style};

pub const HEADER_FG: Color = Color::Cyan;
pub const ACTIVE_BORDER: Color = Color::Cyan;
pub const INACTIVE_BORDER: Color = Color::DarkGray;
pub const DIR_FG: Color = Color::Yellow;
pub const FILE_FG: Color = Color::White;
pub const SELECTED_BG: Color = Color::Blue;
pub const STATUS_OK: Color = Color::Green;
pub const STATUS_ERR: Color = Color::Red;
pub const DIM: Color = Color::DarkGray;
pub const HINT: Color = Color::Gray;

/// Background for the selected file-list row (whole row highlight).
pub const SELECT_ROW_BG: Color = Color::Rgb(44, 54, 74);

pub const MODIFIED_FG: Color = Color::Yellow;
pub const ADDED_FG: Color = Color::Green;
pub const DELETED_FG: Color = Color::Red;
pub const RENAMED_FG: Color = Color::Cyan;
pub const UNTRACKED_FG: Color = Color::Green;

pub const ADD_BG: Color = Color::Rgb(30, 62, 42);
pub const DEL_BG: Color = Color::Rgb(80, 36, 42);

/// Stronger background for inline-emphasized characters within an added line.
pub const INLINE_ADD_BG: Color = Color::Rgb(60, 140, 78);
/// Stronger background for inline-emphasized characters within a deleted line.
pub const INLINE_DEL_BG: Color = Color::Rgb(175, 62, 60);
/// Background for trailing whitespace markers.
pub const TRAILING_WS_BG: Color = Color::Rgb(150, 90, 40);
/// Background for the current diff cursor line.
pub const CURSOR_BG: Color = Color::Rgb(40, 50, 70);

pub fn status_fg(status: crate::model::Status) -> Color {
    match status {
        crate::model::Status::Modified => MODIFIED_FG,
        crate::model::Status::Added => ADDED_FG,
        crate::model::Status::Deleted => DELETED_FG,
        crate::model::Status::Renamed => RENAMED_FG,
        crate::model::Status::Untracked => UNTRACKED_FG,
    }
}

/// Background color for the status badge (a filled letter block).
pub fn status_badge(status: crate::model::Status) -> Color {
    match status {
        crate::model::Status::Modified => Color::Rgb(110, 84, 22),
        crate::model::Status::Added => Color::Rgb(26, 92, 46),
        crate::model::Status::Deleted => Color::Rgb(120, 36, 36),
        crate::model::Status::Renamed => Color::Rgb(22, 78, 92),
        crate::model::Status::Untracked => Color::Rgb(26, 92, 46),
    }
}

pub fn header_style() -> Style {
    Style::default().fg(Color::White).add_modifier(Modifier::BOLD)
}

pub fn help_style() -> Style {
    Style::default().fg(HINT)
}

pub fn separator_style() -> Style {
    Style::default().fg(DIM)
}

pub fn key_style() -> Style {
    Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD)
}

pub fn status_sep_style() -> Style {
    Style::default().fg(DIM)
}