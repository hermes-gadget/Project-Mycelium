// Core types for the radio simulation

pub const SX1262_DEFAULT_FREQUENCY_MHZ: f64 = 868.0;
pub const SX1262_DEFAULT_BANDWIDTH_KHZ: u16 = 125;
pub const SX1262_DEFAULT_SPREADING_FACTOR: u8 = 9;
pub const SX1262_DEFAULT_CODING_RATE: u8 = 5;
pub const SX1262_DEFAULT_TX_POWER_DBM: f64 = 14.0;
pub const SX1262_DEFAULT_TCXO_VOLTAGE_V: f32 = 1.8;

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

impl RadioChannel {
    /// Creates a channel when all settings are supported by an SX1262.
    pub fn new(
        freq_mhz: f64,
        bandwidth_khz: u16,
        spreading_factor: u8,
        coding_rate: u8,
    ) -> Option<Self> {
        (freq_mhz.is_finite()
            && (150.0..=960.0).contains(&freq_mhz)
            && matches!(bandwidth_khz, 125 | 250 | 500)
            && (7..=12).contains(&spreading_factor)
            && (5..=8).contains(&coding_rate))
        .then_some(Self {
            freq_mhz,
            bandwidth_khz,
            spreading_factor,
            coding_rate,
        })
    }
}

impl Default for RadioChannel {
    fn default() -> Self {
        Self {
            freq_mhz: SX1262_DEFAULT_FREQUENCY_MHZ,
            bandwidth_khz: SX1262_DEFAULT_BANDWIDTH_KHZ,
            spreading_factor: SX1262_DEFAULT_SPREADING_FACTOR,
            coding_rate: SX1262_DEFAULT_CODING_RATE,
        }
    }
}

/// SX1262 configuration and board-control state established at initialization.
#[derive(Debug, Clone, PartialEq)]
pub struct Sx1262State {
    pub channel: RadioChannel,
    pub tx_power_dbm: f64,
    /// DIO2 controls the external RF switch on the T-Deck radio board.
    pub dio2_rf_switch_enabled: bool,
    /// DIO3 supplies the TCXO at the configured voltage.
    pub dio3_tcxo_voltage_v: Option<f32>,
}

impl Sx1262State {
    pub fn new(channel: RadioChannel, tx_power_dbm: f64) -> Option<Self> {
        (tx_power_dbm.is_finite() && (-17.0..=22.0).contains(&tx_power_dbm)).then_some(Self {
            channel,
            tx_power_dbm,
            dio2_rf_switch_enabled: true,
            dio3_tcxo_voltage_v: Some(SX1262_DEFAULT_TCXO_VOLTAGE_V),
        })
    }
}

impl Default for Sx1262State {
    fn default() -> Self {
        Self {
            channel: RadioChannel::default(),
            tx_power_dbm: SX1262_DEFAULT_TX_POWER_DBM,
            dio2_rf_switch_enabled: true,
            dio3_tcxo_voltage_v: Some(SX1262_DEFAULT_TCXO_VOLTAGE_V),
        }
    }
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

    #[test]
    fn sx1262_defaults_match_the_t_deck_radio_configuration() {
        let state = Sx1262State::default();

        assert_eq!(state.channel.freq_mhz, 868.0);
        assert_eq!(state.channel.bandwidth_khz, 125);
        assert_eq!(state.channel.spreading_factor, 9);
        assert_eq!(state.channel.coding_rate, 5);
        assert_eq!(state.tx_power_dbm, 14.0);
        assert!(state.dio2_rf_switch_enabled);
        assert_eq!(state.dio3_tcxo_voltage_v, Some(1.8));
    }

    #[test]
    fn constructors_reject_settings_outside_sx1262_limits() {
        assert!(RadioChannel::new(868.0, 125, 9, 5).is_some());
        assert!(RadioChannel::new(149.9, 125, 9, 5).is_none());
        assert!(RadioChannel::new(868.0, 62, 9, 5).is_none());
        assert!(RadioChannel::new(868.0, 125, 13, 5).is_none());
        assert!(RadioChannel::new(868.0, 125, 9, 9).is_none());

        let channel = RadioChannel::default();
        assert!(Sx1262State::new(channel.clone(), 22.0).is_some());
        assert!(Sx1262State::new(channel, 22.1).is_none());
    }
}
