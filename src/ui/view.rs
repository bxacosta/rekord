//! Drawing the main screen.
//!
//! ```text
//! ╭ rekord ───────────────────────────────────── ● LIVE ╮
//! │                                                     │
//! │  Mic      Microphone (Anker PowerConf C200)   48 kHz │
//! │  Listen   Speakers (Qudelix-5K)                   on │
//! │                                                     │
//! │  Level    ■■■■■■■■■■■■■■■■■■■········│·······  -18 dB │
//! │                                                     │
//! │  Gain     +12 dB                                    │
//! │  Enhance  ● on                                      │
//! │  Denoise  ○ off   ▰▰▰▱▱▱▱ 18 dB                     │
//! │ ─────────────────────────────────────────────────── │
//! │  ● REC    00:12:34                         142.1 MB │
//! │  2026-10-17_22-58-01.wav  in C:\Users\me\Music\R... │
//! │ ─────────────────────────────────────────────────── │
//! │  ✓ Saved 00:05:12 recording: 2026-10-17_22-40.wav   │
//! ╰─────────────────────────────────────────────────────╯
//!     space pause   s stop   e enhance   n denoise ...
//! ```

use super::meter::{self, LevelMeter};
use super::theme;
use crate::app::{Severity, Snapshot};
use crate::audio::{Connection, DeviceChoice, Dropouts};
use crate::format;
use crate::recording::RecState;
use crate::settings::DENOISE_LEVELS_DB;
use ratatui::Frame;
use ratatui::layout::{Constraint, Flex, Layout, Rect};
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Paragraph};
use std::time::{Duration, Instant};

const MAX_WIDTH: u16 = 78;
const LABEL_WIDTH: usize = 9;
/// Indentation inside the panel.
const INDENT: &str = "  ";
/// How long a notice stays on screen (errors stay until replaced).
const NOTICE_TTL: Duration = Duration::from_secs(12);

pub struct ViewState<'a> {
    pub snapshot: &'a Snapshot,
    pub meter: &'a LevelMeter,
    /// Toggles twice a second, for the recording indicator.
    pub blink: bool,
}

/// Draws the panel and returns the area it occupies (pop-ups are centered on it).
pub fn draw(frame: &mut Frame, view: &ViewState) -> Rect {
    let s = view.snapshot;
    let area = frame.area();
    let width = area.width.min(MAX_WIDTH);
    // Content width: minus the borders and the indentation on both sides.
    let inner = width.saturating_sub(2 + 2 * INDENT.len() as u16) as usize;

    let mut lines = vec![Line::default()];
    lines.extend(device_lines(s, inner));
    lines.push(Line::default());
    lines.push(level_line(s, view.meter, inner));
    lines.push(Line::default());
    lines.extend(processing_lines(s));
    lines.push(separator(inner));
    lines.extend(recording_lines(s, view.blink, inner));
    lines.push(separator(inner));
    lines.push(message_line(s, inner));

    let footer = key_hints(s, width as usize);
    let [column] = Layout::horizontal([Constraint::Length(width)]).flex(Flex::Center).areas(area);
    let [panel_area, footer_area] =
        Layout::vertical([Constraint::Length(lines.len() as u16 + 2), Constraint::Length(footer.len() as u16 + 1)])
            .flex(Flex::Center)
            .areas(column);

    let block = Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(theme::border())
        .title(Line::from(vec![Span::raw(" "), Span::styled("rekord", theme::title()), Span::raw(" ")]))
        .title(connection_badge(&s.connection).right_aligned());
    frame.render_widget(Paragraph::new(lines).block(block), panel_area);

    let mut footer_lines = vec![Line::default()];
    footer_lines.extend(footer);
    frame.render_widget(Paragraph::new(footer_lines).centered(), footer_area);
    panel_area
}

fn device_lines(s: &Snapshot, width: usize) -> Vec<Line<'static>> {
    let session = match &s.connection {
        Connection::Live { session, .. } => Some(session),
        _ => None,
    };

    let mic = match session {
        Some(session) => Span::styled(session.input.name.clone(), theme::value()),
        None => Span::styled(choice_name(&s.settings.input), theme::dim()),
    };
    let rate = session.map(|session| sample_rate(session.input_rate)).unwrap_or_default();

    let not_listening = || (Span::styled("Not listening", theme::dim()), Span::styled("off", theme::dim()));
    let (output, state) = match (session, &s.settings.output) {
        (_, DeviceChoice::Off) => not_listening(),
        (Some(session), _) => match &session.output {
            Some(out) if s.monitor => (Span::styled(out.name.clone(), theme::value()), Span::styled("on", theme::on())),
            Some(out) => (Span::styled(out.name.clone(), theme::value()), Span::styled("muted", theme::warn())),
            None => not_listening(),
        },
        (None, choice) => (Span::styled(choice_name(choice), theme::dim()), Span::raw("")),
    };

    vec![
        row("Mic", vec![mic], vec![Span::styled(rate, theme::dim())], width),
        row("Listen", vec![output], vec![state], width),
    ]
}

fn level_line(s: &Snapshot, meter: &LevelMeter, width: usize) -> Line<'static> {
    let live = matches!(s.connection, Connection::Live { .. });
    let readout = if live { format!("{:>4.0} dB", meter.level_db()) } else { "   — dB".into() };
    let bar_width = width.saturating_sub(LABEL_WIDTH + readout.chars().count() + 2).max(4);

    let filled = (meter::fraction(meter.level_db()) * bar_width as f32).round() as usize;
    let hold = ((meter::fraction(meter.hold_db()) * bar_width as f32).round() as usize).min(bar_width);
    let mut spans = vec![Span::raw(INDENT), Span::styled(format!("{:<LABEL_WIDTH$}", "Level"), theme::label())];
    for i in 0..bar_width {
        // Level at this cell picks its zone color: green, yellow (loud), red (near clipping).
        let cell_db = meter::FLOOR_DB * (1.0 - (i as f32 + 0.5) / bar_width as f32);
        let color = if cell_db > -3.0 {
            theme::BAD
        } else if cell_db > -12.0 {
            theme::WARN
        } else {
            theme::GOOD
        };
        spans.push(if live && i < filled {
            Span::styled("■", Style::new().fg(color))
        } else if live && hold > filled && i + 1 == hold {
            Span::styled("│", Style::new().fg(color))
        } else {
            Span::styled("·", theme::dim())
        });
    }
    spans.push(Span::styled(format!("  {readout}"), if live { theme::value() } else { theme::dim() }));
    Line::from(spans)
}

fn processing_lines(s: &Snapshot) -> Vec<Line<'static>> {
    let settings = &s.settings;
    let level = settings.denoise_level();
    let max_level = DENOISE_LEVELS_DB.len() - 1;
    let steps: String = (0..=max_level).map(|i| if i <= level { '▰' } else { '▱' }).collect();
    let strength = if level == max_level { "max".to_string() } else { format!("{:.0} dB", settings.denoise_db) };
    let strength_style = if settings.denoise { theme::accent() } else { theme::dim() };

    let mut denoise = toggle(settings.denoise);
    denoise.extend([
        Span::raw("   "),
        Span::styled(steps, strength_style),
        Span::styled(format!(" {strength}"), strength_style),
    ]);
    vec![
        labeled("Gain", vec![Span::styled(format::gain(settings.gain_db), theme::value())]),
        labeled("Enhance", toggle(settings.enhance)),
        labeled("Denoise", denoise),
    ]
}

fn recording_lines(s: &Snapshot, blink: bool, width: usize) -> Vec<Line<'static>> {
    let rec = &s.recording;
    let dir = format::path(&rec.dir);
    let file = rec.file.as_ref().and_then(|f| f.file_name()).map(|n| n.to_string_lossy().into_owned());
    let elapsed = Span::styled(format::duration(rec.elapsed), theme::value());
    let size = vec![Span::styled(format::bytes(rec.bytes), theme::dim())];
    let location = vec![Span::raw(file.unwrap_or_default()), Span::styled(format!("  in {dir}"), theme::dim())];

    let (status, detail) = match rec.state {
        RecState::Idle => (
            row_spans(
                vec![Span::styled("○ Not recording", theme::dim())],
                vec![Span::styled("space", theme::key()), Span::styled(" to start", theme::dim())],
                width,
            ),
            vec![Span::styled("Saves to ", theme::dim()), Span::raw(dir)],
        ),
        RecState::Recording => (
            row_spans(
                vec![
                    Span::styled(if blink { "● " } else { "  " }, theme::bad()),
                    Span::styled(format!("{:<8}", "REC"), theme::bad().bold()),
                    elapsed,
                ],
                size,
                width,
            ),
            location,
        ),
        RecState::Paused => (
            row_spans(
                vec![
                    Span::styled("❚❚ ", theme::warn()),
                    Span::styled(format!("{:<7}", "PAUSED"), theme::warn().bold()),
                    elapsed,
                ],
                size,
                width,
            ),
            location,
        ),
    };
    vec![status, truncate(detail, width)]
}

/// The most relevant message: an error or a recent notice, then the connection state.
fn message_line(s: &Snapshot, width: usize) -> Line<'static> {
    let notice = s.notice.as_ref().filter(|n| n.at.elapsed() < NOTICE_TTL || n.severity == Severity::Error);
    let (icon, text, color) = match (notice, &s.connection) {
        (Some(n), _) if n.severity == Severity::Error => ("✕", n.text.clone(), theme::BAD),
        (_, Connection::Waiting(reason)) => ("✕", reason.clone(), theme::BAD),
        (Some(n), _) if n.severity == Severity::Success => ("✓", n.text.clone(), theme::GOOD),
        (Some(n), _) => ("!", n.text.clone(), theme::WARN),
        (None, Connection::Live { warnings, .. }) if !warnings.is_empty() => ("!", warnings.join(" · "), theme::WARN),
        (None, Connection::Connecting) => ("◌", "Connecting to your devices…".into(), theme::WARN),
        _ if s.dropouts != Dropouts::default() => ("·", dropouts_text(s.dropouts), theme::MUTED),
        _ => ("·", "Ready".into(), theme::MUTED),
    };
    let style = Style::new().fg(color);
    truncate(vec![Span::styled(format!("{icon} "), style.bold()), Span::styled(text, style)], width)
}

type Hint = (&'static str, &'static str);

fn key_hints(s: &Snapshot, width: usize) -> Vec<Line<'static>> {
    let recording = s.recording.state;
    let record = match recording {
        RecState::Idle => "record",
        RecState::Recording => "pause",
        RecState::Paused => "resume",
    };
    // Two natural groups: recording and effects, then levels, devices and quitting.
    let mut first: Vec<Hint> = vec![("space", record)];
    if recording != RecState::Idle {
        first.push(("s", "stop"));
    }
    first.extend([("e", "enhance"), ("n", "denoise"), ("[ ]", "strength")]);
    let mut second: Vec<Hint> = vec![("↑↓", "gain")];
    if s.settings.output != DeviceChoice::Off {
        second.push(("m", if s.monitor { "mute" } else { "unmute" }));
    }
    second.extend([("i", "mic"), ("o", "output"), ("q", "quit")]);

    let all = [first.as_slice(), second.as_slice()].concat();
    let fits = |hints: &[Hint]| hint_lines(hints, usize::MAX)[0].width() <= width;
    if fits(&all) {
        hint_lines(&all, width)
    } else if fits(&first) && fits(&second) {
        [hint_lines(&first, width), hint_lines(&second, width)].concat()
    } else {
        hint_lines(&all, width)
    }
}

/// Lays hints out in as few lines of `width` columns as possible, never splitting one.
fn hint_lines(hints: &[Hint], width: usize) -> Vec<Line<'static>> {
    const GAP: &str = "   ";
    let mut lines = vec![Line::default()];
    for &(key, action) in hints {
        let hint = [Span::styled(key, theme::key()), Span::styled(format!(" {action}"), theme::dim())];
        let hint_width: usize = hint.iter().map(Span::width).sum();
        let line = lines.last_mut().expect("there is always a line");
        if line.width() > 0 && line.width() + GAP.len() + hint_width > width {
            lines.push(Line::from(hint.to_vec()));
        } else {
            if line.width() > 0 {
                line.spans.push(Span::raw(GAP));
            }
            line.spans.extend(hint);
        }
    }
    lines
}

fn connection_badge(connection: &Connection) -> Line<'static> {
    let (text, style) = match connection {
        Connection::Live { .. } => (" ● LIVE ", theme::on()),
        Connection::Connecting => (" ◌ CONNECTING ", theme::warn().bold()),
        Connection::Waiting(_) => (" ✕ OFFLINE ", theme::bad().bold()),
    };
    Line::styled(text, style)
}

fn toggle(on: bool) -> Vec<Span<'static>> {
    if on { vec![Span::styled("● on ", theme::on())] } else { vec![Span::styled("○ off", theme::dim())] }
}

fn labeled(label: &str, value: Vec<Span<'static>>) -> Line<'static> {
    let mut spans = vec![Span::raw(INDENT), Span::styled(format!("{label:<LABEL_WIDTH$}"), theme::label())];
    spans.extend(value);
    Line::from(spans)
}

/// A labeled line with `right` pushed to the right edge.
fn row(label: &str, left: Vec<Span<'static>>, right: Vec<Span<'static>>, width: usize) -> Line<'static> {
    let mut spans = vec![Span::styled(format!("{label:<LABEL_WIDTH$}"), theme::label())];
    spans.extend(left);
    row_spans(spans, right, width)
}

fn row_spans(left: Vec<Span<'static>>, right: Vec<Span<'static>>, width: usize) -> Line<'static> {
    let right_width: usize = right.iter().map(Span::width).sum();
    let mut line = truncate(left, width.saturating_sub(right_width + 1));
    let used = line.width() - INDENT.len();
    line.spans.push(Span::raw(" ".repeat(width.saturating_sub(used + right_width))));
    line.spans.extend(right);
    line
}

/// Indents `spans` and cuts them to `width` columns, ending in "…" when shortened.
fn truncate(spans: Vec<Span<'static>>, width: usize) -> Line<'static> {
    let mut out = vec![Span::raw(INDENT)];
    let mut used = 0;
    for span in spans {
        let w = span.width();
        if used + w <= width {
            used += w;
            out.push(span);
            continue;
        }
        let cut: String = span.content.chars().take(width.saturating_sub(used + 1)).collect();
        out.push(Span::styled(format!("{cut}…"), span.style));
        break;
    }
    Line::from(out)
}

fn separator(width: usize) -> Line<'static> {
    Line::styled(format!(" {}", "─".repeat(width + 2)), theme::border())
}

fn choice_name(choice: &DeviceChoice) -> String {
    match choice {
        DeviceChoice::Default => "System default".into(),
        DeviceChoice::Device { name, .. } => name.clone(),
        DeviceChoice::Off => "Off".into(),
    }
}

/// `Dropouts: 3 mic (in recordings), 12 output (only what you hear)`
fn dropouts_text(d: Dropouts) -> String {
    let mut parts = Vec::new();
    if d.input > 0 {
        parts.push(format!("{} mic (in recordings)", d.input));
    }
    if d.output > 0 {
        parts.push(format!("{} output (only what you hear)", d.output));
    }
    format!("Dropouts: {}", parts.join(", "))
}

/// `48 kHz`, `44.1 kHz`
fn sample_rate(hz: u32) -> String {
    if hz.is_multiple_of(1000) { format!("{} kHz", hz / 1000) } else { format!("{:.1} kHz", hz as f32 / 1000.0) }
}

/// Whether the recording indicator is lit, blinking twice a second since `start`.
pub fn blink_phase(start: Instant) -> bool {
    (start.elapsed().as_millis() / 500).is_multiple_of(2)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::{Notice, RecordingStatus};
    use crate::audio::devices::DeviceInfo;
    use crate::audio::session::SessionInfo;
    use crate::settings::Settings;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use std::path::PathBuf;

    fn snapshot() -> Snapshot {
        Snapshot {
            connection: Connection::Live {
                session: SessionInfo {
                    input: DeviceInfo { id: "in".into(), name: "Microphone (Anker PowerConf C200)".into() },
                    input_rate: 48_000,
                    output: Some(DeviceInfo { id: "out".into(), name: "Speakers (Qudelix-5K USB DAC 96KHz)".into() }),
                },
                warnings: vec![],
            },
            settings: Settings { enhance: true, ..Settings::default() },
            monitor: true,
            recording: RecordingStatus {
                state: RecState::Recording,
                elapsed: Duration::from_secs(754),
                bytes: 145_000_000,
                file: Some(PathBuf::from("2026-10-17_22-58-01.wav")),
                dir: PathBuf::from(r"C:\Users\me\Music\Rekord"),
            },
            notice: Some(Notice {
                text: "Recovered interrupted recording".into(),
                severity: Severity::Success,
                at: Instant::now(),
            }),
            dropouts: Dropouts::default(),
        }
    }

    fn render(snapshot: &Snapshot, width: u16, height: u16) -> String {
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        let mut meter = LevelMeter::new();
        meter.update(0.12);
        terminal
            .draw(|frame| {
                draw(frame, &ViewState { snapshot, meter: &meter, blink: true });
            })
            .unwrap();
        let buffer = terminal.backend().buffer();
        (0..height)
            .map(|y| (0..width).map(|x| buffer[(x, y)].symbol()).collect::<String>().trim_end().to_owned())
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn main_screen_shows_every_section() {
        let screen = render(&snapshot(), 90, 24);
        println!("{screen}");
        for expected in [
            "rekord",
            "● LIVE",
            "Microphone (Anker PowerConf C200)",
            "48 kHz",
            "Speakers (Qudelix-5K USB DAC 96KHz)",
            "-18 dB",
            "+12 dB",
            "● on",
            "REC",
            "00:12:34",
            "138.3 MB",
            "2026-10-17_22-58-01.wav",
            "✓ Recovered interrupted recording",
            "space pause",
        ] {
            assert!(screen.contains(expected), "missing {expected:?}");
        }
    }

    #[test]
    fn narrow_terminals_truncate_instead_of_overflowing() {
        let mut s = snapshot();
        s.recording.state = RecState::Paused;
        s.connection = Connection::Waiting("The microphone stopped sending audio. Reconnecting…".into());
        let screen = render(&s, 44, 26);
        println!("{screen}");
        assert!(screen.contains("PAUSED"));
        assert!(screen.contains("OFFLINE"));
        assert!(screen.lines().all(|line| line.chars().count() <= 44));
    }
}
