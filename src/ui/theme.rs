//! Colors and text styles. Only the 16 standard terminal colors are used, so the UI
//! follows the user's terminal theme (light or dark).

use ratatui::style::{Color, Modifier, Style};

pub const ACCENT: Color = Color::Cyan;
pub const GOOD: Color = Color::Green;
pub const WARN: Color = Color::Yellow;
pub const BAD: Color = Color::Red;
pub const MUTED: Color = Color::DarkGray;

pub fn title() -> Style {
    Style::new().fg(ACCENT).add_modifier(Modifier::BOLD)
}

pub fn label() -> Style {
    Style::new().fg(MUTED)
}

pub fn value() -> Style {
    Style::new().add_modifier(Modifier::BOLD)
}

pub fn dim() -> Style {
    Style::new().fg(MUTED)
}

pub fn key() -> Style {
    Style::new().fg(ACCENT).add_modifier(Modifier::BOLD)
}

pub fn on() -> Style {
    Style::new().fg(GOOD).add_modifier(Modifier::BOLD)
}

pub fn border() -> Style {
    Style::new().fg(MUTED)
}
