//! "Voice enhance": an EQ that carves out mud and nasality and lifts intelligibility,
//! followed by gentle compression so quiet and loud passages sit closer together.

use super::biquad::Biquad;
use super::dynamics::{Compressor, db_to_gain};

const THRESHOLD_DB: f32 = -30.0;
const RATIO: f32 = 4.0;

pub struct VoiceEnhancer {
    eq: [Biquad; 5],
    compressor: Compressor,
    makeup: f32,
}

impl VoiceEnhancer {
    pub fn new(sample_rate: f32) -> Self {
        Self {
            eq: [
                Biquad::high_pass(sample_rate, 100.0, 0.7),        // rumble and handling noise
                Biquad::peaking(sample_rate, 300.0, 1.0, -2.0),    // boxiness
                Biquad::peaking(sample_rate, 1000.0, 1.4, -4.0),   // nasal tone
                Biquad::peaking(sample_rate, 3000.0, 1.0, 4.0),    // consonants: where words are understood
                Biquad::high_shelf(sample_rate, 5000.0, 1.0, 3.0), // a little air
            ],
            compressor: Compressor::new(sample_rate, THRESHOLD_DB, RATIO, 10.0, 0.005, 0.2),
            // Automatic makeup gain, the same rule Web Audio's DynamicsCompressor uses.
            makeup: db_to_gain(0.6 * (-THRESHOLD_DB + THRESHOLD_DB / RATIO)),
        }
    }

    #[inline]
    pub fn process(&mut self, x: f32) -> f32 {
        let shaped = self.eq.iter_mut().fold(x, |acc, filter| filter.process(acc));
        self.compressor.process(shaped) * self.makeup
    }

    pub fn reset(&mut self) {
        self.eq.iter_mut().for_each(Biquad::reset);
        self.compressor.reset();
    }
}
