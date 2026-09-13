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

pub const MODIFIED_FG: Color = Color::Yellow;
pub const ADDED_FG: Color = Color::Green;
pub const DELETED_FG: Color = Color::Red;
pub const RENAMED_FG: Color = Color::Cyan;
pub const UNTRACKED_FG: Color = Color::Green;

pub const ADD_BG: Color = Color::Rgb(38, 80, 48);
pub const DEL_BG: Color = Color::Rgb(95, 45, 50);

/// Stronger background for inline-emphasized characters within an added line.
pub const INLINE_ADD_BG: Color = Color::Rgb(55, 115, 65);
/// Stronger background for inline-emphasized characters within a deleted line.
pub const INLINE_DEL_BG: Color = Color::Rgb(140, 60, 60);
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