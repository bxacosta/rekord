//! One live microphone connection, with an output that can come and go on its own.
//!
//! A session owns the device streams and plugs their rings into the processing
//! thread (see [`processing`](super::processing)). The output can be attached,
//! replaced or detached while the session runs, so a headphone that disconnects
//! never interrupts the microphone or a recording.

use super::controls::Meters;
use super::devices::{DeviceInfo, Resolved};
use super::processing::{Input, Monitor, Patcher};
use super::streams::{self, Health};
use anyhow::Result;
use cpal::Stream;
use std::sync::Arc;
use std::time::Duration;

/// The microphone is considered dead if it stops delivering audio for this long.
const INPUT_TIMEOUT: Duration = Duration::from_secs(3);
/// How long a microphone may take to deliver its first audio. Bluetooth headsets need
/// several seconds to switch to their hands-free profile.
const INPUT_STARTUP_TIMEOUT: Duration = Duration::from_secs(15);

/// Everything a session needs that outlives it.
#[derive(Clone)]
pub struct Context {
    pub meters: Arc<Meters>,
    pub patcher: Patcher,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SessionInfo {
    pub input: DeviceInfo,
    pub input_rate: u32,
    pub output: Option<DeviceInfo>,
}

struct Output {
    info: DeviceInfo,
    health: Arc<Health>,
    _stream: Stream,
}

pub struct Session {
    input: DeviceInfo,
    input_rate: u32,
    /// Microphone and processing failures: both end the session.
    health: Arc<Health>,
    output: Option<Output>,
    ctx: Context,
    _input: Stream,
}

impl Session {
    /// Starts listening to `input`, without output.
    pub fn start(input: Resolved, ctx: &Context) -> Result<Self> {
        let health = Health::new();
        let opened = streams::open_input(&input.device, &input.info.name, &ctx.meters, &health)?;
        ctx.patcher.input(Some(Input::new(opened.ring, opened.sample_rate, health.clone())?));
        Ok(Self {
            input: input.info,
            input_rate: opened.sample_rate,
            health,
            output: None,
            ctx: ctx.clone(),
            _input: opened.stream,
        })
    }

    pub fn info(&self) -> SessionInfo {
        SessionInfo {
            input: self.input.clone(),
            input_rate: self.input_rate,
            output: self.output.as_ref().map(|o| o.info.clone()),
        }
    }

    pub fn input_id(&self) -> &str {
        &self.input.id
    }

    pub fn output_id(&self) -> Option<&str> {
        self.output.as_ref().map(|o| o.info.id.as_str())
    }

    /// Plays the processed audio on `output` instead of the current output (if any).
    /// On failure the session keeps running without output.
    pub fn set_output(&mut self, output: Option<Resolved>) -> Result<()> {
        self.ctx.patcher.output(None);
        self.output = None;
        let Some(output) = output else { return Ok(()) };

        let health = Health::new();
        let opened = streams::open_output(&output.device, &output.info.name, &self.ctx.meters, &health)?;
        self.ctx.patcher.output(Some(Monitor::new(opened.ring, opened.sample_rate, health.clone())?));
        self.output = Some(Output { info: output.info, health, _stream: opened.stream });
        Ok(())
    }

    /// Why the microphone side stopped working, if it did. The session must be restarted.
    pub fn input_problem(&self) -> Option<String> {
        self.health.error().or_else(|| {
            let (timeout, problem) = if self.health.has_data() {
                (INPUT_TIMEOUT, "The microphone stopped sending audio")
            } else {
                (INPUT_STARTUP_TIMEOUT, "The microphone is not sending any audio")
            };
            (self.health.silent_for() > timeout).then(|| problem.into())
        })
    }

    /// Why the output stopped working, if it did. Only the output needs replacing.
    pub fn output_problem(&self) -> Option<String> {
        self.output.as_ref()?.health.error()
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        self.ctx.patcher.input(None);
        self.ctx.patcher.output(None);
    }
}
