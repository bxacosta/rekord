//! Neural noise reduction with DeepFilterNet.

use super::{FRAME_LEN, Frame};
use anyhow::{Result, ensure};
use df::tract::{DfParams, DfTract, RuntimeParams};
use ndarray::Array2;

/// Attenuation at or above this value means "remove as much noise as possible".
pub const UNLIMITED_DB: f32 = 100.0;

pub struct Denoiser {
    model: DfTract,
    noisy: Array2<f32>,
    clean: Array2<f32>,
    attenuation_db: f32,
    latency_frames: usize,
}

impl Denoiser {
    /// `attenuation_db` caps how much noise may be removed: lower values sound more natural.
    pub fn new(attenuation_db: f32) -> Result<Self> {
        let params = RuntimeParams::default_with_ch(1).with_atten_lim(attenuation_db);
        let model = DfTract::new(DfParams::default(), &params)?;
        ensure!(model.hop_size == FRAME_LEN, "unexpected denoiser frame size {}", model.hop_size);
        // STFT overlap plus the model's lookahead, as computed by DeepFilterNet's own tools.
        let latency = model.fft_size - model.hop_size + model.lookahead * model.hop_size;
        ensure!(latency % FRAME_LEN == 0, "denoiser latency of {latency} samples is not a whole number of frames");
        Ok(Self {
            model,
            noisy: Array2::zeros((1, FRAME_LEN)),
            clean: Array2::zeros((1, FRAME_LEN)),
            attenuation_db,
            latency_frames: latency / FRAME_LEN,
        })
    }

    /// How many frames the output lags behind the input.
    pub fn latency_frames(&self) -> usize {
        self.latency_frames
    }

    pub fn set_attenuation(&mut self, db: f32) {
        if db != self.attenuation_db {
            self.model.set_atten_lim(db);
            self.attenuation_db = db;
        }
    }

    /// Returns the denoised version of the frame from [`latency_frames`](Self::latency_frames)
    /// calls ago (borrowed until the next call).
    pub fn process(&mut self, frame: &Frame) -> Result<&[f32]> {
        self.noisy.row_mut(0).iter_mut().zip(frame).for_each(|(dst, src)| *dst = *src);
        self.model.process(self.noisy.view(), self.clean.view_mut())?;
        Ok(self.clean.as_slice().expect("denoiser output is contiguous"))
    }
}
