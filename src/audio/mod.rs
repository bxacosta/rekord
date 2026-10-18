//! Everything that talks to audio hardware.

mod controls;
pub mod devices;
mod engine;
mod session;

pub use devices::{DeviceChoice, Direction};
pub use engine::{Connection, Engine};
