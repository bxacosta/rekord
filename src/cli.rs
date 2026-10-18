//! Command-line interface.

use clap::{ArgAction, Parser, Subcommand};
use std::path::PathBuf;
use std::time::Duration;

/// Listen to a microphone live with clearer voice, and record it safely.
///
/// Options only apply to this run. Changes made inside the app (devices, gain,
/// effects) are remembered for next time.
#[derive(Debug, Parser)]
#[command(name = "rekord", bin_name = "rekord", version, max_term_width = 100)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Option<Command>,

    /// Microphone to use: any part of its name
    #[arg(short, long, value_name = "NAME")]
    pub input: Option<String>,

    /// Where to listen: any part of a device name, or "off" to not listen
    #[arg(short, long, value_name = "NAME")]
    pub output: Option<String>,

    /// Start recording right away
    #[arg(short, long)]
    pub record: bool,

    /// Voice enhance: clearer, more even speech
    #[arg(short, long, value_name = "on|off", num_args = 0..=1, require_equals = true, default_missing_value = "on", action = ArgAction::Set,
          value_parser = clap::builder::BoolishValueParser::new(), hide_possible_values = true)]
    pub enhance: Option<bool>,

    /// Noise reduction: removes background noise
    #[arg(short = 'n', long, value_name = "on|off", num_args = 0..=1, require_equals = true, default_missing_value = "on", action = ArgAction::Set,
          value_parser = clap::builder::BoolishValueParser::new(), hide_possible_values = true)]
    pub denoise: Option<bool>,

    /// Input gain in dB, from -12 to 30
    #[arg(short, long, value_name = "DB", allow_negative_numbers = true)]
    pub gain: Option<f32>,

    /// Folder for recordings [default: Music\Rekord]
    #[arg(short, long, value_name = "FOLDER")]
    pub dir: Option<PathBuf>,

    /// Quit automatically after this long, e.g. 90s, 45m, 1h30m
    #[arg(short, long, value_name = "DURATION", value_parser = parse_duration)]
    pub time: Option<Duration>,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// List microphones and outputs
    Devices,
}

/// Parses "90", "90s", "45m", "1h30m"... A bare number means seconds.
fn parse_duration(text: &str) -> Result<Duration, String> {
    let invalid = || format!("invalid duration \"{text}\" (examples: 90s, 45m, 1h30m)");
    let text = text.trim().to_lowercase();
    if let Ok(seconds) = text.parse::<u64>() {
        return Ok(Duration::from_secs(seconds));
    }
    let mut total = 0u64;
    let mut number = String::new();
    for c in text.chars() {
        if c.is_ascii_digit() {
            number.push(c);
            continue;
        }
        let unit = match c {
            'h' => 3600,
            'm' => 60,
            's' => 1,
            _ => return Err(invalid()),
        };
        total += number.parse::<u64>().map_err(|_| invalid())? * unit;
        number.clear();
    }
    if !number.is_empty() || total == 0 {
        return Err(invalid());
    }
    Ok(Duration::from_secs(total))
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory;

    #[test]
    fn cli_definition_is_valid() {
        Cli::command().debug_assert();
    }

    #[test]
    fn parses_durations() {
        assert_eq!(parse_duration("90"), Ok(Duration::from_secs(90)));
        assert_eq!(parse_duration("45m"), Ok(Duration::from_secs(2700)));
        assert_eq!(parse_duration("1h30m"), Ok(Duration::from_secs(5400)));
        assert_eq!(parse_duration("2m5s"), Ok(Duration::from_secs(125)));
        assert!(parse_duration("1x").is_err());
        assert!(parse_duration("5m3").is_err());
        assert!(parse_duration("").is_err());
    }

    #[test]
    fn effect_flags_accept_an_optional_value() {
        let cli = Cli::try_parse_from(["rekord", "-e", "--denoise=off", "-g", "-6"]).unwrap();
        assert_eq!(cli.enhance, Some(true));
        assert_eq!(cli.denoise, Some(false));
        assert_eq!(cli.gain, Some(-6.0));
    }
}
