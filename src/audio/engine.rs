//! Keeps a session running: opens the chosen devices, follows the system default,
//! and reconnects automatically when a device disappears, comes back or stalls.
//!
//! All device work happens on the engine's own thread, so the UI never blocks on it.

use super::controls::{Controls, Meters};
use super::devices::{self, DeviceChoice, Direction, Resolved};
use super::session::{Context, Session, SessionInfo};
use crate::dsp::Params;
use crate::recording::Tap;
use anyhow::Result;
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender, channel};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

/// How often to look for device changes (unplugged, back, new default).
const RECHECK_EVERY: Duration = Duration::from_secs(2);
/// Pause before reconnecting after a failure.
const RETRY_AFTER: Duration = Duration::from_secs(2);
/// How often session health is checked.
const POLL_EVERY: Duration = Duration::from_millis(100);

#[derive(Clone, Debug, PartialEq)]
pub enum Connection {
    Connecting,
    Live {
        session: SessionInfo,
        warnings: Vec<String>,
    },
    /// Not connected; the reason is shown while the engine keeps retrying.
    Waiting(String),
}

enum Command {
    SetInput(DeviceChoice),
    SetOutput(DeviceChoice),
    Shutdown,
}

pub struct Engine {
    commands: Sender<Command>,
    controls: Arc<Controls>,
    meters: Arc<Meters>,
    connection: Arc<Mutex<Connection>>,
    worker: Option<JoinHandle<()>>,
}

impl Engine {
    pub fn start(input: DeviceChoice, output: DeviceChoice, params: &Params, monitor: bool, tap: Tap) -> Result<Self> {
        let controls = Arc::new(Controls::new(params, monitor));
        let meters = Arc::new(Meters::default());
        let connection = Arc::new(Mutex::new(Connection::Connecting));
        let (commands, rx) = channel();
        let supervisor = Supervisor {
            ctx: Context { controls: controls.clone(), meters: meters.clone(), tap },
            connection: connection.clone(),
            input,
            output,
            session: None,
        };
        let worker = thread::Builder::new().name("engine".into()).spawn(move || supervisor.run(rx))?;
        Ok(Self { commands, controls, meters, connection, worker: Some(worker) })
    }

    pub fn controls(&self) -> &Controls {
        &self.controls
    }

    pub fn meters(&self) -> &Meters {
        &self.meters
    }

    pub fn connection(&self) -> Connection {
        self.connection.lock().unwrap().clone()
    }

    pub fn set_input(&self, choice: DeviceChoice) {
        let _ = self.commands.send(Command::SetInput(choice));
    }

    pub fn set_output(&self, choice: DeviceChoice) {
        let _ = self.commands.send(Command::SetOutput(choice));
    }
}

impl Drop for Engine {
    fn drop(&mut self) {
        let _ = self.commands.send(Command::Shutdown);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

struct Supervisor {
    ctx: Context,
    connection: Arc<Mutex<Connection>>,
    input: DeviceChoice,
    output: DeviceChoice,
    session: Option<Session>,
}

impl Supervisor {
    fn run(mut self, commands: Receiver<Command>) {
        let host = cpal::default_host();
        let mut next_check = Instant::now();
        loop {
            match commands.recv_timeout(POLL_EVERY) {
                Ok(Command::SetInput(choice)) => {
                    self.input = choice;
                    next_check = Instant::now();
                }
                Ok(Command::SetOutput(choice)) => {
                    self.output = choice;
                    next_check = Instant::now();
                }
                Ok(Command::Shutdown) | Err(RecvTimeoutError::Disconnected) => break,
                Err(RecvTimeoutError::Timeout) => {}
            }

            if let Some(problem) = self.session.as_ref().and_then(Session::problem) {
                self.session = None;
                self.set(Connection::Waiting(format!("{problem}. Reconnecting…")));
                next_check = Instant::now() + RETRY_AFTER;
            }
            if Instant::now() >= next_check {
                next_check = Instant::now() + RECHECK_EVERY;
                self.reconcile(&host);
            }
        }
    }

    /// Makes sure the running session uses the devices that should be in use right now.
    fn reconcile(&mut self, host: &cpal::Host) {
        let (input, output) = match self.resolve(host) {
            Ok(devices) => devices,
            Err(e) => {
                self.session = None;
                return self.set(Connection::Waiting(format!("{e:#} Retrying…")));
            }
        };

        let mut warnings = Vec::new();
        if let (true, DeviceChoice::Device { name, .. }) = (input.is_fallback, &self.input) {
            warnings.push(format!("{name} is not connected, using the default microphone"));
        }
        let output = output.filter(|out| {
            if let (true, DeviceChoice::Device { name, .. }) = (out.is_fallback, &self.output) {
                warnings.push(format!("{name} is not connected, using the default output"));
            }
            let clash = devices::same_hardware(&input.info.name, &out.info.name);
            if clash {
                warnings.push(format!("Not monitoring: {} is the same headset as the microphone", out.info.name));
            }
            !clash
        });

        let unchanged = self.session.as_ref().is_some_and(|s| {
            s.info.input.id == input.info.id
                && s.info.output.as_ref().map(|o| &o.id) == output.as_ref().map(|o| &o.info.id)
        });
        if unchanged {
            return self.update_warnings(warnings);
        }

        self.session = None; // release the old devices before opening new ones
        self.set(Connection::Connecting);
        match Session::start(input, output, &self.ctx) {
            Ok(session) => {
                self.set(Connection::Live { session: session.info.clone(), warnings });
                self.session = Some(session);
            }
            Err(e) => self.set(Connection::Waiting(format!("{e:#}. Retrying…"))),
        }
    }

    fn resolve(&self, host: &cpal::Host) -> Result<(Resolved, Option<Resolved>)> {
        // The input cannot be switched off: treat that as "use the default".
        let input_choice = match &self.input {
            DeviceChoice::Off => &DeviceChoice::Default,
            choice => choice,
        };
        let input = devices::resolve(host, Direction::Input, input_choice)?.expect("input is never off");
        let output = devices::resolve(host, Direction::Output, &self.output)?;
        Ok((input, output))
    }

    fn update_warnings(&self, warnings: Vec<String>) {
        if let Connection::Live { warnings: current, .. } = &mut *self.connection.lock().unwrap() {
            *current = warnings;
        }
    }

    fn set(&self, connection: Connection) {
        *self.connection.lock().unwrap() = connection;
    }
}
