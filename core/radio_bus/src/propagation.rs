// Free-space path loss model
// L = 20*log10(d_km) + 20*log10(f_mhz) + 32.45
// RSSI = tx_power - L + antenna_gain_tx + antenna_gain_rx

use crate::types::RadioChannel;

/// Calculates the great-circle distance between two latitude/longitude pairs.
pub fn distance_km(pos1: (f64, f64), pos2: (f64, f64)) -> f64 {
    let (lat1, lon1) = pos1;
    let (lat2, lon2) = pos2;
    let earth_radius_km = 6_371.0;
    let dlat = (lat2 - lat1).to_radians();
    let dlon = (lon2 - lon1).to_radians();
    let a = (dlat / 2.0).sin().powi(2)
        + lat1.to_radians().cos() * lat2.to_radians().cos() * (dlon / 2.0).sin().powi(2);
    let c = 2.0 * a.clamp(0.0, 1.0).sqrt().asin();

    earth_radius_km * c
}

/// Calculates free-space path loss in dB.
pub fn free_space_path_loss(distance_km: f64, freq_mhz: f64) -> f64 {
    if distance_km <= 0.0 {
        return 0.0;
    }

    20.0 * distance_km.log10() + 20.0 * freq_mhz.log10() + 32.45
}

/// Calculates received signal strength for a free-space link budget.
pub fn received_rssi(
    tx_power_dbm: f64,
    distance_km: f64,
    freq_mhz: f64,
    antenna_gain_tx_dbi: f64,
    antenna_gain_rx_dbi: f64,
) -> f64 {
    let path_loss = free_space_path_loss(distance_km, freq_mhz);
    tx_power_dbm - path_loss + antenna_gain_tx_dbi + antenna_gain_rx_dbi
}

/// Returns the receiver sensitivity threshold for a channel.
pub fn sensitivity(channel: &RadioChannel) -> f64 {
    let base = match channel.spreading_factor {
        7 => -123.0,
        8 => -126.0,
        9 => -129.0,
        10 => -132.0,
        11 => -134.5,
        12 => -137.0,
        _ => -120.0,
    };
    let bandwidth_penalty = 10.0 * (channel.bandwidth_khz as f64 / 125.0).log10();

    base + bandwidth_penalty - 3.0
}

/// Estimates LoRa packet airtime in milliseconds.
pub fn airtime_ms(
    packet_bytes: usize,
    sf: u8,
    bw_khz: u16,
    coding_rate: u8,
    preamble_len: u16,
    explicit_header: bool,
) -> u32 {
    let sf_f = sf as f64;
    let bandwidth_hz = bw_khz as f64 * 1_000.0;
    let coding_rate = (coding_rate as f64).max(1.0);

    let symbol_duration = (1_u64 << sf) as f64 / bandwidth_hz;
    let preamble_symbols = preamble_len as f64 + 4.25;
    let preamble_duration = preamble_symbols * symbol_duration;

    let payload_symbols_numerator = if explicit_header {
        8.0 * packet_bytes as f64 - 4.0 * sf_f + 28.0 + 16.0 - 20.0
    } else {
        8.0 * packet_bytes as f64 - 4.0 * sf_f + 28.0
    };
    let payload_symbols =
        8.0 + (payload_symbols_numerator.max(0.0) / (4.0 * (sf_f - 2.0))).ceil() * coding_rate;
    let payload_duration = payload_symbols * symbol_duration;

    ((preamble_duration + payload_duration) * 1_000.0) as u32
}

#[cfg(test)]
mod tests {
    use super::*;

    fn channel(bandwidth_khz: u16) -> RadioChannel {
        RadioChannel {
            freq_mhz: 915.0,
            bandwidth_khz,
            spreading_factor: 7,
            coding_rate: 5,
        }
    }

    #[test]
    fn distance_between_london_and_paris_matches_known_distance() {
        let london = (51.5074, -0.1278);
        let paris = (48.8566, 2.3522);

        assert!((distance_km(london, paris) - 343.6).abs() < 0.5);
        assert_eq!(distance_km(london, london), 0.0);
    }

    #[test]
    fn path_loss_at_one_kilometre_and_915_mhz_is_known_value() {
        assert!((free_space_path_loss(1.0, 915.0) - 91.68).abs() < 0.01);
        assert_eq!(free_space_path_loss(0.0, 915.0), 0.0);
    }

    #[test]
    fn rssi_applies_path_loss_and_both_antenna_gains() {
        let rssi = received_rssi(14.0, 1.0, 915.0, 2.0, 3.0);
        assert!((rssi - -72.68).abs() < 0.01);
    }

    #[test]
    fn wider_bandwidth_raises_the_sensitivity_threshold() {
        assert!((sensitivity(&channel(125)) - -126.0).abs() < f64::EPSILON);
        assert!((sensitivity(&channel(250)) - -122.9897).abs() < 0.001);
    }

    #[test]
    fn airtime_matches_known_lora_parameters() {
        assert_eq!(airtime_ms(16, 7, 125, 5, 8, true), 56);
        assert_eq!(airtime_ms(16, 12, 125, 5, 8, true), 1_155);
    }
}
