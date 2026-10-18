//! Application state and behavior, shared by the interactive and the plain front ends.

use crate::audio::{Connection, DeviceChoice, Engine};
use crate::recording::{RecEvent, RecState, Recorder, Take};
use crate::settings::{Settings, SettingsFile};
use anyhow::Result;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// Everything the user can ask the app to do.
#[derive(Clone, Debug, PartialEq)]
pub enum Action {
    /// Start a recording, or pause / resume the current one.
    ToggleRecording,
    StopRecording,
    ToggleEnhance,
    ToggleDenoise,
    ChangeDenoiseLevel(i32),
    ChangeGain(i32),
    ToggleMonitor,
    SetInput(DeviceChoice),
    SetOutput(DeviceChoice),
    Quit,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Severity {
    Success,
    Warning,
    Error,
}

#[derive(Clone, Debug)]
pub struct Notice {
    pub text: String,
    pub severity: Severity,
    pub at: Instant,
}

/// What the front ends display, captured at one instant.
#[derive(Clone, Debug)]
pub struct Snapshot {
    pub connection: Connection,
    pub settings: Settings,
    pub monitor: bool,
    pub recording: RecordingStatus,
    /// The most recent notice, however old.
    pub notice: Option<Notice>,
    pub dropouts: u32,
}

#[derive(Clone, Debug)]
pub struct RecordingStatus {
    pub state: RecState,
    /// Length of the current take, pauses excluded.
    pub elapsed: Duration,
    pub bytes: u64,
    pub file: Option<PathBuf>,
    pub dir: PathBuf,
}

/// How the app was configured at startup.
pub struct Startup {
    pub settings: Settings,
    /// The settings as stored on disk (without one-off command-line overrides).
    pub saved: Settings,
    pub settings_file: SettingsFile,
    pub recordings_dir: PathBuf,
    pub record: bool,
    pub notices: Vec<(Severity, String)>,
}

pub struct App {
    engine: Engine,
    recorder: Recorder,
    settings: Settings,
    saved: Settings,
    settings_file: SettingsFile,
    monitor: bool,
    notices: Vec<Notice>,
    takes: Vec<Take>,
    /// Recording errors, repeated in the summary on exit.
    errors: Vec<String>,
    quit: bool,
}

impl App {
    pub fn start(startup: Startup) -> Result<Self> {
        let recorder = Recorder::new(startup.recordings_dir)?;
        let settings = startup.settings;
        let monitor = settings.output != DeviceChoice::Off;
        let engine = Engine::start(
            settings.input.clone(),
            settings.output.clone(),
            &settings.params(),
            monitor,
            recorder.tap(),
        )?;
        if startup.record {
            recorder.start();
        }
        let mut app = Self {
            engine,
            recorder,
            settings,
            saved: startup.saved,
            settings_file: startup.settings_file,
            monitor,
            notices: Vec::new(),
            takes: Vec::new(),
            errors: Vec::new(),
            quit: false,
        };
        for (severity, text) in startup.notices {
            app.notify(severity, text);
        }
        Ok(app)
    }

    pub fn apply(&mut self, action: Action) {
        match action {
            Action::ToggleRecording => self.recorder.toggle(),
            Action::StopRecording => self.recorder.stop(),
            Action::ToggleEnhance => self.update(|s| s.enhance = !s.enhance),
            Action::ToggleDenoise => self.update(|s| s.denoise = !s.denoise),
            Action::ChangeDenoiseLevel(steps) => self.update(|s| {
                s.denoise = true;
                s.change_denoise_level(steps);
            }),
            Action::ChangeGain(steps) => self.update(|s| s.change_gain(steps)),
            Action::ToggleMonitor if self.settings.output == DeviceChoice::Off => {
                self.notify(Severity::Warning, "Not listening on any output: press o to choose one");
            }
            Action::ToggleMonitor => {
                self.monitor = !self.monitor;
                self.engine.controls().set_monitor(self.monitor);
            }
            Action::SetInput(choice) => {
                self.engine.set_input(choice.clone());
                self.update(|s| s.input = choice);
            }
            Action::SetOutput(choice) => {
                self.monitor = choice != DeviceChoice::Off;
                self.engine.controls().set_monitor(self.monitor);
                self.engine.set_output(choice.clone());
                self.update(|s| s.output = choice);
            }
            Action::Quit => self.quit = true,
        }
    }

    /// Collects what happened in the background since the last call.
    pub fn tick(&mut self) {
        let events: Vec<RecEvent> = self.recorder.events().collect();
        for event in events {
            self.handle(event);
        }
    }

    /// Stops everything, closing any recording in progress.
    /// Returns the takes saved and the recording errors that happened during the run.
    pub fn finish(self) -> (Vec<Take>, Vec<String>) {
        let Self { engine, recorder, mut takes, mut errors, .. } = self;
        drop(engine); // stop feeding audio first
        for event in recorder.shutdown() {
            match event {
                RecEvent::Saved(take) => takes.push(take),
                RecEvent::Failed(message) => errors.push(message),
            }
        }
        (takes, errors)
    }

    pub fn should_quit(&self) -> bool {
        self.quit
    }

    /// A consistent picture of the current state, for display.
    pub fn snapshot(&self) -> Snapshot {
        Snapshot {
            connection: self.engine.connection(),
            settings: self.settings.clone(),
            monitor: self.monitor,
            recording: RecordingStatus {
                state: self.recorder.state(),
                elapsed: self.recorder.elapsed(),
                bytes: self.recorder.bytes(),
                file: self.recorder.current_file(),
                dir: self.recorder.dir().to_owned(),
            },
            notice: self.notices.last().cloned(),
            dropouts: self.engine.meters().dropouts(),
        }
    }

    /// Highest output level since the last call (linear).
    pub fn take_peak(&self) -> f32 {
        self.engine.meters().take_peak()
    }

    pub fn recordings_dir(&self) -> &Path {
        self.recorder.dir()
    }

    pub fn notices(&self) -> &[Notice] {
        &self.notices
    }

    pub fn notify(&mut self, severity: Severity, text: impl Into<String>) {
        const KEEP: usize = 20;
        self.notices.push(Notice { text: text.into(), severity, at: Instant::now() });
        if self.notices.len() > KEEP {
            self.notices.remove(0);
        }
    }

    fn handle(&mut self, event: RecEvent) {
        match event {
            RecEvent::Saved(take) => {
                let name = take.files.first().and_then(|f| f.file_name()).map(|n| n.to_string_lossy().into_owned());
                self.notify(
                    Severity::Success,
                    format!("Saved {} recording: {}", crate::format::duration(take.duration), name.unwrap_or_default()),
                );
                self.takes.push(take);
            }
            RecEvent::Failed(message) => {
                self.notify(Severity::Error, message.clone());
                self.errors.push(message);
            }
        }
    }

    /// Changes a setting for this run and remembers it for the next one.
    fn update(&mut self, change: impl FnOnce(&mut Settings)) {
        let before = self.settings.clone();
        change(&mut self.settings);
        // Only what the user just changed is persisted, not one-off command-line overrides.
        let (now, saved) = (&self.settings, &mut self.saved);
        if now.input != before.input {
            saved.input = now.input.clone();
        }
        if now.output != before.output {
            saved.output = now.output.clone();
        }
        if now.gain_db != before.gain_db {
            saved.gain_db = now.gain_db;
        }
        if now.enhance != before.enhance {
            saved.enhance = now.enhance;
        }
        if now.denoise != before.denoise {
            saved.denoise = now.denoise;
        }
        if now.denoise_db != before.denoise_db {
            saved.denoise_db = now.denoise_db;
        }
        self.engine.controls().set_params(&self.settings.params());
        if let Err(e) = self.settings_file.save(&self.saved) {
            self.notify(Severity::Warning, format!("Settings not saved: {e:#}"));
        }
    }
}
