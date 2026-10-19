//! Fixed delay in whole frames, to keep signals aligned with the denoiser output.

use super::{FRAME_LEN, Frame};
use std::collections::VecDeque;

pub struct FrameDelay {
    frames: VecDeque<Frame>,
}

impl FrameDelay {
    pub fn new(frames: usize) -> Self {
        Self { frames: VecDeque::from(vec![[0.0; FRAME_LEN]; frames]) }
    }

    /// Takes in `frame` and returns the one from `frames` frames ago.
    pub fn process(&mut self, frame: &Frame) -> Frame {
        self.frames.push_back(*frame);
        self.frames.pop_front().expect("the queue is never empty after a push")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn delays_by_whole_frames() {
        let mut delay = FrameDelay::new(2);
        let outputs: Vec<f32> = (1..=4).map(|i| delay.process(&[i as f32; FRAME_LEN])[0]).collect();
        assert_eq!(outputs, [0.0, 0.0, 1.0, 2.0]);
        assert_eq!(FrameDelay::new(0).process(&[5.0; FRAME_LEN])[0], 5.0);
    }
}
