//! One live microphone connection, with an output that can come and go on its own.
//!
//! ```text
//! input stream ─► ring ─► processing thread ─► ring ─► output stream (optional)
//!                         │ resample to 48 kHz
//!                         │ VoiceProcessor
//!                         ├─► recorder tap (processed + original)
//!                         └─► monitor: fade, resample to the output rate
//! ```
//!
//! The output can be attached, replaced or detached while the session runs, so a
//! headphone that disconnects never interrupts the microphone or a recording.
//! Dropping a [`Session`] stops its streams and joins the processing thread.

use super::controls::{Controls, Meters};
use super::devices::{DeviceInfo, Resolved};
use super::streams::{self, Health};
use crate::dsp::resample::Resampler;
use crate::dsp::{FRAME_LEN, Frame, SAMPLE_RATE, VoiceProcessor};
use crate::recording::Tap;
use anyhow::Result;
use cpal::Stream;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering::Relaxed};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::thread::{self, JoinHandle};
use std::time::Duration;

/// Most audio allowed to queue up for the output before frames are skipped to catch up.
/// Input and output devices run on separate clocks, so the queue slowly drifts.
const MAX_MONITOR_QUEUE: Duration = Duration::from_millis(60);
/// The microphone is considered dead if it delivers nothing for this long.
const INPUT_TIMEOUT: Duration = Duration::from_secs(3);

/// Everything a session needs that outlives it.
#[derive(Clone)]
pub struct Context {
    pub controls: Arc<Controls>,
    pub meters: Arc<Meters>,
    pub tap: Tap,
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
    monitors: Sender<Option<Monitor>>,
    meters: Arc<Meters>,
    running: Arc<AtomicBool>,
    worker: Option<JoinHandle<()>>,
    _input: Stream,
}

impl Session {
    /// Starts listening to `input`, without output.
    pub fn start(input: Resolved, ctx: &Context) -> Result<Self> {
        let health = Health::new();
        let opened = streams::open_input(&input.device, &input.info.name, &ctx.meters, &health)?;
        let (monitors, monitor_rx) = channel();
        let running = Arc::new(AtomicBool::new(true));
        let worker = {
            let (ctx, health, running, rate) = (ctx.clone(), health.clone(), running.clone(), opened.sample_rate);
            thread::Builder::new().name("processing".into()).spawn(move || {
                if let Err(e) = process(opened.ring, rate, monitor_rx, &ctx, &running) {
                    health.fail(format!("Processing failed: {e:#}"));
                }
            })?
        };
        Ok(Self {
            input: input.info,
            input_rate: opened.sample_rate,
            health,
            output: None,
            monitors,
            meters: ctx.meters.clone(),
            running,
            worker: Some(worker),
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
        let _ = self.monitors.send(None);
        self.output = None;
        let Some(output) = output else { return Ok(()) };

        let health = Health::new();
        let opened = streams::open_output(&output.device, &output.info.name, &self.meters, &health)?;
        let _ = self.monitors.send(Some(Monitor::new(opened.ring, opened.sample_rate)?));
        self.output = Some(Output { info: output.info, health, _stream: opened.stream });
        Ok(())
    }

    /// Why the microphone side stopped working, if it did. The session must be restarted.
    pub fn input_problem(&self) -> Option<String> {
        self.health.error().or_else(|| {
            (self.health.silent_for() > INPUT_TIMEOUT).then(|| "The microphone stopped sending audio".into())
        })
    }

    /// Why the output stopped working, if it did. Only the output needs replacing.
    pub fn output_problem(&self) -> Option<String> {
        self.output.as_ref()?.health.error()
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        self.running.store(false, Relaxed);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

/// The output branch of the processing thread.
struct Monitor {
    ring: rtrb::Producer<f32>,
    resampler: Resampler,
    max_queue: usize,
    volume: f32,
    scratch: Vec<f32>,
}

impl Monitor {
    fn new(ring: rtrb::Producer<f32>, rate: u32) -> Result<Self> {
        Ok(Self {
            ring,
            resampler: Resampler::new(SAMPLE_RATE, rate, FRAME_LEN)?,
            max_queue: (rate as u128 * MAX_MONITOR_QUEUE.as_millis() / 1000) as usize,
            volume: 0.0, // fade in
            scratch: Vec::with_capacity(4 * FRAME_LEN),
        })
    }

    fn play(&mut self, frame: &Frame, enabled: bool) -> Result<()> {
        // Fade over one frame when (un)muting instead of cutting abruptly.
        let target = if enabled { 1.0 } else { 0.0 };
        let (start, step) = (self.volume, (target - self.volume) / FRAME_LEN as f32);
        self.volume = target;
        let faded: Frame = std::array::from_fn(|i| frame[i] * (start + step * (i + 1) as f32));

        self.scratch.clear();
        self.resampler.process(&faded, &mut self.scratch)?;
        let queued = self.ring.buffer().capacity() - self.ring.slots();
        if queued + self.scratch.len() > self.max_queue {
            return Ok(()); // too far behind: skip this frame to win back latency
        }
        for &s in &self.scratch {
            let _ = self.ring.push(s);
        }
        Ok(())
    }
}

fn process(
    mut input: rtrb::Consumer<f32>,
    in_rate: u32,
    monitors: Receiver<Option<Monitor>>,
    ctx: &Context,
    running: &AtomicBool,
) -> Result<()> {
    let mut processor = VoiceProcessor::new(&ctx.controls.params())?;
    let mut upsampler = Resampler::new(in_rate, SAMPLE_RATE, (in_rate / 100) as usize)?;
    let chunk_len = upsampler.chunk_len();
    let mut chunk = vec![0.0; chunk_len];
    let mut pending: Vec<f32> = Vec::with_capacity(4 * FRAME_LEN);
    let mut monitor = None;

    while running.load(Relaxed) {
        if let Some(latest) = monitors.try_iter().last() {
            monitor = latest;
        }
        let Ok(read) = input.read_chunk(chunk_len) else {
            thread::sleep(Duration::from_millis(1));
            continue;
        };
        let (a, b) = read.as_slices();
        chunk[..a.len()].copy_from_slice(a);
        chunk[a.len()..].copy_from_slice(b);
        read.commit_all();
        upsampler.process(&chunk, &mut pending)?;

        let mut consumed = 0;
        while pending.len() - consumed >= FRAME_LEN {
            let original: Frame = pending[consumed..consumed + FRAME_LEN].try_into().unwrap();
            consumed += FRAME_LEN;

            let mut processed = original;
            let peak = processor.process(&mut processed, &ctx.controls.params())?;
            ctx.meters.record_peak(peak);
            ctx.tap.push(&processed, &original);
            if let Some(monitor) = monitor.as_mut() {
                monitor.play(&processed, ctx.controls.monitor())?;
            }
        }
        pending.drain(..consumed);
    }
    Ok(())
}
