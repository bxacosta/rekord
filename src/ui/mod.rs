//! Front ends: an interactive terminal UI, and a plain line-based mode used when
//! the output is not a terminal (logs, scripts, services).

mod keys;
mod meter;
mod picker;
pub mod plain;
mod theme;
mod view;

use crate::app::{Action, App, Severity};
use crate::audio::{Connection, Direction};
use crate::shutdown;
use anyhow::Result;
use meter::LevelMeter;
use picker::{Outcome, Picker};
use ratatui::DefaultTerminal;
use ratatui::crossterm::event::{self, Event, KeyEventKind};
use std::time::{Duration, Instant};
use view::ViewState;

/// Screen refresh interval (~30 fps keeps the level meter fluid).
const FRAME: Duration = Duration::from_millis(33);

/// Runs the interactive UI until the user quits or `deadline` passes.
pub fn run(app: &mut App, deadline: Option<Instant>) -> Result<()> {
    let mut terminal = ratatui::try_init()?;
    let result = event_loop(&mut terminal, app, deadline);
    ratatui::try_restore()?;
    result
}

fn event_loop(terminal: &mut DefaultTerminal, app: &mut App, deadline: Option<Instant>) -> Result<()> {
    let host = cpal::default_host();
    let started = Instant::now();
    let mut meter = LevelMeter::new();
    let mut picker: Option<(Direction, Picker)> = None;

    loop {
        app.tick();
        let snapshot = app.snapshot();
        if matches!(snapshot.connection, Connection::Live { .. }) {
            meter.update(app.take_peak());
        } else {
            meter.reset();
        }

        terminal.draw(|frame| {
            let view = ViewState { snapshot: &snapshot, meter: &meter, blink: view::blink_phase(started) };
            let panel = view::draw(frame, &view);
            if let Some((_, picker)) = picker.as_mut() {
                picker.render(frame, panel);
            }
        })?;

        if app.should_quit() || shutdown::requested() || deadline.is_some_and(|d| Instant::now() >= d) {
            return Ok(());
        }
        if !event::poll(FRAME)? {
            continue;
        }
        let Event::Key(key) = event::read()? else { continue };
        // Windows also reports key releases; acting on them would double every press.
        if key.kind == KeyEventKind::Release {
            continue;
        }

        if let Some((direction, open)) = picker.as_mut() {
            match open.handle(key) {
                Outcome::Open => {}
                Outcome::Cancelled => picker = None,
                Outcome::Chosen(choice) => {
                    app.apply(match direction {
                        Direction::Input => Action::SetInput(choice),
                        Direction::Output => Action::SetOutput(choice),
                    });
                    picker = None;
                }
            }
            continue;
        }

        match keys::command(key) {
            Some(keys::Command::Do(action)) => app.apply(action),
            Some(keys::Command::Pick(direction)) => {
                let current = match direction {
                    Direction::Input => &snapshot.settings.input,
                    Direction::Output => &snapshot.settings.output,
                };
                match Picker::open(&host, direction, current) {
                    Ok(open) => picker = Some((direction, open)),
                    Err(e) => app.notify(Severity::Error, format!("Could not list devices: {e:#}")),
                }
            }
            None => {}
        }
    }
}
