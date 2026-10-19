//! Plain console output outside the interactive UI. Colors are dropped automatically
//! when the output is redirected or `NO_COLOR` is set.

use crate::audio::DeviceChoice;
use crate::audio::devices::Inventory;
use crate::format;
use crate::recording::Take;
use crate::settings::Settings;
use anstyle::{AnsiColor, Style};
use anyhow::Result;
use std::io::Write as _;
use std::path::Path;

/// `println!` that never panics. Write errors are ignored: a closed pipe
/// (`rekord ... | head`) must not crash the app, least of all while recording.
macro_rules! say {
    ($($arg:tt)*) => {{
        use std::io::Write as _;
        let _ = writeln!(anstream::stdout(), $($arg)*);
    }};
}
pub(crate) use say;

const HEADING: Style = Style::new().bold();
const GOOD: Style = AnsiColor::Green.on_default();
const STRONG_GOOD: Style = AnsiColor::Green.on_default().bold();
const ERROR: Style = AnsiColor::Red.on_default().bold();
const DIM: Style = AnsiColor::BrightBlack.on_default();

pub fn error(message: impl std::fmt::Display) {
    let _ = writeln!(anstream::stderr(), "{ERROR}error:{ERROR:#} {message}");
}

/// `rekord devices`: connected devices, marking the one in use and the system default.
pub fn print_devices(host: &cpal::Host, saved: &Settings) -> Result<()> {
    let lists = Inventory::scan(host)?.lists();
    let sections = [(&lists.inputs, "Microphones", &saved.input), (&lists.outputs, "Outputs", &saved.output)];
    for (list, title, choice) in sections {
        say!("{HEADING}{title}{HEADING:#}");
        if list.devices.is_empty() {
            say!("  {DIM}none connected{DIM:#}");
        }
        let width = list.devices.iter().map(|d| d.name.chars().count()).max().unwrap_or(0);
        for device in &list.devices {
            let is_default = list.default.as_ref() == Some(device);
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
            say!("  {marker} {:<width$}   {}", device.name, tags.join(&format!("{DIM} · {DIM:#}")));
        }
        if *choice == DeviceChoice::Off {
            say!("  {GOOD}●{GOOD:#} {DIM}Off: not listening{DIM:#}");
        }
        say!();
    }
    say!("{DIM}Pick one with `rekord -i <name>` / `-o <name>`, or press i / o inside the app.{DIM:#}");
    Ok(())
}

/// What was recorded during the session, printed on exit.
pub fn print_summary(takes: &[Take], errors: &[String], dir: &Path) {
    if !takes.is_empty() {
        let plural = if takes.len() == 1 { "" } else { "s" };
        say!("{STRONG_GOOD}Saved{STRONG_GOOD:#} {} recording{plural} in {}", takes.len(), format::path(dir));
        for take in takes {
            let names: Vec<_> = take.files.iter().filter_map(|f| f.file_name()).map(|n| n.to_string_lossy()).collect();
            say!(
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
