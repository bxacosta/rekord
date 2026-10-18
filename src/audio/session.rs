//! One live connection: microphone stream, processing thread and (optional) monitor stream.
//!
//! ```text
//! input callback ─► ring ─► processing thread ─► ring ─► output callback
//!                           │ resample to 48 kHz
//!                           │ VoiceProcessor
//!                           ├─► recorder tap (processed + original)
//!                           └─► resample to the output rate
//! ```
//!
//! The device callbacks only move samples; everything else happens on the processing
//! thread. Dropping a [`Session`] stops the streams and joins the thread.

use super::controls::{Controls, Meters};
use super::devices::{DeviceInfo, Resolved};
use crate::dsp::resample::Resampler;
use crate::dsp::{FRAME_LEN, Frame, SAMPLE_RATE, VoiceProcessor};
use crate::recording::Tap;
use anyhow::{Context as _, Result, bail};
use cpal::traits::{DeviceTrait, StreamTrait};
use cpal::{Device, FromSample, Sample, SampleFormat, SizedSample, Stream, StreamConfig};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering::Relaxed};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

/// Most audio allowed to queue up for the monitor before frames are skipped to catch up.
/// Input and output devices run on separate clocks, so the queue slowly drifts.
const MAX_MONITOR_QUEUE: Duration = Duration::from_millis(60);
/// After running dry, the monitor waits for this much audio before playing again.
/// Absorbs Bluetooth's bursty delivery at the cost of a little latency.
const MONITOR_PREBUFFER: Duration = Duration::from_millis(20);
/// The input is considered dead if it delivers nothing for this long.
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

pub struct Session {
    pub info: SessionInfo,
    health: Arc<Health>,
    running: Arc<AtomicBool>,
    worker: Option<JoinHandle<()>>,
    // Streams stop when dropped; kept last so they stop after the worker is told to.
    _input: Stream,
    _output: Option<Stream>,
}

impl Session {
    pub fn start(input: Resolved, output: Option<Resolved>, ctx: &Context) -> Result<Self> {
        let health = Arc::new(Health::new());

        let in_config =
            input.device.default_input_config().with_context(|| format!("Cannot open {}", input.info.name))?;
        let in_rate = in_config.sample_rate();
        let (in_tx, in_rx) = rtrb::RingBuffer::<f32>::new(in_rate as usize);
        let input_stream =
            build_input(&input.device, in_config.sample_format(), &in_config.config(), in_tx, ctx, &health)
                .with_context(|| format!("Cannot open {}", input.info.name))?;

        let mut monitor = None;
        let output_stream = match &output {
            Some(out) => {
                let config =
                    out.device.default_output_config().with_context(|| format!("Cannot open {}", out.info.name))?;
                let rate = config.sample_rate();
                let (tx, rx) = rtrb::RingBuffer::<f32>::new(rate as usize);
                monitor = Some(Monitor::new(tx, rate)?);
                let stream = build_output(&out.device, config.sample_format(), &config.config(), rx, ctx, &health)
                    .with_context(|| format!("Cannot open {}", out.info.name))?;
                Some(stream)
            }
            None => None,
        };

        let running = Arc::new(AtomicBool::new(true));
        let worker = {
            let (ctx, health, running) = (ctx.clone(), health.clone(), running.clone());
            thread::Builder::new().name("processing".into()).spawn(move || {
                if let Err(e) = process(in_rx, in_rate, monitor, &ctx, &running) {
                    health.fail(format!("Processing failed: {e:#}"));
                }
            })?
        };

        input_stream.play()?;
        if let Some(stream) = &output_stream {
            stream.play()?;
        }

        Ok(Self {
            info: SessionInfo { input: input.info, input_rate: in_rate, output: output.map(|o| o.info) },
            health,
            running,
            worker: Some(worker),
            _input: input_stream,
            _output: output_stream,
        })
    }

    /// Why this session stopped working, if it did.
    pub fn problem(&self) -> Option<String> {
        self.health.problem()
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

/// Liveness of a session, reported by the device callbacks.
struct Health {
    epoch: Instant,
    last_input_ms: AtomicU64,
    error: Mutex<Option<String>>,
}

impl Health {
    fn new() -> Self {
        Self { epoch: Instant::now(), last_input_ms: AtomicU64::new(0), error: Mutex::new(None) }
    }

    fn now_ms(&self) -> u64 {
        self.epoch.elapsed().as_millis() as u64
    }

    fn input_alive(&self) {
        self.last_input_ms.store(self.now_ms(), Relaxed);
    }

    fn fail(&self, message: String) {
        self.error.lock().unwrap().get_or_insert(message);
    }

    fn problem(&self) -> Option<String> {
        if let Some(error) = self.error.lock().unwrap().clone() {
            return Some(error);
        }
        let silent_ms = self.now_ms().saturating_sub(self.last_input_ms.load(Relaxed));
        (silent_ms > INPUT_TIMEOUT.as_millis() as u64).then(|| "The microphone stopped sending audio".into())
    }

    /// Stream errors: glitches are counted, anything else ends the session.
    fn stream_error(&self, meters: &Meters, side: &str, error: cpal::Error) {
        match error.kind() {
            cpal::ErrorKind::Xrun => meters.record_dropout(),
            cpal::ErrorKind::RealtimeDenied | cpal::ErrorKind::DeviceChanged => {}
            _ => self.fail(format!("{side}: {error}")),
        }
    }
}

fn build_input(
    device: &Device,
    format: SampleFormat,
    config: &StreamConfig,
    ring: rtrb::Producer<f32>,
    ctx: &Context,
    health: &Arc<Health>,
) -> Result<Stream> {
    match format {
        SampleFormat::F32 => input_stream::<f32>(device, config, ring, ctx, health),
        SampleFormat::I16 => input_stream::<i16>(device, config, ring, ctx, health),
        SampleFormat::I32 => input_stream::<i32>(device, config, ring, ctx, health),
        SampleFormat::U16 => input_stream::<u16>(device, config, ring, ctx, health),
        other => bail!("unsupported input sample format {other}"),
    }
}

fn input_stream<T>(
    device: &Device,
    config: &StreamConfig,
    mut ring: rtrb::Producer<f32>,
    ctx: &Context,
    health: &Arc<Health>,
) -> Result<Stream>
where
    T: SizedSample,
    f32: FromSample<T>,
{
    let channels = config.channels as usize;
    let (meters, health, err_meters, err_health) =
        (ctx.meters.clone(), health.clone(), ctx.meters.clone(), health.clone());
    let mut overflowing = false;
    let stream = device.build_input_stream(
        *config,
        move |data: &[T], _: &cpal::InputCallbackInfo| {
            health.input_alive();
            for frame in data.chunks_exact(channels) {
                let mono = frame.iter().map(|s| f32::from_sample(*s)).sum::<f32>() / channels as f32;
                // A full ring means processing is stalled: count it once per episode.
                let lost = ring.push(mono).is_err();
                if lost && !overflowing {
                    meters.record_dropout();
                }
                overflowing = lost;
            }
        },
        move |e| err_health.stream_error(&err_meters, "Microphone", e),
        None,
    )?;
    Ok(stream)
}

fn build_output(
    device: &Device,
    format: SampleFormat,
    config: &StreamConfig,
    ring: rtrb::Consumer<f32>,
    ctx: &Context,
    health: &Arc<Health>,
) -> Result<Stream> {
    match format {
        SampleFormat::F32 => output_stream::<f32>(device, config, ring, ctx, health),
        SampleFormat::I16 => output_stream::<i16>(device, config, ring, ctx, health),
        SampleFormat::I32 => output_stream::<i32>(device, config, ring, ctx, health),
        SampleFormat::U16 => output_stream::<u16>(device, config, ring, ctx, health),
        other => bail!("unsupported output sample format {other}"),
    }
}

fn output_stream<T>(
    device: &Device,
    config: &StreamConfig,
    mut ring: rtrb::Consumer<f32>,
    ctx: &Context,
    health: &Arc<Health>,
) -> Result<Stream>
where
    T: SizedSample + FromSample<f32>,
{
    let channels = config.channels as usize;
    let prebuffer = samples_in(config.sample_rate, MONITOR_PREBUFFER);
    let (meters, err_meters, err_health) = (ctx.meters.clone(), ctx.meters.clone(), health.clone());
    let mut buffering = true;
    let mut has_played = false;
    let stream = device.build_output_stream(
        *config,
        move |data: &mut [T], _: &cpal::OutputCallbackInfo| {
            if buffering && ring.slots() >= prebuffer {
                buffering = false;
                has_played = true;
            }
            for frame in data.chunks_exact_mut(channels) {
                let sample = if buffering {
                    0.0
                } else if let Ok(s) = ring.pop() {
                    s
                } else {
                    // Ran dry: play silence until the prebuffer refills.
                    buffering = true;
                    if has_played {
                        meters.record_dropout();
                    }
                    0.0
                };
                frame.fill(T::from_sample(sample));
            }
        },
        move |e| err_health.stream_error(&err_meters, "Output", e),
        None,
    )?;
    Ok(stream)
}

/// The monitoring branch of the processing thread.
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
            max_queue: samples_in(rate, MAX_MONITOR_QUEUE),
            volume: 1.0,
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
    mut monitor: Option<Monitor>,
    ctx: &Context,
    running: &AtomicBool,
) -> Result<()> {
    let mut processor = VoiceProcessor::new(&ctx.controls.params())?;
    let mut upsampler = Resampler::new(in_rate, SAMPLE_RATE, (in_rate / 100) as usize)?;
    let chunk_len = upsampler.chunk_len();
    let mut chunk = vec![0.0; chunk_len];
    let mut pending: Vec<f32> = Vec::with_capacity(4 * FRAME_LEN);

    while running.load(Relaxed) {
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

fn samples_in(rate: u32, duration: Duration) -> usize {
    (rate as u128 * duration.as_millis() / 1000) as usize
}
