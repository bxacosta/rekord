//! Feed-forward peak compressor with a soft knee. Also used as a limiter.

#[derive(Clone, Debug)]
pub struct Compressor {
    threshold_db: f32,
    slope: f32,
    knee_db: f32,
    attack: f32,
    release: f32,
    /// Current gain change in dB (always <= 0).
    reduction_db: f32,
}

impl Compressor {
    pub fn new(sample_rate: f32, threshold_db: f32, ratio: f32, knee_db: f32, attack_s: f32, release_s: f32) -> Self {
        Self {
            threshold_db,
            slope: 1.0 / ratio - 1.0,
            knee_db,
            attack: smoothing_coefficient(sample_rate, attack_s),
            release: smoothing_coefficient(sample_rate, release_s),
            reduction_db: 0.0,
        }
    }

    /// A transparent safety limiter. Attack and release are deliberately not too fast:
    /// very fast settings audibly distort speech.
    pub fn limiter(sample_rate: f32) -> Self {
        Self::new(sample_rate, -4.0, 20.0, 3.0, 0.003, 0.25)
    }

    #[inline]
    pub fn process(&mut self, x: f32) -> f32 {
        let level_db = 20.0 * (x.abs() + 1e-9).log10();
        let target = self.static_curve(level_db - self.threshold_db);
        let coef = if target < self.reduction_db { self.attack } else { self.release };
        self.reduction_db = coef * self.reduction_db + (1.0 - coef) * target;
        x * db_to_gain(self.reduction_db)
    }

    /// Gain change in dB for a signal `over` dB above the threshold.
    fn static_curve(&self, over: f32) -> f32 {
        if 2.0 * over < -self.knee_db {
            0.0
        } else if 2.0 * over.abs() <= self.knee_db {
            self.slope * (over + self.knee_db / 2.0).powi(2) / (2.0 * self.knee_db)
        } else {
            self.slope * over
        }
    }

    pub fn reset(&mut self) {
        self.reduction_db = 0.0;
    }
}

fn smoothing_coefficient(sample_rate: f32, seconds: f32) -> f32 {
    (-1.0 / (seconds * sample_rate)).exp()
}

#[inline]
pub fn db_to_gain(db: f32) -> f32 {
    10f32.powf(db / 20.0)
}

#[inline]
pub fn gain_to_db(gain: f32) -> f32 {
    20.0 * gain.max(1e-6).log10()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quiet_signals_pass_untouched() {
        let mut c = Compressor::limiter(48_000.0);
        for _ in 0..48_000 {
            assert!((c.process(0.1) - 0.1).abs() < 1e-4);
        }
    }

    #[test]
    fn limiter_holds_loud_signals_near_threshold() {
        let mut c = Compressor::limiter(48_000.0);
        let mut y = 0.0;
        for _ in 0..48_000 {
            y = c.process(1.0);
        }
        // 0 dBFS in, threshold -4 dB at 20:1 -> about -3.8 dBFS out.
        assert!((gain_to_db(y) + 3.8).abs() < 0.3, "got {} dB", gain_to_db(y));
    }
}
