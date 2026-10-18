//! Second-order IIR filters (RBJ "Audio EQ Cookbook"), transposed direct form II.

use std::f32::consts::PI;

#[derive(Clone, Copy, Debug)]
pub struct Biquad {
    b0: f32,
    b1: f32,
    b2: f32,
    a1: f32,
    a2: f32,
    z1: f32,
    z2: f32,
}

impl Biquad {
    fn normalized(b0: f32, b1: f32, b2: f32, a0: f32, a1: f32, a2: f32) -> Self {
        Self { b0: b0 / a0, b1: b1 / a0, b2: b2 / a0, a1: a1 / a0, a2: a2 / a0, z1: 0.0, z2: 0.0 }
    }

    pub fn high_pass(sample_rate: f32, freq: f32, q: f32) -> Self {
        let (cos, alpha) = cos_alpha(sample_rate, freq, q);
        Self::normalized((1.0 + cos) / 2.0, -(1.0 + cos), (1.0 + cos) / 2.0, 1.0 + alpha, -2.0 * cos, 1.0 - alpha)
    }

    pub fn peaking(sample_rate: f32, freq: f32, q: f32, gain_db: f32) -> Self {
        let (cos, alpha) = cos_alpha(sample_rate, freq, q);
        let a = 10f32.powf(gain_db / 40.0);
        Self::normalized(1.0 + alpha * a, -2.0 * cos, 1.0 - alpha * a, 1.0 + alpha / a, -2.0 * cos, 1.0 - alpha / a)
    }

    pub fn high_shelf(sample_rate: f32, freq: f32, slope: f32, gain_db: f32) -> Self {
        let (sin, cos) = (2.0 * PI * freq / sample_rate).sin_cos();
        let a = 10f32.powf(gain_db / 40.0);
        let alpha = sin / 2.0 * ((a + 1.0 / a) * (1.0 / slope - 1.0) + 2.0).sqrt();
        let k = 2.0 * a.sqrt() * alpha;
        Self::normalized(
            a * ((a + 1.0) + (a - 1.0) * cos + k),
            -2.0 * a * ((a - 1.0) + (a + 1.0) * cos),
            a * ((a + 1.0) + (a - 1.0) * cos - k),
            (a + 1.0) - (a - 1.0) * cos + k,
            2.0 * ((a - 1.0) - (a + 1.0) * cos),
            (a + 1.0) - (a - 1.0) * cos - k,
        )
    }

    #[inline]
    pub fn process(&mut self, x: f32) -> f32 {
        let y = self.b0 * x + self.z1;
        self.z1 = self.b1 * x - self.a1 * y + self.z2;
        self.z2 = self.b2 * x - self.a2 * y;
        y
    }

    pub fn reset(&mut self) {
        self.z1 = 0.0;
        self.z2 = 0.0;
    }
}

fn cos_alpha(sample_rate: f32, freq: f32, q: f32) -> (f32, f32) {
    let (sin, cos) = (2.0 * PI * freq / sample_rate).sin_cos();
    (cos, sin / (2.0 * q))
}

#[cfg(test)]
mod tests {
    use super::*;

    const SR: f32 = 48_000.0;

    /// Steady-state gain of `filter` for a sine at `freq`, in dB.
    fn gain_db(mut filter: Biquad, freq: f32) -> f32 {
        let n = SR as usize;
        let mut peak = 0f32;
        for i in 0..n {
            let y = filter.process((2.0 * PI * freq * i as f32 / SR).sin());
            if i > n / 2 {
                peak = peak.max(y.abs());
            }
        }
        20.0 * peak.log10()
    }

    #[test]
    fn high_pass_removes_rumble_and_keeps_voice() {
        let hp = Biquad::high_pass(SR, 100.0, 0.7);
        assert!(gain_db(hp, 30.0) < -15.0);
        assert!(gain_db(hp, 1000.0).abs() < 0.5);
    }

    #[test]
    fn peaking_boosts_its_center_frequency() {
        let eq = Biquad::peaking(SR, 3000.0, 1.0, 4.0);
        assert!((gain_db(eq, 3000.0) - 4.0).abs() < 0.3);
        assert!(gain_db(eq, 200.0).abs() < 0.5);
    }

    #[test]
    fn high_shelf_lifts_the_top_end() {
        let shelf = Biquad::high_shelf(SR, 5000.0, 1.0, 3.0);
        assert!((gain_db(shelf, 15_000.0) - 3.0).abs() < 0.5);
        assert!(gain_db(shelf, 300.0).abs() < 0.3);
    }
}
