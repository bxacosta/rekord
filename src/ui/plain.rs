//! Line-based front end for when the output is not a terminal.
//!
//! Prints status changes and a short report every few seconds. Commands can be
//! sent one per line on standard input: `record`, `pause`, `resume`, `stop`,
//! `enhance`, `denoise`, `mute`, `quit`.

use crate::app::{Action, App, Severity, Snapshot};
use crate::audio::Connection;
use crate::dsp::dynamics::gain_to_db;
use crate::format;
use crate::recording::RecState;
use crate::shutdown;
use anyhow::Result;
use std::io::BufRead;
use std::sync::mpsc::{Receiver, channel};
use std::time::{Duration, Instant};

const REPORT_EVERY: Duration = Duration::from_secs(5);
const POLL: Duration = Duration::from_millis(50);

pub fn run(app: &mut App, deadline: Option<Instant>) -> Result<()> {
    let commands = read_stdin();
    let mut printed_until = Instant::now() - Duration::from_secs(3600);
    let mut last: Option<Snapshot> = None;
    let mut next_report = Instant::now() + REPORT_EVERY;
    let mut peak = 0f32;

    loop {
        app.tick();
        for line in commands.try_iter() {
            match parse_command(&line, app.snapshot().recording.state) {
                Ok(Some(action)) => app.apply(action),
                Ok(None) => {}
                Err(message) => log(&message),
            }
        }

        let since = printed_until;
        for notice in app.notices().iter().filter(|n| n.at > since) {
            let prefix = match notice.severity {
                Severity::Error => "error: ",
                Severity::Warning => "warning: ",
                Severity::Success => "",
            };
            log(&format!("{prefix}{}", notice.text));
            printed_until = notice.at;
        }

        let now = app.snapshot();
        if last.as_ref().is_none_or(|l| l.connection != now.connection) {
            log(&describe(&now.connection));
        }
        let previous_state = last.as_ref().map_or(RecState::Idle, |l| l.recording.state);
        if now.recording.state != previous_state {
            log(&recording_change(previous_state, &now));
        }

        peak = peak.max(app.take_peak());
        if Instant::now() >= next_report {
            next_report = Instant::now() + REPORT_EVERY;
            log(&report(&now, peak));
            peak = 0.0;
        }
        last = Some(now);

        if app.should_quit() || shutdown::requested() || deadline.is_some_and(|d| Instant::now() >= d) {
            return Ok(());
        }
        std::thread::sleep(POLL);
    }
}

fn parse_command(line: &str, state: RecState) -> Result<Option<Action>, String> {
    let action = match line.trim().to_lowercase().as_str() {
        "" => return Ok(None),
        "record" | "r" => (state == RecState::Idle).then_some(Action::ToggleRecording),
        "pause" | "p" => (state == RecState::Recording).then_some(Action::ToggleRecording),
        "resume" => (state == RecState::Paused).then_some(Action::ToggleRecording),
        "stop" | "s" => Some(Action::StopRecording),
        "enhance" | "e" => Some(Action::ToggleEnhance),
        "denoise" | "n" => Some(Action::ToggleDenoise),
        "mute" | "m" => Some(Action::ToggleMonitor),
        "quit" | "q" | "exit" => Some(Action::Quit),
        other => {
            return Err(format!(
                "Unknown command \"{other}\" (record, pause, resume, stop, enhance, denoise, mute, quit)"
            ));
        }
    };
    Ok(action)
}

fn describe(connection: &Connection) -> String {
    match connection {
        Connection::Connecting => "Connecting…".into(),
        Connection::Waiting(reason) => format!("Offline: {reason}"),
        Connection::Live { session, warnings } => {
            let output = session.output.as_ref().map_or("not listening".to_string(), |o| o.name.clone());
            let mut text = format!("Live: {} ({} Hz) → {output}", session.input.name, session.input_rate);
            for warning in warnings {
                text += &format!("\n           warning: {warning}");
            }
            text
        }
    }
}

fn recording_change(previous: RecState, now: &Snapshot) -> String {
    let rec = &now.recording;
    match rec.state {
        RecState::Recording if previous == RecState::Paused => "Recording resumed".into(),
        RecState::Recording => format!("Recording to {}", rec.file.as_deref().map(format::path).unwrap_or_default()),
        RecState::Paused => format!("Recording paused at {}", format::duration(rec.elapsed)),
        RecState::Idle => "Recording stopped".into(),
    }
}

fn report(s: &Snapshot, peak: f32) -> String {
    let on_off = |on: bool| if on { "on" } else { "off" };
    let rec = &s.recording;
    let recording = match rec.state {
        RecState::Idle => "not recording".to_string(),
        RecState::Recording => format!("REC {} ({})", format::duration(rec.elapsed), format::bytes(rec.bytes)),
        RecState::Paused => format!("PAUSED {}", format::duration(rec.elapsed)),
    };
    format!(
        "level {:>4.0} dB · gain {} · enhance {} · denoise {} · {recording}",
        gain_to_db(peak).max(-60.0),
        format::gain(s.settings.gain_db),
        on_off(s.settings.enhance),
        on_off(s.settings.denoise),
    )
}

fn log(message: &str) {
    println!("[{}] {message}", chrono::Local::now().format("%H:%M:%S"));
}

/// Lines from standard input, read on a background thread. Ends quietly at EOF.
fn read_stdin() -> Receiver<String> {
    let (tx, rx) = channel();
    std::thread::spawn(move || {
        for line in std::io::stdin().lock().lines().map_while(Result::ok) {
            if tx.send(line).is_err() {
                break;
            }
        }
    });
    rx
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recording_commands_respect_the_current_state() {
        assert_eq!(parse_command("record", RecState::Idle), Ok(Some(Action::ToggleRecording)));
        assert_eq!(parse_command("record", RecState::Recording), Ok(None));
        assert_eq!(parse_command(" Pause ", RecState::Recording), Ok(Some(Action::ToggleRecording)));
        assert_eq!(parse_command("pause", RecState::Paused), Ok(None));
        assert_eq!(parse_command("resume", RecState::Paused), Ok(Some(Action::ToggleRecording)));
        assert!(parse_command("dance", RecState::Idle).is_err());
    }
}
