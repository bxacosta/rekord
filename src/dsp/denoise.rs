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
}

impl Denoiser {
    /// `attenuation_db` caps how much noise may be removed: lower values sound more natural.
    pub fn new(attenuation_db: f32) -> Result<Self> {
        let params = RuntimeParams::default_with_ch(1).with_atten_lim(attenuation_db);
        let model = DfTract::new(DfParams::default(), &params)?;
        ensure!(model.hop_size == FRAME_LEN, "unexpected denoiser frame size {}", model.hop_size);
        Ok(Self { model, noisy: Array2::zeros((1, FRAME_LEN)), clean: Array2::zeros((1, FRAME_LEN)), attenuation_db })
    }

    pub fn set_attenuation(&mut self, db: f32) {
        if db != self.attenuation_db {
            self.model.set_atten_lim(db);
            self.attenuation_db = db;
        }
    }

    /// Returns the denoised version of `frame` (borrowed until the next call).
    pub fn process(&mut self, frame: &Frame) -> Result<&[f32]> {
        self.noisy.row_mut(0).iter_mut().zip(frame).for_each(|(dst, src)| *dst = *src);
        self.model.process(self.noisy.view(), self.clean.view_mut())?;
        Ok(self.clean.as_slice().expect("denoiser output is contiguous"))
    }
}
