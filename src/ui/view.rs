//! Drawing the main screen.
//!
//! ```text
//! ╭ rekord ──────────────────────────────────── ● LIVE ╮
//! │                                                    │
//! │  Mic      Microphone (Anker PowerConf C200)  48 kHz │
//! │  Listen   Speakers (Qudelix-5K)                  on │
//! │                                                    │
//! │  Level    ■■■■■■■■■■■■■■■■■·········│······  -18 dB │
//! │                                                    │
//! │  Gain     +12 dB                                   │
//! │  Enhance  ● on                                     │
//! │  Denoise  ○ off  ▰▰▰▱▱▱▱ 18 dB                     │
//! │ ────────────────────────────────────────────────── │
//! │  ● REC    00:12:34                         228 MB  │
//! │  2026-10-17_22-58-01.wav  in C:\Users\me\Music\... │
//! │ ────────────────────────────────────────────────── │
//! │  ✓ Saved 00:05:12 recording: 2026-10-17_22-40.wav  │
//! ╰────────────────────────────────────────────────────╯
//!     space pause  s stop  e enhance  n denoise ...
//! ```

use super::meter::{self, LevelMeter};
use super::theme;
use crate::app::{App, Severity};
use crate::audio::{Connection, DeviceChoice};
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
/// How long a notice stays on screen.
const NOTICE_TTL: Duration = Duration::from_secs(12);

pub struct ViewState<'a> {
    pub app: &'a App,
    pub meter: &'a LevelMeter,
    /// Toggles twice a second, for the recording indicator.
    pub blink: bool,
}

/// Draws the panel and returns the area it occupies (pop-ups are centered on it).
pub fn draw(frame: &mut Frame, view: &ViewState) -> Rect {
    let area = frame.area();
    let width = area.width.min(MAX_WIDTH);
    // Two columns of margin plus the border and indentation on both sides.
    let inner = width.saturating_sub(2 + 2 * INDENT.len() as u16) as usize;

    let mut lines = Vec::new();
    lines.push(Line::default());
    lines.extend(device_lines(view.app, inner));
    lines.push(Line::default());
    lines.push(level_line(view, inner));
    lines.push(Line::default());
    lines.extend(processing_lines(view.app));
    lines.push(separator(inner));
    lines.extend(recording_lines(view, inner));
    lines.push(separator(inner));
    lines.push(message_line(view.app, inner));

    let footer = key_hints(view.app, width as usize);
    let panel_height = lines.len() as u16 + 2;
    let [panel_area, footer_area] =
        Layout::vertical([Constraint::Length(panel_height), Constraint::Length(footer.len() as u16)])
            .flex(Flex::Center)
            .areas(Layout::horizontal([Constraint::Length(width)]).flex(Flex::Center).areas::<1>(area)[0]);

    let border = match view.app.recording() {
        RecState::Recording => Style::new().fg(theme::BAD),
        RecState::Paused => Style::new().fg(theme::WARN),
        RecState::Idle => theme::border(),
    };
    let block = Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(border)
        .title(Line::from(vec![Span::raw(" "), Span::styled("rekord", theme::title()), Span::raw(" ")]))
        .title(connection_badge(&view.app.connection()).right_aligned());
    frame.render_widget(Paragraph::new(lines).block(block), panel_area);
    frame.render_widget(Paragraph::new(footer).centered(), footer_area);
    panel_area
}

fn device_lines(app: &App, width: usize) -> Vec<Line<'static>> {
    let connection = app.connection();
    let session = match &connection {
        Connection::Live { session, .. } => Some(session),
        _ => None,
    };
    let settings = app.settings();

    let mic = match session {
        Some(s) => Span::styled(s.input.name.clone(), theme::value()),
        None => Span::styled(choice_name(&settings.input), theme::dim()),
    };
    let rate =
        session.map(|s| format!("{:.1} kHz", s.input_rate as f32 / 1000.0).replace(".0 ", " ")).unwrap_or_default();

    let (output, state) = match (session, &settings.output) {
        (_, DeviceChoice::Off) => (Span::styled("Not listening", theme::dim()), Span::styled("off", theme::dim())),
        (Some(s), _) => match &s.output {
            Some(out) if app.monitor() => {
                (Span::styled(out.name.clone(), theme::value()), Span::styled("on", theme::on()))
            }
            Some(out) => {
                (Span::styled(out.name.clone(), theme::value()), Span::styled("muted", Style::new().fg(theme::WARN)))
            }
            None => (Span::styled("Not listening", theme::dim()), Span::styled("off", theme::dim())),
        },
        (None, choice) => (Span::styled(choice_name(choice), theme::dim()), Span::raw("")),
    };

    vec![
        row("Mic", vec![mic], vec![Span::styled(rate, theme::dim())], width),
        row("Listen", vec![output], vec![state], width),
    ]
}

fn level_line(view: &ViewState, width: usize) -> Line<'static> {
    let live = matches!(view.app.connection(), Connection::Live { .. });
    let readout =
        if live { format!("{:>4.0} dB", view.meter.level_db().max(meter::FLOOR_DB)) } else { "   — dB".into() };
    let bar_width = width.saturating_sub(LABEL_WIDTH + readout.len() + 2).max(4);

    let filled = (meter::fraction(view.meter.level_db()) * bar_width as f32).round() as usize;
    let hold = ((meter::fraction(view.meter.hold_db()) * bar_width as f32).round() as usize).min(bar_width);
    let mut spans = Vec::with_capacity(bar_width + 2);
    for i in 0..bar_width {
        // dB at this cell, to color the bar by zone: green, yellow (loud), red (near clipping).
        let cell_db = meter::FLOOR_DB * (1.0 - (i as f32 + 0.5) / bar_width as f32);
        let color = if cell_db > -3.0 {
            theme::BAD
        } else if cell_db > -12.0 {
            theme::WARN
        } else {
            theme::GOOD
        };
        let span = if live && i < filled {
            Span::styled("■", Style::new().fg(color))
        } else if live && hold > filled && i + 1 == hold {
            Span::styled("│", Style::new().fg(color))
        } else {
            Span::styled("·", theme::dim())
        };
        spans.push(span);
    }
    let mut line = vec![Span::raw(INDENT), Span::styled(format!("{:<LABEL_WIDTH$}", "Level"), theme::label())];
    line.extend(spans);
    line.push(Span::styled(format!("  {readout}"), if live { theme::value() } else { theme::dim() }));
    Line::from(line)
}

fn processing_lines(app: &App) -> Vec<Line<'static>> {
    let s = app.settings();
    let level = s.denoise_level();
    let steps: String = (0..DENOISE_LEVELS_DB.len()).map(|i| if i <= level { '▰' } else { '▱' }).collect();
    let strength =
        if level == DENOISE_LEVELS_DB.len() - 1 { "max".to_string() } else { format!("{:.0} dB", s.denoise_db) };
    let strength_style = if s.denoise { Style::new().fg(theme::ACCENT) } else { theme::dim() };

    let mut denoise = toggle(s.denoise);
    denoise.extend([
        Span::raw("   "),
        Span::styled(steps, strength_style),
        Span::styled(format!(" {strength}"), strength_style),
    ]);
    vec![
        labeled("Gain", vec![Span::styled(format::gain(s.gain_db), theme::value())]),
        labeled("Enhance", toggle(s.enhance)),
        labeled("Denoise", denoise),
    ]
}

fn recording_lines(view: &ViewState, width: usize) -> Vec<Line<'static>> {
    let app = view.app;
    let elapsed = format::duration(app.recording_elapsed());
    let size = Span::styled(format::bytes(app.recording_bytes()), theme::dim());
    let file = app.recording_file().and_then(|f| f.file_name().map(|n| n.to_string_lossy().into_owned()));
    let dir = format::path(app.recordings_dir());

    let (status, detail) = match app.recording() {
        RecState::Idle => (
            row_spans(
                vec![Span::styled("○ ", theme::dim()), Span::styled("Not recording", theme::dim())],
                vec![Span::styled("space", theme::key()), Span::styled(" to record", theme::dim())],
                width,
            ),
            vec![Span::styled("Saves to ", theme::dim()), Span::raw(dir)],
        ),
        RecState::Recording => (
            row_spans(
                vec![
                    Span::styled(if view.blink { "● " } else { "  " }, Style::new().fg(theme::BAD)),
                    Span::styled(format!("{:<7}", "REC"), Style::new().fg(theme::BAD).bold()),
                    Span::styled(elapsed, theme::value()),
                ],
                vec![size],
                width,
            ),
            vec![Span::raw(file.unwrap_or_default()), Span::styled(format!("  in {dir}"), theme::dim())],
        ),
        RecState::Paused => (
            row_spans(
                vec![
                    Span::styled("❚❚ ", Style::new().fg(theme::WARN)),
                    Span::styled(format!("{:<6}", "PAUSED"), Style::new().fg(theme::WARN).bold()),
                    Span::styled(elapsed, theme::value()),
                ],
                vec![size],
                width,
            ),
            vec![Span::raw(file.unwrap_or_default()), Span::styled(format!("  in {dir}"), theme::dim())],
        ),
    };
    vec![status, truncate(detail, width)]
}

/// The most relevant message: an error or a recent notice, then connection problems.
fn message_line(app: &App, width: usize) -> Line<'static> {
    let notice = app.notices().last().filter(|n| n.at.elapsed() < NOTICE_TTL || n.severity == Severity::Error);
    let (icon, text, color) = match (notice, app.connection()) {
        (Some(n), _) if n.severity == Severity::Error => ("✕", n.text.clone(), theme::BAD),
        (_, Connection::Waiting(reason)) => ("✕", reason, theme::BAD),
        (Some(n), _) => match n.severity {
            Severity::Success => ("✓", n.text.clone(), theme::GOOD),
            _ => ("!", n.text.clone(), theme::WARN),
        },
        (None, Connection::Live { warnings, .. }) if !warnings.is_empty() => ("!", warnings.join(" · "), theme::WARN),
        (None, Connection::Connecting) => ("◌", "Connecting to your devices…".into(), theme::WARN),
        _ => {
            let dropouts = app.dropouts();
            if dropouts == 0 {
                return Line::styled(format!("{INDENT}Ready"), theme::dim());
            }
            ("·", format!("{dropouts} audio dropout{}", if dropouts == 1 { "" } else { "s" }), theme::MUTED)
        }
    };
    truncate(
        vec![
            Span::styled(format!("{icon} "), Style::new().fg(color).bold()),
            Span::styled(text, Style::new().fg(color)),
        ],
        width,
    )
}

fn key_hints(app: &App, width: usize) -> Vec<Line<'static>> {
    let record = match app.recording() {
        RecState::Idle => "record",
        RecState::Recording => "pause",
        RecState::Paused => "resume",
    };
    let mut groups = vec![("space", record)];
    if app.recording() != RecState::Idle {
        groups.push(("s", "stop"));
    }
    groups.extend([("e", "enhance"), ("n", "denoise"), ("[ ]", "strength")]);
    let second = [
        ("↑↓", "gain"),
        ("m", if app.monitor() { "mute" } else { "unmute" }),
        ("i", "mic"),
        ("o", "output"),
        ("q", "quit"),
    ];

    let render = |items: &[(&'static str, &'static str)]| {
        let mut spans = Vec::new();
        for (i, (key, action)) in items.iter().enumerate() {
            if i > 0 {
                spans.push(Span::styled("   ", theme::dim()));
            }
            spans.push(Span::styled(*key, theme::key()));
            spans.push(Span::styled(format!(" {action}"), theme::dim()));
        }
        Line::from(spans)
    };
    let all: Vec<_> = groups.iter().chain(second.iter()).copied().collect();
    let single = render(&all);
    if single.width() <= width { vec![single] } else { vec![render(&groups), render(&second)] }
}

fn connection_badge(connection: &Connection) -> Line<'static> {
    let (text, color) = match connection {
        Connection::Live { .. } => (" ● LIVE ", theme::GOOD),
        Connection::Connecting => (" ◌ CONNECTING ", theme::WARN),
        Connection::Waiting(_) => (" ✕ OFFLINE ", theme::BAD),
    };
    Line::styled(text, Style::new().fg(color).bold())
}

fn toggle(on: bool) -> Vec<Span<'static>> {
    if on {
        vec![Span::styled("● ", theme::on()), Span::styled("on ", theme::on())]
    } else {
        vec![Span::styled("○ ", theme::dim()), Span::styled("off", theme::dim())]
    }
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
        let room = width.saturating_sub(used + 1);
        let cut: String = span.content.chars().take(room).collect();
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

/// Whether the recording indicator is lit at `now`.
pub fn blink_phase(start: Instant) -> bool {
    (start.elapsed().as_millis() / 500).is_multiple_of(2)
}
