//! Level meter ballistics: instant rise, smooth fall, and a short peak hold.

use crate::dsp::dynamics::gain_to_db;
use std::time::{Duration, Instant};

pub const FLOOR_DB: f32 = -60.0;
const FALL_DB_PER_SECOND: f32 = 24.0;
const HOLD: Duration = Duration::from_millis(1500);

pub struct LevelMeter {
    level_db: f32,
    hold_db: f32,
    hold_since: Instant,
    updated: Instant,
}

impl LevelMeter {
    pub fn new() -> Self {
        let now = Instant::now();
        Self { level_db: FLOOR_DB, hold_db: FLOOR_DB, hold_since: now, updated: now }
    }

    /// Feeds the peak (linear) measured since the previous update.
    pub fn update(&mut self, peak: f32) {
        let now = Instant::now();
        let dt = now.duration_since(self.updated).as_secs_f32();
        self.updated = now;

        let db = gain_to_db(peak).max(FLOOR_DB);
        self.level_db = db.max(self.level_db - FALL_DB_PER_SECOND * dt);
        if db >= self.hold_db || now.duration_since(self.hold_since) > HOLD {
            self.hold_db = db;
            self.hold_since = now;
        }
    }

    pub fn reset(&mut self) {
        *self = Self::new();
    }

    pub fn level_db(&self) -> f32 {
        self.level_db
    }

    pub fn hold_db(&self) -> f32 {
        self.hold_db
    }
}

/// Position of `db` on the meter, from 0 (floor) to 1 (full scale).
pub fn fraction(db: f32) -> f32 {
    ((db - FLOOR_DB) / -FLOOR_DB).clamp(0.0, 1.0)
}
