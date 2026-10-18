//! Plain console output outside the interactive UI. Colors are dropped automatically
//! when the output is redirected or `NO_COLOR` is set.

use crate::audio::{DeviceChoice, Direction, devices};
use crate::format;
use crate::recording::Take;
use crate::settings::Settings;
use anstream::{eprintln, println};
use anstyle::{AnsiColor, Style};
use anyhow::Result;
use std::path::Path;

const HEADING: Style = Style::new().bold();
const GOOD: Style = AnsiColor::Green.on_default();
const STRONG_GOOD: Style = AnsiColor::Green.on_default().bold();
const ERROR: Style = AnsiColor::Red.on_default().bold();
const DIM: Style = AnsiColor::BrightBlack.on_default();

pub fn error(message: impl std::fmt::Display) {
    eprintln!("{ERROR}error:{ERROR:#} {message}");
}

/// `rekord devices`: connected devices, marking the one in use and the system default.
pub fn print_devices(host: &cpal::Host, saved: &Settings) -> Result<()> {
    let sections = [(Direction::Input, "Microphones", &saved.input), (Direction::Output, "Outputs", &saved.output)];
    for (direction, title, choice) in sections {
        println!("{HEADING}{title}{HEADING:#}");
        let default = devices::default_device(host, direction).ok().map(|(_, info)| info.id);
        let list = devices::list(host, direction)?;
        if list.is_empty() {
            println!("  {DIM}none connected{DIM:#}");
        }
        let width = list.iter().map(|d| d.name.chars().count()).max().unwrap_or(0);
        for device in list {
            let is_default = default.as_ref() == Some(&device.id);
            let in_use = match choice {
                DeviceChoice::Device { id, .. } => *id == device.id,
                DeviceChoice::Default => is_default,
                DeviceChoice::Off => false,
            };
            let marker = if in_use { format!("{GOOD}●{GOOD:#}") } else { " ".into() };
            let mut tags = Vec::new();
            if in_use {
                tags.push(format!("{GOOD}in use{GOOD:#}"));
            }
            if is_default {
                tags.push(format!("{DIM}system default{DIM:#}"));
            }
            println!("  {marker} {:<width$}   {}", device.name, tags.join(&format!("{DIM} · {DIM:#}")));
        }
        if *choice == DeviceChoice::Off {
            println!("  {GOOD}●{GOOD:#} {DIM}Off: not listening{DIM:#}");
        }
        println!();
    }
    println!("{DIM}Pick one with `rekord -i <name>` / `-o <name>`, or press i / o inside the app.{DIM:#}");
    Ok(())
}

/// What was recorded during the session, printed on exit.
pub fn print_summary(takes: &[Take], errors: &[String], dir: &Path) {
    if !takes.is_empty() {
        let plural = if takes.len() == 1 { "" } else { "s" };
        println!("{STRONG_GOOD}Saved{STRONG_GOOD:#} {} recording{plural} in {}", takes.len(), format::path(dir));
        for take in takes {
            let names: Vec<_> = take.files.iter().filter_map(|f| f.file_name()).map(|n| n.to_string_lossy()).collect();
            println!(
                "  {HEADING}{}{HEADING:#}  {}",
                format::duration(take.duration),
                names.join(&format!("{DIM},{DIM:#} "))
            );
        }
    }
    for message in errors {
        error(message);
    }
}
