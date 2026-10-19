//! rekord: listen to a microphone live with clearer voice, and record it safely.

mod app;
mod audio;
mod cli;
mod console;
mod dsp;
mod format;
mod recording;
mod settings;
mod shutdown;
mod ui;

use anyhow::{Context, Result, bail};
use app::{App, Severity, Startup};
use audio::devices::Inventory;
use audio::{DeviceChoice, Direction};
use clap::Parser;
use cli::{Cli, Command};
use settings::{GAIN_RANGE_DB, Settings, SettingsFile};
use std::io::IsTerminal;
use std::process::ExitCode;
use std::time::Instant;

fn main() -> ExitCode {
    match run(Cli::parse()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            console::error(format!("{e:#}"));
            ExitCode::FAILURE
        }
    }
}

fn run(cli: Cli) -> Result<()> {
    let settings_file = SettingsFile::locate();
    let (saved, warning) = settings_file.load();
    let host = cpal::default_host();

    if let Some(Command::Devices) = cli.command {
        return console::print_devices(&host, &saved);
    }

    let mut notices = Vec::new();
    notices.extend(warning.map(|w| (Severity::Warning, w)));
    let settings = apply_overrides(&cli, &host, saved.clone())?;

    let recordings_dir =
        cli.dir.clone().or_else(|| settings.recordings_dir.clone()).unwrap_or_else(settings::default_recordings_dir);
    std::fs::create_dir_all(&recordings_dir).with_context(|| format!("Cannot create {}", recordings_dir.display()))?;
    let recovered = recording::recover_dir(&recordings_dir);
    if !recovered.is_empty() {
        let names: Vec<_> = recovered.iter().filter_map(|p| p.file_name()).map(|n| n.to_string_lossy()).collect();
        notices.push((Severity::Success, format!("Recovered interrupted recording: {}", names.join(", "))));
    }

    shutdown::install();
    // A limit too far ahead to represent is the same as no limit.
    let deadline = cli.time.and_then(|t| Instant::now().checked_add(t));
    let mut app = App::start(Startup { settings, saved, settings_file, recordings_dir, record: cli.record, notices })?;
    let interactive = std::io::stdin().is_terminal() && std::io::stdout().is_terminal();
    let result = if interactive { ui::run(&mut app, deadline) } else { ui::plain::run(&mut app, deadline) };

    let dir = app.recordings_dir().to_owned();
    let (takes, errors) = app.finish();
    shutdown::completed();

    console::print_summary(&takes, &errors, &dir);
    result
}

/// Applies one-off command-line options on top of the saved settings.
fn apply_overrides(cli: &Cli, host: &cpal::Host, mut settings: Settings) -> Result<Settings> {
    if cli.input.is_some() || cli.output.is_some() {
        let inventory = Inventory::scan(host)?;
        if let Some(name) = &cli.input {
            settings.input = find_device(&inventory, Direction::Input, name)?;
        }
        if let Some(name) = &cli.output {
            settings.output = if name.eq_ignore_ascii_case("off") {
                DeviceChoice::Off
            } else {
                find_device(&inventory, Direction::Output, name)?
            };
        }
    }
    if let Some(gain) = cli.gain {
        let (min, max) = GAIN_RANGE_DB;
        if !(min..=max).contains(&gain) {
            bail!("Gain must be between {min} and {max} dB");
        }
        settings.gain_db = gain;
    }
    if let Some(on) = cli.enhance {
        settings.enhance = on;
    }
    if let Some(on) = cli.denoise {
        settings.denoise = on;
    }
    Ok(settings)
}

fn find_device(inventory: &Inventory, direction: Direction, name: &str) -> Result<DeviceChoice> {
    match inventory.find_by_name(direction, name) {
        Some(info) => Ok(DeviceChoice::device(info)),
        None => {
            let kind = if direction == Direction::Input { "microphone" } else { "output" };
            bail!("No {kind} matches \"{name}\". Run `rekord devices` to see what is connected.")
        }
    }
}
