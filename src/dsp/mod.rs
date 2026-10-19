//! Voice processing chain, independent of any audio device.
//!
//! ```text
//! gain ─► limiter ─► noise reduction ─► voice enhance ─► limiter
//! ```
//!
//! Gain comes first so the denoiser sees the level you actually hear: Bluetooth
//! headsets deliver very quiet audio, and DeepFilterNet treats anything that
//! quiet as silence. Every stage change glides over one frame, so toggling
//! effects or changing the gain never clicks.
//!
//! The denoiser output lags its input by 30 ms. That latency only applies while
//! noise reduction is on: when it is off the signal bypasses the denoiser (which
//! keeps running, to stay adapted to the room). Switching fades out, changes the
//! delay and fades back in; see [`VoiceProcessor::process`].

mod biquad;
mod delay;
pub mod denoise;
pub mod dynamics;
mod enhance;
mod ramp;
pub mod resample;

use anyhow::Result;
use delay::FrameDelay;
use denoise::Denoiser;
use dynamics::{Compressor, db_to_gain};
use enhance::VoiceEnhancer;
use ramp::Ramp;

/// The rate everything is processed and recorded at (required by DeepFilterNet).
pub const SAMPLE_RATE: u32 = 48_000;
/// Samples per processing frame: 10 ms.
pub const FRAME_LEN: usize = 480;

pub type Frame = [f32; FRAME_LEN];

/// User-facing processing settings, read once per frame.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Params {
    pub gain_db: f32,
    pub enhance: bool,
    pub denoise: bool,
    pub denoise_db: f32,
}

/// One processed frame.
pub struct Processed {
    pub audio: Frame,
    /// The input, delayed to line up with `audio`.
    pub original: Frame,
    /// Peak level of `audio` (linear, 0..=1).
    pub peak: f32,
    /// A silent frame that fills the denoiser's latency while noise reduction
    /// switches on. It must be played but not recorded.
    pub is_gap: bool,
}

pub struct VoiceProcessor {
    gain: Ramp,
    input_limiter: Compressor,
    denoiser: Denoiser,
    /// Whether the output comes from the denoiser, and so carries its latency.
    denoising: bool,
    /// Silent frames left to play while switching noise reduction on.
    gap: usize,
    /// Dips to silence while noise reduction switches on or off.
    level: Ramp,
    /// The input, delayed to match the denoiser output.
    delayed_input: FrameDelay,
    enhancer: VoiceEnhancer,
    enhance_mix: Ramp,
    output_limiter: Compressor,
}

impl VoiceProcessor {
    pub fn new(params: &Params) -> Result<Self> {
        let rate = SAMPLE_RATE as f32;
        let denoiser = Denoiser::new(params.denoise_db)?;
        let latency = denoiser.latency_frames();
        Ok(Self {
            gain: Ramp::new(db_to_gain(params.gain_db)),
            input_limiter: Compressor::limiter(rate),
            denoiser,
            denoising: params.denoise,
            gap: 0,
            level: Ramp::new(1.0),
            delayed_input: FrameDelay::new(latency),
            enhancer: VoiceEnhancer::new(rate),
            enhance_mix: Ramp::new(mix(params.enhance)),
            output_limiter: Compressor::limiter(rate),
        })
    }

    /// Processes one frame.
    ///
    /// Switching noise reduction changes the output's delay. Turning it on fades
    /// out, plays the denoiser's latency as silence ([`Processed::is_gap`]) and
    /// fades in, so no audio is repeated. Turning it off fades out and in,
    /// skipping the audio still inside the denoiser.
    pub fn process(&mut self, input: &Frame, params: &Params) -> Result<Processed> {
        let delayed_input = self.delayed_input.process(input);
        let mut frame = *input;
        for (sample, gain) in frame.iter_mut().zip(self.gain.glide(db_to_gain(params.gain_db), FRAME_LEN)) {
            *sample = self.input_limiter.process(*sample * gain);
        }

        // The denoiser always runs so it is already adapted to the room when switched on.
        self.denoiser.set_attenuation(params.denoise_db);
        let clean = self.denoiser.process(&frame)?;

        if self.gap > 0 {
            // These denoised frames were already played without noise reduction.
            self.gap -= 1;
            return Ok(Processed { audio: [0.0; FRAME_LEN], original: [0.0; FRAME_LEN], peak: 0.0, is_gap: true });
        }
        let original = if self.denoising {
            frame.copy_from_slice(clean);
            delayed_input
        } else {
            *input
        };

        let switching = params.denoise != self.denoising;
        let level = if switching { 0.0 } else { 1.0 };
        for (sample, level) in frame.iter_mut().zip(self.level.glide(level, FRAME_LEN)) {
            *sample *= level;
        }
        if switching {
            self.denoising = params.denoise;
            if self.denoising {
                self.gap = self.denoiser.latency_frames();
            }
        }

        let target = mix(params.enhance);
        if target == 0.0 && self.enhance_mix.value() == 0.0 {
            // Fully off: keep the enhancer idle so it starts from a clean state when enabled.
            self.enhancer.reset();
        } else {
            for (sample, wet) in frame.iter_mut().zip(self.enhance_mix.glide(target, FRAME_LEN)) {
                let enhanced = self.enhancer.process(*sample);
                *sample += (enhanced - *sample) * wet;
            }
        }

        let mut peak = 0f32;
        for sample in frame.iter_mut() {
            *sample = self.output_limiter.process(*sample).clamp(-1.0, 1.0);
            peak = peak.max(sample.abs());
        }
        Ok(Processed { audio: frame, original, peak, is_gap: false })
    }
}

fn mix(enabled: bool) -> f32 {
    if enabled { 1.0 } else { 0.0 }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f32::consts::PI;

    fn params() -> Params {
        Params { gain_db: 0.0, enhance: false, denoise: false, denoise_db: 18.0 }
    }

    fn tone(frame_index: usize, amplitude: f32) -> Frame {
        std::array::from_fn(|i| {
            let t = (frame_index * FRAME_LEN + i) as f32 / SAMPLE_RATE as f32;
            amplitude * (2.0 * PI * 440.0 * t).sin()
        })
    }

    fn max_diff(a: &Frame, b: &Frame) -> f32 {
        a.iter().zip(b).map(|(a, b)| (a - b).abs()).fold(0.0, f32::max)
    }

    #[test]
    fn transparent_and_without_delay_when_all_effects_are_off() {
        let mut processor = VoiceProcessor::new(&params()).unwrap();
        for i in 0..20 {
            let out = processor.process(&tone(i, 0.2), &params()).unwrap();
            assert!(max_diff(&tone(i, 0.2), &out.audio) < 1e-3, "frame {i} differs");
            assert_eq!(out.original, tone(i, 0.2));
            assert!(!out.is_gap);
        }
    }

    #[test]
    fn switching_noise_reduction_changes_the_delay_without_repeating_audio() {
        let mut processor = VoiceProcessor::new(&params()).unwrap();
        let latency = processor.denoiser.latency_frames();
        let (on_at, off_at) = (10, 30);
        let (mut gaps, mut recorded, mut previous) = (0, Vec::new(), 0.0);
        for i in 0..50 {
            let denoise = (on_at..off_at).contains(&i);
            let out = processor.process(&tone(i, 0.2), &Params { denoise, ..params() }).unwrap();
            for &s in &out.audio {
                assert!((s - previous).abs() < 0.05, "click at frame {i}");
                previous = s;
            }
            if out.is_gap {
                gaps += 1;
            } else {
                recorded.push(out.original);
            }
        }
        assert_eq!(gaps, latency);
        // The recorded original runs on without repeats; turning off skips the
        // frames that were still inside the denoiser.
        let expected: Vec<Frame> = (0..=off_at - latency).chain(off_at + 1..50).map(|i| tone(i, 0.2)).collect();
        assert_eq!(recorded.len(), expected.len());
        for (n, (got, want)) in recorded.iter().zip(&expected).enumerate() {
            assert!(max_diff(got, want) < 1e-6, "recorded frame {n} is out of place");
        }
    }

    #[test]
    fn output_never_clips_even_with_high_gain() {
        let loud = Params { gain_db: 30.0, enhance: true, denoise: true, ..params() };
        let mut processor = VoiceProcessor::new(&loud).unwrap();
        for i in 0..100 {
            let out = processor.process(&tone(i, 0.9), &loud).unwrap();
            assert!(out.peak <= 1.0);
        }
    }

    #[test]
    fn toggling_an_effect_does_not_jump() {
        let mut processor = VoiceProcessor::new(&params()).unwrap();
        let mut previous = 0.0;
        for i in 0..40 {
            let enhance = i >= 20;
            let out = processor.process(&tone(i, 0.2), &Params { enhance, ..params() }).unwrap();
            for &s in &out.audio {
                assert!((s - previous).abs() < 0.2, "discontinuity at frame {i}");
                previous = s;
            }
        }
    }
}
