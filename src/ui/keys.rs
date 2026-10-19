//! Keyboard shortcuts of the main screen.

use crate::app::Action;
use crate::audio::Direction;
use crate::recording::Control;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

pub enum Command {
    Do(Action),
    Pick(Direction),
}

pub fn command(key: KeyEvent) -> Option<Command> {
    if key.modifiers.contains(KeyModifiers::CONTROL) {
        return matches!(key.code, KeyCode::Char('c')).then_some(Command::Do(Action::Quit));
    }
    let action = match key.code {
        KeyCode::Char(' ' | 'r' | 'p') => Action::Record(Control::Toggle),
        KeyCode::Char('s') => Action::Record(Control::Stop),
        KeyCode::Char('e') => Action::ToggleEnhance,
        KeyCode::Char('n') => Action::ToggleDenoise,
        KeyCode::Char('[') => Action::ChangeDenoiseLevel(-1),
        KeyCode::Char(']') => Action::ChangeDenoiseLevel(1),
        KeyCode::Up | KeyCode::Char('+' | '=') => Action::ChangeGain(1),
        KeyCode::Down | KeyCode::Char('-') => Action::ChangeGain(-1),
        KeyCode::Char('m') => Action::ToggleMonitor,
        KeyCode::Char('i') => return Some(Command::Pick(Direction::Input)),
        KeyCode::Char('o') => return Some(Command::Pick(Direction::Output)),
        KeyCode::Char('q') | KeyCode::Esc => Action::Quit,
        _ => return None,
    };
    Some(Command::Do(action))
}
