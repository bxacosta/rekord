//! Everything that talks to audio hardware.

mod controls;
pub mod devices;
mod engine;
mod processing;
pub mod session;
mod streams;

pub use controls::Dropouts;
pub use devices::{DeviceChoice, Direction};
pub use engine::{Connection, Engine};
