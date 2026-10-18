//! Device streams. The callbacks only move samples between the device and a ring
//! buffer, and report their health; all processing happens elsewhere.

use super::controls::Meters;
use anyhow::{Context as _, Result, bail};
use cpal::traits::{DeviceTrait, StreamTrait};
use cpal::{Device, FromSample, Sample, SampleFormat, SizedSample, Stream, StreamConfig};
use std::sync::atomic::{AtomicU64, Ordering::Relaxed};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// After running dry, the output waits for this much audio before playing again.
/// Absorbs Bluetooth's bursty delivery at the cost of a little latency.
const OUTPUT_PREBUFFER: Duration = Duration::from_millis(20);

/// Liveness of one stream, reported from its callbacks.
pub struct Health {
    epoch: Instant,
    last_data_ms: AtomicU64,
    error: Mutex<Option<String>>,
}

impl Health {
    pub fn new() -> Arc<Self> {
        Arc::new(Self { epoch: Instant::now(), last_data_ms: AtomicU64::new(0), error: Mutex::new(None) })
    }

    /// Records the first fatal error; later ones are usually consequences of it.
    pub fn fail(&self, message: String) {
        self.error.lock().unwrap().get_or_insert(message);
    }

    pub fn error(&self) -> Option<String> {
        self.error.lock().unwrap().clone()
    }

    /// Time since the stream last delivered data (or since it was created).
    pub fn silent_for(&self) -> Duration {
        let now = self.epoch.elapsed().as_millis() as u64;
        Duration::from_millis(now.saturating_sub(self.last_data_ms.load(Relaxed)))
    }

    fn alive(&self) {
        self.last_data_ms.store(self.epoch.elapsed().as_millis() as u64, Relaxed);
    }

    /// Glitches are counted; anything else means the stream is dead.
    fn stream_error(&self, meters: &Meters, device: &str, error: cpal::Error) {
        match error.kind() {
            cpal::ErrorKind::Xrun => meters.record_dropout(),
            cpal::ErrorKind::RealtimeDenied | cpal::ErrorKind::DeviceChanged => {}
            _ => self.fail(format!("{device}: {error}")),
        }
    }
}

/// An open, playing stream plus the ring buffer end the rest of the app uses.
pub struct Opened<R> {
    pub stream: Stream,
    pub sample_rate: u32,
    pub ring: R,
}

/// Opens a microphone. Its audio arrives, mixed down to mono, on the returned ring.
pub fn open_input(
    device: &Device,
    name: &str,
    meters: &Arc<Meters>,
    health: &Arc<Health>,
) -> Result<Opened<rtrb::Consumer<f32>>> {
    let config = device.default_input_config().with_context(|| format!("Cannot open {name}"))?;
    let sample_rate = config.sample_rate();
    // One second of headroom: only a stalled processing thread can fill it.
    let (producer, ring) = rtrb::RingBuffer::new(sample_rate as usize);
    let args = (device, &config.config(), producer, meters, health, name);
    let stream = match config.sample_format() {
        SampleFormat::F32 => input_stream::<f32>(args),
        SampleFormat::I16 => input_stream::<i16>(args),
        SampleFormat::I32 => input_stream::<i32>(args),
        SampleFormat::U16 => input_stream::<u16>(args),
        other => bail!("{name} uses an unsupported sample format ({other})"),
    }
    .with_context(|| format!("Cannot open {name}"))?;
    stream.play().with_context(|| format!("Cannot start {name}"))?;
    Ok(Opened { stream, sample_rate, ring })
}

/// Opens an output. Mono audio pushed into the returned ring is played on every channel.
pub fn open_output(
    device: &Device,
    name: &str,
    meters: &Arc<Meters>,
    health: &Arc<Health>,
) -> Result<Opened<rtrb::Producer<f32>>> {
    let config = device.default_output_config().with_context(|| format!("Cannot open {name}"))?;
    let sample_rate = config.sample_rate();
    let (ring, consumer) = rtrb::RingBuffer::new(sample_rate as usize);
    let args = (device, &config.config(), consumer, meters, health, name);
    let stream = match config.sample_format() {
        SampleFormat::F32 => output_stream::<f32>(args),
        SampleFormat::I16 => output_stream::<i16>(args),
        SampleFormat::I32 => output_stream::<i32>(args),
        SampleFormat::U16 => output_stream::<u16>(args),
        other => bail!("{name} uses an unsupported sample format ({other})"),
    }
    .with_context(|| format!("Cannot open {name}"))?;
    stream.play().with_context(|| format!("Cannot start {name}"))?;
    Ok(Opened { stream, sample_rate, ring })
}

type StreamArgs<'a, R> = (&'a Device, &'a StreamConfig, R, &'a Arc<Meters>, &'a Arc<Health>, &'a str);

fn input_stream<T>((device, config, mut ring, meters, health, name): StreamArgs<rtrb::Producer<f32>>) -> Result<Stream>
where
    T: SizedSample,
    f32: FromSample<T>,
{
    let channels = config.channels as usize;
    let (meters, health, err_meters, err_health, name) =
        (meters.clone(), health.clone(), meters.clone(), health.clone(), name.to_owned());
    let mut overflowing = false;
    let stream = device.build_input_stream(
        *config,
        move |data: &[T], _: &cpal::InputCallbackInfo| {
            health.alive();
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
        move |e| err_health.stream_error(&err_meters, &name, e),
        None,
    )?;
    Ok(stream)
}

fn output_stream<T>((device, config, mut ring, meters, health, name): StreamArgs<rtrb::Consumer<f32>>) -> Result<Stream>
where
    T: SizedSample + FromSample<f32>,
{
    let channels = config.channels as usize;
    let prebuffer = (config.sample_rate as u128 * OUTPUT_PREBUFFER.as_millis() / 1000) as usize;
    let (meters, err_meters, err_health, name) = (meters.clone(), meters.clone(), health.clone(), name.to_owned());
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
        move |e| err_health.stream_error(&err_meters, &name, e),
        None,
    )?;
    Ok(stream)
}
