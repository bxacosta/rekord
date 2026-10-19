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
//! The denoiser output lags its input by a few frames. The dry signal it is mixed
//! with, and the original returned for recording, are delayed by the same amount,
//! so the output always has the same latency (30 ms) whether noise reduction is
//! on or off, and stays aligned with the original.

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
}

pub struct VoiceProcessor {
    gain: Ramp,
    input_limiter: Compressor,
    denoiser: Denoiser,
    /// The signal entering the denoiser, delayed to match its output.
    dry: FrameDelay,
    original: FrameDelay,
    denoise_mix: Ramp,
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
            dry: FrameDelay::new(latency),
            original: FrameDelay::new(latency),
            denoise_mix: Ramp::new(mix(params.denoise)),
            enhancer: VoiceEnhancer::new(rate),
            enhance_mix: Ramp::new(mix(params.enhance)),
            output_limiter: Compressor::limiter(rate),
        })
    }

    pub fn process(&mut self, input: &Frame, params: &Params) -> Result<Processed> {
        let original = self.original.process(input);
        let mut frame = *input;
        for (sample, gain) in frame.iter_mut().zip(self.gain.glide(db_to_gain(params.gain_db), FRAME_LEN)) {
            *sample = self.input_limiter.process(*sample * gain);
        }

        // The denoiser always runs so it is already adapted to the room when switched on.
        self.denoiser.set_attenuation(params.denoise_db);
        let clean = self.denoiser.process(&frame)?;
        frame = self.dry.process(&frame);
        for ((sample, &clean), wet) in
            frame.iter_mut().zip(clean).zip(self.denoise_mix.glide(mix(params.denoise), FRAME_LEN))
        {
            *sample += (clean - *sample) * wet;
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
        Ok(Processed { audio: frame, original, peak })
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
    fn transparent_and_aligned_with_the_original_when_all_effects_are_off() {
        let mut processor = VoiceProcessor::new(&params()).unwrap();
        let delay = processor.denoiser.latency_frames();
        assert!(delay > 0);
        for i in 0..20 {
            let out = processor.process(&tone(i, 0.2), &params()).unwrap();
            let expected = if i >= delay { tone(i - delay, 0.2) } else { [0.0; FRAME_LEN] };
            assert!(max_diff(&expected, &out.audio) < 1e-3, "frame {i} differs");
            assert_eq!(out.original, expected);
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
