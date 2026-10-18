//! High-quality mono sample-rate conversion in fixed-size chunks.

use anyhow::{Result, anyhow};
use audioadapter_buffers::direct::InterleavedSlice;
use rubato::{Async, FixedAsync, Resampler as _, SincInterpolationParameters, SincInterpolationType, WindowFunction};

pub enum Resampler {
    /// Rates already match: samples are copied through.
    Passthrough {
        chunk_len: usize,
    },
    Sinc {
        inner: Box<Async<f32>>,
        chunk_len: usize,
        scratch: Vec<f32>,
    },
}

impl Resampler {
    /// Converts from `from` Hz to `to` Hz, consuming exactly `chunk_len` samples per call.
    pub fn new(from: u32, to: u32, chunk_len: usize) -> Result<Self> {
        if from == to {
            return Ok(Self::Passthrough { chunk_len });
        }
        let params = SincInterpolationParameters::new(128, WindowFunction::BlackmanHarris2)
            .oversampling_factor(128)
            .interpolation(SincInterpolationType::Cubic);
        let inner = Async::<f32>::new_sinc(to as f64 / from as f64, 1.0, &params, chunk_len, 1, FixedAsync::Input)?;
        let scratch = vec![0.0; inner.output_frames_max()];
        Ok(Self::Sinc { inner: Box::new(inner), chunk_len, scratch })
    }

    pub fn chunk_len(&self) -> usize {
        match self {
            Self::Passthrough { chunk_len } | Self::Sinc { chunk_len, .. } => *chunk_len,
        }
    }

    /// Converts exactly [`chunk_len`](Self::chunk_len) samples and appends the result to `out`.
    pub fn process(&mut self, input: &[f32], out: &mut Vec<f32>) -> Result<()> {
        debug_assert_eq!(input.len(), self.chunk_len());
        match self {
            Self::Passthrough { .. } => out.extend_from_slice(input),
            Self::Sinc { inner, scratch, .. } => {
                let src = InterleavedSlice::new(input, 1, input.len()).map_err(|e| anyhow!("{e:?}"))?;
                let capacity = scratch.len();
                let mut dst =
                    InterleavedSlice::new_mut(scratch.as_mut_slice(), 1, capacity).map_err(|e| anyhow!("{e:?}"))?;
                let (_, produced) = inner.process_into_buffer(&src, &mut dst, None)?;
                out.extend_from_slice(&scratch[..produced]);
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn output_length_follows_the_rate_ratio() {
        let mut resampler = Resampler::new(16_000, 48_000, 160).unwrap();
        let mut out = Vec::new();
        for _ in 0..100 {
            resampler.process(&[0.0; 160], &mut out).unwrap();
        }
        // One second in, one second (give or take a few samples) out.
        assert!((out.len() as i64 - 48_000).abs() < 100, "{} samples", out.len());
    }

    #[test]
    fn passthrough_copies_samples() {
        let mut resampler = Resampler::new(48_000, 48_000, 3).unwrap();
        let mut out = vec![9.0];
        resampler.process(&[1.0, 2.0, 3.0], &mut out).unwrap();
        assert_eq!(out, [9.0, 1.0, 2.0, 3.0]);
    }
}
