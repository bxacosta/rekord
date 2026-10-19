//! Pop-up list for choosing the microphone or the output.

use super::theme;
use crate::audio::devices::DeviceList;
use crate::audio::{DeviceChoice, Direction};
use ratatui::Frame;
use ratatui::crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::{Constraint, Flex, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Clear, List, ListItem, ListState, Padding};

pub enum Outcome {
    Open,
    Cancelled,
    Chosen(DeviceChoice),
}

struct Item {
    label: String,
    detail: String,
    choice: DeviceChoice,
}

pub struct Picker {
    direction: Direction,
    items: Vec<Item>,
    current: Option<usize>,
    state: ListState,
}

impl Picker {
    pub fn open(direction: Direction, list: &DeviceList, current: &DeviceChoice) -> Self {
        let default_name = list.default.as_ref().map_or("none", |info| info.name.as_str());
        let mut items = vec![Item {
            label: "System default".into(),
            detail: format!("now {default_name}"),
            choice: DeviceChoice::Default,
        }];
        items.extend(list.devices.iter().map(|info| Item {
            label: info.name.clone(),
            detail: String::new(),
            choice: DeviceChoice::device(info),
        }));
        if direction == Direction::Output {
            items.push(Item {
                label: "Off".into(),
                detail: "don't listen, only record".into(),
                choice: DeviceChoice::Off,
            });
        }

        let current_index = items.iter().position(|item| same_choice(&item.choice, current));
        let mut state = ListState::default();
        state.select(Some(current_index.unwrap_or(0)));
        Self { direction, items, current: current_index, state }
    }

    pub fn handle(&mut self, key: KeyEvent) -> Outcome {
        match key.code {
            KeyCode::Up | KeyCode::Char('k') => self.state.select_previous(),
            KeyCode::Down | KeyCode::Char('j') => self.state.select_next(),
            KeyCode::Home => self.state.select_first(),
            KeyCode::End => self.state.select_last(),
            KeyCode::Enter | KeyCode::Char(' ') => {
                let index = self.state.selected().unwrap_or(0).min(self.items.len() - 1);
                return Outcome::Chosen(self.items[index].choice.clone());
            }
            KeyCode::Char(c @ '1'..='9') => {
                let index = c as usize - '1' as usize;
                if let Some(item) = self.items.get(index) {
                    return Outcome::Chosen(item.choice.clone());
                }
            }
            KeyCode::Esc | KeyCode::Char('q') => return Outcome::Cancelled,
            _ => {}
        }
        Outcome::Open
    }

    pub fn render(&mut self, frame: &mut Frame, area: Rect) {
        let title = match self.direction {
            Direction::Input => " Microphone ",
            Direction::Output => " Listen on ",
        };
        let widest =
            self.items.iter().map(|i| i.label.chars().count() + i.detail.chars().count() + 10).max().unwrap_or(30);
        let width = (widest as u16 + 4).clamp(40, area.width.saturating_sub(4).max(20));
        let height = (self.items.len() as u16 + 4).min(area.height);
        let [popup] = Layout::horizontal([Constraint::Length(width)]).flex(Flex::Center).areas(area);
        let [popup] = Layout::vertical([Constraint::Length(height)]).flex(Flex::Center).areas(popup);

        let items: Vec<ListItem> = self
            .items
            .iter()
            .enumerate()
            .map(|(i, item)| {
                let marker = if Some(i) == self.current { Span::styled("● ", theme::on()) } else { Span::raw("  ") };
                let number = Span::styled(format!("{} ", i + 1), theme::dim());
                let mut spans = vec![marker, number, Span::raw(item.label.clone())];
                if !item.detail.is_empty() {
                    spans.push(Span::styled(format!("  {}", item.detail), theme::dim()));
                }
                ListItem::new(Line::from(spans))
            })
            .collect();

        let hint = Line::from(vec![
            Span::styled(" ↑↓ ", theme::key()),
            Span::styled("move  ", theme::dim()),
            Span::styled("enter ", theme::key()),
            Span::styled("choose  ", theme::dim()),
            Span::styled("esc ", theme::key()),
            Span::styled("cancel ", theme::dim()),
        ]);
        let block = Block::bordered()
            .border_type(BorderType::Rounded)
            .border_style(Style::new().fg(theme::ACCENT))
            .title(Line::styled(title, theme::title()))
            .title_bottom(hint.centered())
            .padding(Padding::uniform(1));
        let list = List::new(items)
            .block(block)
            .highlight_style(Style::new().add_modifier(Modifier::REVERSED))
            .highlight_symbol("");

        frame.render_widget(Clear, popup);
        frame.render_stateful_widget(list, popup, &mut self.state);
    }
}

/// Devices are compared by id only: names may change between connections.
fn same_choice(a: &DeviceChoice, b: &DeviceChoice) -> bool {
    match (a, b) {
        (DeviceChoice::Device { id: x, .. }, DeviceChoice::Device { id: y, .. }) => x == y,
        _ => a == b,
    }
}
