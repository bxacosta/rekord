//! rekord: listen to a microphone live with clearer voice, and record it safely.

mod app;
mod audio;
mod cli;
mod dsp;
mod format;
mod recording;
mod settings;
mod shutdown;
mod ui;

use anyhow::{Context, Result, bail};
use app::{App, Severity, Startup};
use audio::{DeviceChoice, Direction, devices};
use clap::Parser;
use cli::{Cli, Command};
use ratatui::crossterm::style::Stylize;
use recording::Take;
use settings::{GAIN_RANGE_DB, Settings, SettingsFile};
use std::io::IsTerminal;
use std::path::Path;
use std::process::ExitCode;
use std::time::Instant;

fn main() -> ExitCode {
    match run(Cli::parse()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("{} {e:#}", "error:".red().bold());
            ExitCode::FAILURE
        }
    }
}

fn run(cli: Cli) -> Result<()> {
    let settings_file = SettingsFile::locate();
    let (saved, warning) = settings_file.load();
    let host = cpal::default_host();

    if let Some(Command::Devices) = cli.command {
        return print_devices(&host, &saved);
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
    let deadline = cli.time.map(|t| Instant::now() + t);
    let mut app = App::start(Startup { settings, saved, settings_file, recordings_dir, record: cli.record, notices })?;
    let interactive = std::io::stdin().is_terminal() && std::io::stdout().is_terminal();
    let result = if interactive { ui::run(&mut app, deadline) } else { ui::plain::run(&mut app, deadline) };

    let dir = app.recordings_dir().to_owned();
    let (takes, errors) = app.finish();
    shutdown::completed();

    print_summary(&takes, &dir);
    for error in &errors {
        eprintln!("{} {error}", "error:".red().bold());
    }
    result
}

/// Applies one-off command-line options on top of the saved settings.
fn apply_overrides(cli: &Cli, host: &cpal::Host, mut settings: Settings) -> Result<Settings> {
    if let Some(name) = &cli.input {
        settings.input = find_device(host, Direction::Input, name)?;
    }
    if let Some(name) = &cli.output {
        settings.output = if name.eq_ignore_ascii_case("off") {
            DeviceChoice::Off
        } else {
            find_device(host, Direction::Output, name)?
        };
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

fn find_device(host: &cpal::Host, direction: Direction, name: &str) -> Result<DeviceChoice> {
    match devices::find_by_name(host, direction, name)? {
        Some(info) => Ok(DeviceChoice::device(&info)),
        None => {
            let kind = if direction == Direction::Input { "microphone" } else { "output" };
            bail!("No {kind} matches \"{name}\". Run `rekord devices` to see what is connected.")
        }
    }
}

fn print_devices(host: &cpal::Host, saved: &Settings) -> Result<()> {
    for (direction, title, choice) in
        [(Direction::Input, "Microphones", &saved.input), (Direction::Output, "Outputs", &saved.output)]
    {
        println!("{}", title.bold());
        let default = devices::default_device(host, direction).ok().map(|(_, info)| info.id);
        let list = devices::list(host, direction)?;
        if list.is_empty() {
            println!("  {}", "none connected".dark_grey());
        }
        let width = list.iter().map(|d| d.name.chars().count()).max().unwrap_or(0);
        for device in list {
            let selected = matches!(choice, DeviceChoice::Device { id, .. } if *id == device.id)
                || (*choice == DeviceChoice::Default && default.as_ref() == Some(&device.id));
            let marker = if selected { "●".green().to_string() } else { " ".into() };
            let mut tags = Vec::new();
            if selected {
                tags.push("in use".green().to_string());
            }
            if default.as_ref() == Some(&device.id) {
                tags.push("system default".dark_grey().to_string());
            }
            println!("  {marker} {:<width$}   {}", device.name, tags.join(&" · ".dark_grey().to_string()));
        }
        println!();
    }
    println!("{}", "Choose with `rekord -i <name>` / `-o <name>`, or press i / o inside the app.".dark_grey());
    Ok(())
}

fn print_summary(takes: &[Take], dir: &Path) {
    if takes.is_empty() {
        return;
    }
    let count = takes.len();
    println!(
        "{} {count} recording{} in {}",
        "Saved".green().bold(),
        if count == 1 { "" } else { "s" },
        format::path(dir)
    );
    for take in takes {
        let names: Vec<_> = take.files.iter().filter_map(|f| f.file_name()).map(|n| n.to_string_lossy()).collect();
        println!("  {}  {}", format::duration(take.duration).bold(), names.join(&", ".dark_grey().to_string()));
    }
}
