//! Deterministic propagation and LoRa airtime calculations.

use crate::types::RadioChannel;

/// The environment correction used by the Okumura-Hata model.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HataEnvironment {
    Urban,
    Suburban,
}

/// Path-loss model used by [`crate::RadioBus`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum PropagationModel {
    /// Ideal line-of-sight propagation. Useful for controlled tests.
    FreeSpace,
    /// Ground-reflection model using antenna heights above ground.
    TwoRayGround { tx_height_m: f64, rx_height_m: f64 },
    /// Okumura-Hata empirical terrestrial model.
    OkumuraHata {
        environment: HataEnvironment,
        base_height_m: f64,
        mobile_height_m: f64,
    },
    /// A deterministic distance cutoff, independent of link budget.
    FixedRange { max_distance_km: f64 },
}

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

/// Calculates two-ray ground path loss, bounded by free-space loss in the
/// near field where the asymptotic two-ray equation is not applicable.
pub fn two_ray_ground_path_loss(
    distance_km: f64,
    freq_mhz: f64,
    tx_height_m: f64,
    rx_height_m: f64,
) -> f64 {
    if distance_km <= 0.0 {
        return 0.0;
    }
    if tx_height_m <= 0.0 || rx_height_m <= 0.0 {
        return f64::INFINITY;
    }

    let distance_m = distance_km * 1_000.0;
    let asymptotic_loss =
        40.0 * distance_m.log10() - 20.0 * tx_height_m.log10() - 20.0 * rx_height_m.log10();
    asymptotic_loss.max(free_space_path_loss(distance_km, freq_mhz))
}

/// Calculates Okumura-Hata path loss. Free-space loss is used below one
/// kilometre, outside the empirical model's distance range.
pub fn okumura_hata_path_loss(
    distance_km: f64,
    freq_mhz: f64,
    base_height_m: f64,
    mobile_height_m: f64,
    environment: HataEnvironment,
) -> f64 {
    if distance_km <= 0.0 {
        return 0.0;
    }
    if distance_km < 1.0 {
        return free_space_path_loss(distance_km, freq_mhz);
    }
    if freq_mhz <= 0.0 || base_height_m <= 0.0 || mobile_height_m <= 0.0 {
        return f64::INFINITY;
    }

    let log_frequency = freq_mhz.log10();
    let mobile_correction =
        (1.1 * log_frequency - 0.7) * mobile_height_m - (1.56 * log_frequency - 0.8);
    let urban = 69.55 + 26.16 * log_frequency - 13.82 * base_height_m.log10() - mobile_correction
        + (44.9 - 6.55 * base_height_m.log10()) * distance_km.log10();

    match environment {
        HataEnvironment::Urban => urban,
        HataEnvironment::Suburban => urban - 2.0 * (freq_mhz / 28.0).log10().powi(2) - 5.4,
    }
}

/// Calculates path loss for the selected model. `None` means that a fixed
/// range cutoff rejected the link.
pub fn path_loss(model: PropagationModel, distance_km: f64, freq_mhz: f64) -> Option<f64> {
    match model {
        PropagationModel::FreeSpace => Some(free_space_path_loss(distance_km, freq_mhz)),
        PropagationModel::TwoRayGround {
            tx_height_m,
            rx_height_m,
        } => Some(two_ray_ground_path_loss(
            distance_km,
            freq_mhz,
            tx_height_m,
            rx_height_m,
        )),
        PropagationModel::OkumuraHata {
            environment,
            base_height_m,
            mobile_height_m,
        } => Some(okumura_hata_path_loss(
            distance_km,
            freq_mhz,
            base_height_m,
            mobile_height_m,
            environment,
        )),
        PropagationModel::FixedRange { max_distance_km } => {
            (distance_km <= max_distance_km).then_some(0.0)
        }
    }
}

/// Calculates received signal strength for the legacy free-space link budget.
pub fn received_rssi(
    tx_power_dbm: f64,
    distance_km: f64,
    freq_mhz: f64,
    antenna_gain_tx_dbi: f64,
    antenna_gain_rx_dbi: f64,
) -> f64 {
    received_rssi_with_model(
        tx_power_dbm,
        distance_km,
        freq_mhz,
        antenna_gain_tx_dbi,
        antenna_gain_rx_dbi,
        0.0,
        PropagationModel::FreeSpace,
    )
    .unwrap_or(f64::NEG_INFINITY)
}

/// Calculates received signal strength for a configurable link budget.
pub fn received_rssi_with_model(
    tx_power_dbm: f64,
    distance_km: f64,
    freq_mhz: f64,
    antenna_gain_tx_dbi: f64,
    antenna_gain_rx_dbi: f64,
    system_loss_db: f64,
    model: PropagationModel,
) -> Option<f64> {
    let path_loss = path_loss(model, distance_km, freq_mhz)?;
    Some(tx_power_dbm - path_loss + antenna_gain_tx_dbi + antenna_gain_rx_dbi - system_loss_db)
}

/// Returns the SX1262 receiver sensitivity threshold for a channel.
pub fn sensitivity(channel: &RadioChannel) -> f64 {
    let base = match channel.spreading_factor {
        7 => -123.0,
        8 => -126.0,
        9 => -129.0,
        10 => -132.0,
        11 => -134.5,
        12 => -137.0,
        _ => return f64::INFINITY,
    };
    let bandwidth_penalty = 10.0 * (channel.bandwidth_khz as f64 / 125.0).log10();

    base + bandwidth_penalty
}

/// Calculates LoRa packet airtime in microseconds using the Semtech formula.
///
/// CRC is enabled. `coding_rate` is the denominator in 4/5 through 4/8.
pub fn airtime_us(
    packet_bytes: usize,
    sf: u8,
    bw_khz: u16,
    coding_rate: u8,
    preamble_len: u16,
    explicit_header: bool,
) -> u64 {
    if !(7..=12).contains(&sf)
        || !matches!(bw_khz, 125 | 250 | 500)
        || !(5..=8).contains(&coding_rate)
    {
        return 0;
    }

    let symbol_duration_us = (1_u64 << sf) * 1_000 / u64::from(bw_khz);
    let low_data_rate_optimization = symbol_duration_us > 16_000;
    let implicit_header = !explicit_header;
    let numerator = 8_i128 * packet_bytes as i128 - 4 * i128::from(sf) + 28 + 16
        - 20 * i128::from(implicit_header);
    let denominator = 4 * (i128::from(sf) - 2 * i128::from(low_data_rate_optimization));
    let encoded_blocks = if numerator > 0 {
        (numerator + denominator - 1) / denominator
    } else {
        0
    };
    let payload_symbols = 8 + encoded_blocks as u64 * u64::from(coding_rate);

    let preamble_quarter_symbols = u64::from(preamble_len) * 4 + 17;
    let total_quarter_symbols = preamble_quarter_symbols + payload_symbols * 4;
    total_quarter_symbols * symbol_duration_us / 4
}

/// Calculates LoRa packet airtime rounded up to whole milliseconds.
pub fn airtime_ms(
    packet_bytes: usize,
    sf: u8,
    bw_khz: u16,
    coding_rate: u8,
    preamble_len: u16,
    explicit_header: bool,
) -> u32 {
    let microseconds = airtime_us(
        packet_bytes,
        sf,
        bw_khz,
        coding_rate,
        preamble_len,
        explicit_header,
    );
    microseconds.div_ceil(1_000).min(u64::from(u32::MAX)) as u32
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
    fn propagation_models_and_system_loss_change_the_link_budget() {
        let free_space = received_rssi_with_model(
            14.0,
            10.0,
            868.0,
            0.0,
            0.0,
            0.0,
            PropagationModel::FreeSpace,
        )
        .unwrap();
        let two_ray = received_rssi_with_model(
            14.0,
            10.0,
            868.0,
            0.0,
            0.0,
            6.0,
            PropagationModel::TwoRayGround {
                tx_height_m: 1.5,
                rx_height_m: 1.5,
            },
        )
        .unwrap();
        assert!(two_ray < free_space - 20.0);

        assert!(received_rssi_with_model(
            14.0,
            2.0,
            868.0,
            0.0,
            0.0,
            0.0,
            PropagationModel::FixedRange {
                max_distance_km: 1.0
            },
        )
        .is_none());
    }

    #[test]
    fn okumura_hata_suburban_loss_is_below_urban_loss() {
        let urban = okumura_hata_path_loss(5.0, 868.0, 30.0, 1.5, HataEnvironment::Urban);
        let suburban = okumura_hata_path_loss(5.0, 868.0, 30.0, 1.5, HataEnvironment::Suburban);

        assert!(urban > suburban);
        assert!((urban - 150.62).abs() < 0.1);
    }

    #[test]
    fn sensitivity_uses_sx1262_threshold_without_extra_margin() {
        assert!((sensitivity(&channel(125)) - -123.0).abs() < f64::EPSILON);
        assert!((sensitivity(&channel(250)) - -119.9897).abs() < 0.001);
    }

    #[test]
    fn airtime_matches_semtech_reference_vectors_exactly() {
        assert_eq!(airtime_us(16, 7, 125, 5, 8, true), 51_456);
        assert_eq!(airtime_us(16, 12, 125, 5, 8, true), 1_318_912);
        assert_eq!(airtime_us(255, 7, 125, 5, 8, true), 399_616);

        assert_eq!(airtime_ms(16, 7, 125, 5, 8, true), 52);
        assert_eq!(airtime_ms(16, 12, 125, 5, 8, true), 1_319);
        assert_eq!(airtime_ms(255, 7, 125, 5, 8, true), 400);
    }

    #[test]
    fn invalid_modulation_settings_do_not_shift_or_panic() {
        assert_eq!(airtime_us(16, 64, 125, 5, 8, true), 0);
        assert_eq!(airtime_us(16, 7, 0, 5, 8, true), 0);
    }
}
