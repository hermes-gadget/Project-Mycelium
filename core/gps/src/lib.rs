//! Virtual GPS state, NMEA generation, and movement simulation.

pub mod manager;
pub mod nmea;

pub use manager::{GpsManager, MovementModel};
pub use nmea::GpsState;
