//! Click-free parameter changes: values glide linearly across one frame instead of jumping.

#[derive(Clone, Copy, Debug)]
pub struct Ramp {
    value: f32,
}

impl Ramp {
    pub fn new(value: f32) -> Self {
        Self { value }
    }

    pub fn value(&self) -> f32 {
        self.value
    }

    /// Moves to `target` over `len` samples, yielding the value to use for each sample.
    pub fn glide(&mut self, target: f32, len: usize) -> impl Iterator<Item = f32> + use<> {
        let start = self.value;
        self.value = target;
        let step = (target - start) / len as f32;
        (1..=len).map(move |i| start + step * i as f32)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn glides_to_target_and_stays_there() {
        let mut ramp = Ramp::new(0.0);
        let values: Vec<f32> = ramp.glide(1.0, 4).collect();
        assert_eq!(values, [0.25, 0.5, 0.75, 1.0]);
        assert!(ramp.glide(1.0, 4).all(|v| v == 1.0));
    }
}
