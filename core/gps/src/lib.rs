//! Virtual GPS state, NMEA generation, and movement simulation.

pub mod manager;
pub mod nmea;

pub use manager::{
    GpsManager, GpxTrackPoint, MovementModel, GPS_BAUD_RATE, GPS_RX_PIN, GPS_TX_PIN,
    UART_BYTES_PER_SECOND,
};
pub use nmea::GpsState;
