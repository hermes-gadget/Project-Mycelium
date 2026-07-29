// Core types for the radio simulation

/// Radio channel parameters (matches LoRa modulation settings).
#[derive(Debug, Clone, PartialEq)]
pub struct RadioChannel {
    /// Center frequency, commonly 433, 868, or 915 MHz.
    pub freq_mhz: f64,
    /// Channel bandwidth in kHz.
    pub bandwidth_khz: u16,
    /// LoRa spreading factor (7-12).
    pub spreading_factor: u8,
    /// LoRa coding-rate denominator (5-8, representing 4/5 to 4/8).
    pub coding_rate: u8,
}

/// A transmission event from a node.
#[derive(Debug, Clone)]
pub struct TxEvent {
    pub node_id: String,
    pub channel: RadioChannel,
    pub data: Vec<u8>,
    pub tx_power_dbm: f64,
    pub airtime_ms: u32,
    /// `(latitude, longitude)` in decimal degrees.
    pub position: (f64, f64),
    pub timestamp_ms: u64,
}

/// A received packet delivered to a node.
#[derive(Debug, Clone)]
pub struct RxPacket {
    pub from_node: String,
    pub data: Vec<u8>,
    pub rssi_dbm: f64,
    pub snr_db: f64,
    pub channel: RadioChannel,
    pub timestamp_ms: u64,
}

/// Sensitivity thresholds for different SF values at 125 kHz bandwidth.
pub const SENSITIVITY_DBM: &[(u8, f64)] = &[
    (7, -123.0),
    (8, -126.0),
    (9, -129.0),
    (10, -132.0),
    (11, -134.5),
    (12, -137.0),
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sensitivity_table_covers_all_supported_spreading_factors() {
        let spreading_factors: Vec<_> = SENSITIVITY_DBM.iter().map(|(sf, _)| *sf).collect();
        assert_eq!(spreading_factors, vec![7, 8, 9, 10, 11, 12]);
    }

    #[test]
    fn radio_channel_equality_includes_every_modulation_parameter() {
        let channel = RadioChannel {
            freq_mhz: 915.0,
            bandwidth_khz: 125,
            spreading_factor: 7,
            coding_rate: 5,
        };
        let mut other = channel.clone();

        assert_eq!(channel, other);
        other.spreading_factor = 8;
        assert_ne!(channel, other);
    }
}
