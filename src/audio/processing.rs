//! The processing thread. There is one for the engine's whole life: the DeepFilterNet
//! model cannot move between threads, takes a while to load, and keeps adapting to
//! the room, so it must outlive reconnections. Sessions plug their device rings in.
//!
//! ```text
//! input ring ─► resample to 48 kHz ─► VoiceProcessor ─┬─► recorder tap (processed + original)
//!                                                     └─► monitor: fade, resample ─► output ring
//! ```

use super::controls::{Controls, Meters};
use super::streams::Health;
use crate::dsp::resample::Resampler;
use crate::dsp::{FRAME_LEN, Frame, SAMPLE_RATE, VoiceProcessor};
use crate::recording::Tap;
use anyhow::{Context as _, Result};
use std::sync::Arc;
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender, channel};
use std::thread::{self, JoinHandle};
use std::time::Duration;

/// Most audio allowed to queue up for the output before frames are skipped to catch up.
/// Input and output devices run on separate clocks, so the queue slowly drifts.
const MAX_MONITOR_QUEUE: Duration = Duration::from_millis(60);
/// How long to wait for new commands when there is no audio to process.
const IDLE_WAIT: Duration = Duration::from_millis(1);

enum Patch {
    Input(Option<Input>),
    Output(Option<Monitor>),
    Shutdown,
}

/// Connects device rings to the processing thread. Cheap to clone.
#[derive(Clone)]
pub struct Patcher(Sender<Patch>);

impl Patcher {
    /// Processes audio from `input` instead of the current input (if any).
    pub fn input(&self, input: Option<Input>) {
        let _ = self.0.send(Patch::Input(input));
    }

    /// Plays the processed audio on `output` instead of the current output (if any).
    pub fn output(&self, output: Option<Monitor>) {
        let _ = self.0.send(Patch::Output(output));
    }
}

pub struct Processing {
    patcher: Patcher,
    worker: Option<JoinHandle<()>>,
}

impl Processing {
    /// Starts the thread and waits until the processor is ready.
    pub fn start(controls: Arc<Controls>, meters: Arc<Meters>, tap: Tap) -> Result<Self> {
        let (patches, rx) = channel();
        let (ready_tx, ready) = channel();
        let worker = thread::Builder::new().name("processing".into()).spawn(move || {
            match VoiceProcessor::new(&controls.params()) {
                Ok(processor) => {
                    let _ = ready_tx.send(Ok(()));
                    Worker { processor, controls, meters, tap, input: None, monitor: None }.run(rx);
                }
                Err(e) => {
                    let _ = ready_tx.send(Err(e));
                }
            }
        })?;
        ready.recv().context("The processing thread stopped")?.context("Could not start audio processing")?;
        Ok(Self { patcher: Patcher(patches), worker: Some(worker) })
    }

    pub fn patcher(&self) -> Patcher {
        self.patcher.clone()
    }
}

impl Drop for Processing {
    fn drop(&mut self) {
        let _ = self.patcher.0.send(Patch::Shutdown);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

/// A microphone ring, converted to 48 kHz frames.
pub struct Input {
    ring: rtrb::Consumer<f32>,
    upsampler: Resampler,
    chunk: Vec<f32>,
    pending: Vec<f32>,
    consumed: usize,
    /// Processing failures are reported here, which restarts the session.
    health: Arc<Health>,
}

impl Input {
    pub fn new(ring: rtrb::Consumer<f32>, rate: u32, health: Arc<Health>) -> Result<Self> {
        // 10 ms chunks, so each one yields about one frame.
        let upsampler = Resampler::new(rate, SAMPLE_RATE, (rate / 100) as usize)?;
        let chunk = vec![0.0; upsampler.chunk_len()];
        Ok(Self { ring, upsampler, chunk, pending: Vec::with_capacity(4 * FRAME_LEN), consumed: 0, health })
    }

    /// Moves the next chunk from the ring into the frame queue. False if none is available.
    fn pull(&mut self) -> Result<bool> {
        let Ok(read) = self.ring.read_chunk(self.chunk.len()) else { return Ok(false) };
        let (a, b) = read.as_slices();
        self.chunk[..a.len()].copy_from_slice(a);
        self.chunk[a.len()..].copy_from_slice(b);
        read.commit_all();
        self.pending.drain(..self.consumed);
        self.consumed = 0;
        self.upsampler.process(&self.chunk, &mut self.pending)?;
        Ok(true)
    }

    fn next_frame(&mut self) -> Option<Frame> {
        let frame = self.pending.get(self.consumed..self.consumed + FRAME_LEN)?.try_into().ok()?;
        self.consumed += FRAME_LEN;
        Some(frame)
    }
}

/// An output ring, fed with processed audio at the output's rate.
pub struct Monitor {
    ring: rtrb::Producer<f32>,
    resampler: Resampler,
    max_queue: usize,
    volume: f32,
    scratch: Vec<f32>,
    /// Failures are reported here, which makes the engine replace the output.
    health: Arc<Health>,
}

impl Monitor {
    pub fn new(ring: rtrb::Producer<f32>, rate: u32, health: Arc<Health>) -> Result<Self> {
        Ok(Self {
            ring,
            resampler: Resampler::new(SAMPLE_RATE, rate, FRAME_LEN)?,
            max_queue: (rate as u128 * MAX_MONITOR_QUEUE.as_millis() / 1000) as usize,
            volume: 0.0, // fade in
            scratch: Vec::with_capacity(4 * FRAME_LEN),
            health,
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

struct Worker {
    processor: VoiceProcessor,
    controls: Arc<Controls>,
    meters: Arc<Meters>,
    tap: Tap,
    input: Option<Input>,
    monitor: Option<Monitor>,
}

impl Worker {
    fn run(mut self, patches: Receiver<Patch>) {
        let mut idle = false;
        loop {
            let waited = if idle {
                match patches.recv_timeout(IDLE_WAIT) {
                    Ok(patch) => Some(patch),
                    Err(RecvTimeoutError::Timeout) => None,
                    Err(RecvTimeoutError::Disconnected) => return,
                }
            } else {
                None
            };
            for patch in waited.into_iter().chain(patches.try_iter()) {
                match patch {
                    Patch::Input(input) => self.input = input,
                    Patch::Output(monitor) => self.monitor = monitor,
                    Patch::Shutdown => return,
                }
            }
            idle = !self.process_available();
        }
    }

    /// Processes the next chunk of input, if there is one.
    fn process_available(&mut self) -> bool {
        let Some(input) = self.input.as_mut() else { return false };
        match input.pull() {
            Ok(true) => {}
            Ok(false) => return false,
            Err(e) => {
                input.health.fail(format!("Processing failed: {e:#}"));
                self.input = None;
                return false;
            }
        }
        while let Some(frame) = input.next_frame() {
            let out = match self.processor.process(&frame, &self.controls.params()) {
                Ok(out) => out,
                Err(e) => {
                    input.health.fail(format!("Processing failed: {e:#}"));
                    self.input = None;
                    return false;
                }
            };
            self.meters.record_peak(out.peak);
            self.tap.push(&out.audio, &out.original);
            if let Some(monitor) = self.monitor.as_mut()
                && let Err(e) = monitor.play(&out.audio, self.controls.monitor())
            {
                monitor.health.fail(format!("Output failed: {e:#}"));
                self.monitor = None;
            }
        }
        true
    }
}
