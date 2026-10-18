//! Lock-free values shared between the UI and the audio threads.
//! They outlive individual sessions, so settings survive reconnections.

use crate::dsp::Params;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering::Relaxed};

#[derive(Default)]
pub struct AtomicF32(AtomicU32);

impl AtomicF32 {
    pub fn new(value: f32) -> Self {
        Self(AtomicU32::new(value.to_bits()))
    }

    pub fn load(&self) -> f32 {
        f32::from_bits(self.0.load(Relaxed))
    }

    pub fn store(&self, value: f32) {
        self.0.store(value.to_bits(), Relaxed);
    }

    /// Stores `value` if it is larger; returns nothing (used for peak tracking).
    pub fn fetch_max(&self, value: f32) {
        let _ = self.0.try_update(Relaxed, Relaxed, |old| (value > f32::from_bits(old)).then_some(value.to_bits()));
    }

    pub fn take(&self) -> f32 {
        f32::from_bits(self.0.swap(0, Relaxed))
    }
}

/// Settings written by the UI, read by the processing thread every frame.
pub struct Controls {
    gain_db: AtomicF32,
    enhance: AtomicBool,
    denoise: AtomicBool,
    denoise_db: AtomicF32,
    monitor: AtomicBool,
}

impl Controls {
    pub fn new(params: &Params, monitor: bool) -> Self {
        Self {
            gain_db: AtomicF32::new(params.gain_db),
            enhance: AtomicBool::new(params.enhance),
            denoise: AtomicBool::new(params.denoise),
            denoise_db: AtomicF32::new(params.denoise_db),
            monitor: AtomicBool::new(monitor),
        }
    }

    pub fn params(&self) -> Params {
        Params {
            gain_db: self.gain_db.load(),
            enhance: self.enhance.load(Relaxed),
            denoise: self.denoise.load(Relaxed),
            denoise_db: self.denoise_db.load(),
        }
    }

    pub fn set_params(&self, params: &Params) {
        self.gain_db.store(params.gain_db);
        self.enhance.store(params.enhance, Relaxed);
        self.denoise.store(params.denoise, Relaxed);
        self.denoise_db.store(params.denoise_db);
    }

    pub fn monitor(&self) -> bool {
        self.monitor.load(Relaxed)
    }

    pub fn set_monitor(&self, on: bool) {
        self.monitor.store(on, Relaxed);
    }
}

/// Measurements written by the audio threads, read by the UI.
#[derive(Default)]
pub struct Meters {
    /// Highest processed sample since the UI last looked (linear).
    peak: AtomicF32,
    /// Audio glitches since start, by kind.
    glitches: [AtomicU32; 3],
}

/// Where audio was lost.
#[derive(Clone, Copy, Debug)]
pub enum Glitch {
    /// Processing fell behind and microphone audio was dropped. Affects recordings.
    InputOverflow,
    /// The output driver reported an underrun. Only affects what you hear.
    OutputXrun,
    /// Processed audio did not arrive in time for the output. Only affects what you hear.
    OutputStarved,
}

/// Glitch totals, split by what they affect.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Dropouts {
    pub input: u32,
    pub output: u32,
}

impl Meters {
    pub fn record_peak(&self, peak: f32) {
        self.peak.fetch_max(peak);
    }

    pub fn take_peak(&self) -> f32 {
        self.peak.take()
    }

    pub fn record_glitch(&self, glitch: Glitch) {
        self.glitches[glitch as usize].fetch_add(1, Relaxed);
    }

    pub fn glitches(&self, glitch: Glitch) -> u32 {
        self.glitches[glitch as usize].load(Relaxed)
    }

    pub fn dropouts(&self) -> Dropouts {
        Dropouts {
            input: self.glitches(Glitch::InputOverflow),
            output: self.glitches(Glitch::OutputXrun) + self.glitches(Glitch::OutputStarved),
        }
    }
}
