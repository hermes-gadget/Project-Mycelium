use std::collections::VecDeque;
use std::f64::consts::PI;

use chrono::Utc;

use crate::nmea::GpsState;

const EARTH_RADIUS_M: f64 = 6_371_000.0;
const MPS_TO_KNOTS: f64 = 1.943_844_492_440_6;
const EPOCH_MS: u64 = 1_000;

/// L76K UART baud rate used by default.
pub const GPS_BAUD_RATE: u32 = 9_600;
/// Alternate L76K baud rate used by the real receiver's baud cycling.
pub const GPS_BAUD_RATE_FAST: u32 = 38_400;
/// Lowest baud rate accepted by [`GpsManager::set_baud_rate`].
pub const MIN_GPS_BAUD_RATE: u32 = 4_800;
/// Highest baud rate accepted by [`GpsManager::set_baud_rate`].
pub const MAX_GPS_BAUD_RATE: u32 = 115_200;
/// L76K UART TX pin on the emulated board.
pub const GPS_TX_PIN: u8 = 43;
/// L76K UART RX pin on the emulated board.
pub const GPS_RX_PIN: u8 = 44;
/// 9600 baud with 8N1 framing transfers approximately 960 bytes per second.
pub const UART_BYTES_PER_SECOND: usize = GPS_BAUD_RATE as usize / 10;

/// Bytes transferred per second at `baud_rate` with 8N1 framing.
pub fn uart_bytes_per_second(baud_rate: u32) -> usize {
    (baud_rate / 10) as usize
}

/// A GPX track sample with an optional source timestamp.
#[derive(Clone, Debug, PartialEq)]
pub struct GpxTrackPoint {
    pub latitude: f64,
    pub longitude: f64,
    /// Milliseconds on the source track's timeline.
    pub timestamp_ms: Option<i64>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum MovementModel {
    Static,
    Linear {
        speed_ms: f64,
        heading_deg: f64,
    },
    Waypoint {
        points: Vec<(f64, f64)>,
        speed_ms: f64,
        current_idx: usize,
    },
    GpxReplay {
        points: Vec<(f64, f64)>,
        speed_multiplier: f64,
        current_idx: usize,
    },
    GpxReplayWithTimestamps {
        points: Vec<GpxTrackPoint>,
        speed_multiplier: f64,
        current_idx: usize,
    },
}

pub struct GpsManager {
    state: GpsState,
    movement: MovementModel,
    sentence_buffer: VecDeque<u8>,
    replay_progress: f64,
    epoch_elapsed_ms: u64,
    uart_bytes_remaining: usize,
    baud_rate: u32,
    output_enabled: bool,
}

impl GpsManager {
    pub fn new(lat: f64, lon: f64) -> Self {
        Self {
            state: GpsState::new(lat, lon),
            movement: MovementModel::Static,
            sentence_buffer: VecDeque::new(),
            replay_progress: 0.0,
            epoch_elapsed_ms: 0,
            uart_bytes_remaining: 0,
            baud_rate: GPS_BAUD_RATE,
            output_enabled: true,
        }
    }

    pub fn state(&self) -> &GpsState {
        &self.state
    }

    pub fn state_mut(&mut self) -> &mut GpsState {
        &mut self.state
    }

    /// Configure the virtual UART baud rate.
    ///
    /// The real L76K cycles between 9600 and 38400 baud. Returns `false` for
    /// rates the receiver cannot use; the current rate is left unchanged.
    /// Reconfiguring restarts output at a sentence boundary, matching a
    /// receiver restart.
    pub fn set_baud_rate(&mut self, baud_rate: u32) -> bool {
        if !(MIN_GPS_BAUD_RATE..=MAX_GPS_BAUD_RATE).contains(&baud_rate) {
            return false;
        }
        if self.baud_rate != baud_rate {
            self.baud_rate = baud_rate;
            self.reset_output();
        }
        true
    }

    /// Pin the GPS clock for deterministic NMEA replay.
    ///
    /// Pass `Some(unix_seconds)` to lock the timestamp, or `None` to
    /// fall back to the system clock.  Useful for reproducible tests.
    pub fn set_time(&mut self, unix_seconds: Option<i64>) {
        self.state.set_fixed_time(unix_seconds);
    }

    pub fn baud_rate(&self) -> u32 {
        self.baud_rate
    }

    pub fn movement(&self) -> &MovementModel {
        &self.movement
    }

    pub fn set_movement(&mut self, movement: MovementModel) {
        self.movement = movement;
        self.replay_progress = 0.0;
        match &mut self.movement {
            MovementModel::GpxReplay {
                points,
                speed_multiplier,
                current_idx,
            } => {
                if points.is_empty() {
                    self.state.speed_knots = 0.0;
                    return;
                }
                *current_idx = (*current_idx).min(points.len() - 1);
                let point = points[*current_idx];
                self.state.latitude = point.0;
                self.state.longitude = point.1;
                update_replay_telemetry(&mut self.state, points, *current_idx, *speed_multiplier);
            }
            MovementModel::GpxReplayWithTimestamps {
                points,
                current_idx,
                ..
            } => {
                if points.is_empty() {
                    self.state.speed_knots = 0.0;
                    return;
                }
                *current_idx = (*current_idx).min(points.len() - 1);
                let point = &points[*current_idx];
                self.state.latitude = point.latitude;
                self.state.longitude = point.longitude;
                self.state.speed_knots = 0.0;
            }
            _ => {}
        }
    }

    /// Enable or disable NMEA output.
    ///
    /// Changing state discards partial output and starts a fresh one-second
    /// epoch, matching a receiver restart at a sentence boundary.
    pub fn set_enabled(&mut self, enabled: bool) {
        if self.state.enabled != enabled {
            self.state.enabled = enabled;
            self.output_enabled = enabled;
            self.reset_output();
        }
    }

    /// Update position and make one NMEA batch available per elapsed second.
    pub fn tick(&mut self, dt_ms: u64) {
        let dt_s = dt_ms as f64 / 1_000.0;
        match &mut self.movement {
            MovementModel::Static => {
                self.state.speed_knots = 0.0;
            }
            MovementModel::Linear {
                speed_ms,
                heading_deg,
            } => {
                let speed = speed_ms.max(0.0);
                if speed.is_finite() && heading_deg.is_finite() {
                    self.state.speed_knots = speed * MPS_TO_KNOTS;
                    self.state.course_deg = heading_deg.rem_euclid(360.0);
                    let distance = speed * dt_s;
                    let course_deg = self.state.course_deg;
                    move_by_distance(&mut self.state, distance, course_deg);
                }
            }
            MovementModel::Waypoint {
                points,
                speed_ms,
                current_idx,
            } => {
                let speed = speed_ms.max(0.0);
                self.state.speed_knots = speed * MPS_TO_KNOTS;
                advance_waypoints(&mut self.state, points, current_idx, speed * dt_s, false);
            }
            MovementModel::GpxReplay {
                points,
                speed_multiplier,
                current_idx,
            } => {
                if points.is_empty() || !speed_multiplier.is_finite() || *speed_multiplier <= 0.0 {
                    self.state.speed_knots = 0.0;
                } else {
                    self.replay_progress += dt_s * *speed_multiplier;
                    let requested_steps = self.replay_progress.floor() as usize;
                    if requested_steps > 0 {
                        self.replay_progress -= requested_steps as f64;
                        let old_idx = (*current_idx).min(points.len() - 1);
                        let new_idx = old_idx
                            .saturating_add(requested_steps)
                            .min(points.len() - 1);
                        *current_idx = new_idx;
                        let new = points[new_idx];
                        self.state.latitude = new.0;
                        self.state.longitude = new.1;
                        update_replay_telemetry(
                            &mut self.state,
                            points,
                            new_idx,
                            *speed_multiplier,
                        );
                        if new_idx == old_idx {
                            self.state.speed_knots = 0.0;
                        }
                    }
                }
            }
            MovementModel::GpxReplayWithTimestamps {
                points,
                speed_multiplier,
                current_idx,
            } => advance_timestamped_replay(
                &mut self.state,
                points,
                current_idx,
                &mut self.replay_progress,
                *speed_multiplier,
                dt_ms,
            ),
        }

        self.sync_enabled_state();
        if !self.output_enabled {
            return;
        }

        self.epoch_elapsed_ms = self.epoch_elapsed_ms.saturating_add(dt_ms);
        let elapsed_epochs = self.epoch_elapsed_ms / EPOCH_MS;
        self.epoch_elapsed_ms %= EPOCH_MS;
        if elapsed_epochs == 0 {
            return;
        }

        for _ in 0..elapsed_epochs {
            self.enqueue_epoch();
        }
        self.uart_bytes_remaining = usize::try_from(elapsed_epochs)
            .unwrap_or(usize::MAX)
            .saturating_mul(uart_bytes_per_second(self.baud_rate));
    }

    /// Read bytes already emitted during the current NMEA epoch.
    ///
    /// Returns zero when output is disabled, no epoch is ready, the current
    /// epoch has been drained, or the UART allowance has been consumed.
    pub fn read(&mut self, buf: &mut [u8]) -> usize {
        self.sync_enabled_state();
        if !self.output_enabled || buf.is_empty() {
            return 0;
        }
        let len = self
            .sentence_buffer
            .len()
            .min(buf.len())
            .min(self.uart_bytes_remaining);
        for destination in &mut buf[..len] {
            *destination = self
                .sentence_buffer
                .pop_front()
                .expect("length was bounded by queued bytes");
        }
        self.uart_bytes_remaining -= len;
        len
    }

    fn enqueue_epoch(&mut self) {
        let now = Utc::now();
        self.sentence_buffer
            .extend(self.state.generate_gga_at(now).bytes());
        self.sentence_buffer
            .extend(self.state.generate_rmc_at(now).bytes());
        self.sentence_buffer
            .extend(self.state.generate_gsa().bytes());
        for sentence in self.state.generate_gsv() {
            self.sentence_buffer.extend(sentence.bytes());
        }
    }

    fn sync_enabled_state(&mut self) {
        if self.output_enabled != self.state.enabled {
            self.output_enabled = self.state.enabled;
            self.reset_output();
        }
    }

    fn reset_output(&mut self) {
        self.sentence_buffer.clear();
        self.epoch_elapsed_ms = 0;
        self.uart_bytes_remaining = 0;
    }
}

fn advance_timestamped_replay(
    state: &mut GpsState,
    points: &[GpxTrackPoint],
    current_idx: &mut usize,
    replay_progress_ms: &mut f64,
    speed_multiplier: f64,
    dt_ms: u64,
) {
    if points.is_empty() || !speed_multiplier.is_finite() || speed_multiplier <= 0.0 {
        state.speed_knots = 0.0;
        return;
    }
    *current_idx = (*current_idx).min(points.len() - 1);
    *replay_progress_ms += dt_ms as f64 * speed_multiplier;
    let mut moved = false;
    while *current_idx + 1 < points.len() {
        let next_idx = *current_idx + 1;
        let interval_ms = timestamp_interval_ms(&points[*current_idx], &points[next_idx]);
        if *replay_progress_ms < interval_ms {
            break;
        }
        *replay_progress_ms -= interval_ms;
        let from = &points[*current_idx];
        let to = &points[next_idx];
        let from_position = (from.latitude, from.longitude);
        let to_position = (to.latitude, to.longitude);
        state.latitude = to.latitude;
        state.longitude = to.longitude;
        state.speed_knots = haversine_m(from_position, to_position) / (interval_ms / 1_000.0)
            * speed_multiplier
            * MPS_TO_KNOTS;
        state.course_deg = initial_bearing_deg(from_position, to_position);
        *current_idx = next_idx;
        moved = true;
    }
    if !moved && *current_idx + 1 >= points.len() {
        state.speed_knots = 0.0;
    }
}

fn timestamp_interval_ms(from: &GpxTrackPoint, to: &GpxTrackPoint) -> f64 {
    match (from.timestamp_ms, to.timestamp_ms) {
        (Some(from_ms), Some(to_ms)) if to_ms > from_ms => (to_ms - from_ms) as f64,
        _ => EPOCH_MS as f64,
    }
}

fn update_replay_telemetry(
    state: &mut GpsState,
    points: &[(f64, f64)],
    current_idx: usize,
    samples_per_second: f64,
) {
    if current_idx == 0 || !samples_per_second.is_finite() || samples_per_second <= 0.0 {
        state.speed_knots = 0.0;
        return;
    }
    let from = points[current_idx - 1];
    let to = points[current_idx];
    state.speed_knots = haversine_m(from, to) * samples_per_second * MPS_TO_KNOTS;
    state.course_deg = initial_bearing_deg(from, to);
}

fn advance_waypoints(
    state: &mut GpsState,
    points: &[(f64, f64)],
    current_idx: &mut usize,
    mut remaining_m: f64,
    loop_at_end: bool,
) {
    if points.is_empty() || !remaining_m.is_finite() {
        return;
    }
    *current_idx = (*current_idx).min(points.len() - 1);
    let mut visited = 0;
    while visited < points.len() {
        let target = points[*current_idx];
        let distance = haversine_m((state.latitude, state.longitude), target);
        state.course_deg = initial_bearing_deg((state.latitude, state.longitude), target);
        if distance > remaining_m {
            move_by_distance(state, remaining_m, state.course_deg);
            break;
        }
        state.latitude = target.0;
        state.longitude = target.1;
        remaining_m -= distance;
        visited += 1;
        if *current_idx + 1 < points.len() {
            *current_idx += 1;
        } else if loop_at_end {
            *current_idx = 0;
        } else {
            state.speed_knots = 0.0;
            break;
        }
    }
}

fn move_by_distance(state: &mut GpsState, distance_m: f64, bearing_deg: f64) {
    if distance_m <= 0.0 || !distance_m.is_finite() {
        return;
    }
    let angular_distance = distance_m / EARTH_RADIUS_M;
    let bearing = bearing_deg.to_radians();
    let lat1 = state.latitude.to_radians();
    let lon1 = state.longitude.to_radians();
    let lat2 = (lat1.sin() * angular_distance.cos()
        + lat1.cos() * angular_distance.sin() * bearing.cos())
    .asin();
    let lon2 = lon1
        + (bearing.sin() * angular_distance.sin() * lat1.cos())
            .atan2(angular_distance.cos() - lat1.sin() * lat2.sin());
    state.latitude = lat2.to_degrees();
    state.longitude = (lon2.to_degrees() + 540.0).rem_euclid(360.0) - 180.0;
}

fn haversine_m(from: (f64, f64), to: (f64, f64)) -> f64 {
    let lat1 = from.0.to_radians();
    let lat2 = to.0.to_radians();
    let delta_lat = (to.0 - from.0).to_radians();
    let delta_lon = (to.1 - from.1).to_radians();
    let a =
        (delta_lat / 2.0).sin().powi(2) + lat1.cos() * lat2.cos() * (delta_lon / 2.0).sin().powi(2);
    2.0 * EARTH_RADIUS_M * a.sqrt().atan2((1.0 - a).sqrt())
}

fn initial_bearing_deg(from: (f64, f64), to: (f64, f64)) -> f64 {
    if from == to {
        return 0.0;
    }
    let lat1 = from.0.to_radians();
    let lat2 = to.0.to_radians();
    let delta_lon = (to.1 - from.1).to_radians();
    let y = delta_lon.sin() * lat2.cos();
    let x = lat1.cos() * lat2.sin() - lat1.sin() * lat2.cos() * delta_lon.cos();
    (y.atan2(x) * 180.0 / PI).rem_euclid(360.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn drain_epoch(manager: &mut GpsManager, chunk_size: usize) -> Vec<u8> {
        let mut output = Vec::new();
        let mut buffer = vec![0_u8; chunk_size];
        loop {
            let len = manager.read(&mut buffer);
            if len == 0 {
                return output;
            }
            output.extend_from_slice(&buffer[..len]);
        }
    }

    #[test]
    fn linear_model_moves_expected_distance_north() {
        let mut manager = GpsManager::new(0.0, 0.0);
        manager.set_movement(MovementModel::Linear {
            speed_ms: 10.0,
            heading_deg: 0.0,
        });

        manager.tick(1_000);

        assert!((manager.state().latitude - 0.000_089_93).abs() < 0.000_000_1);
        assert!(manager.state().longitude.abs() < 1e-10);
        assert!((manager.state().speed_knots - 19.438_444_9).abs() < 1e-6);
    }

    #[test]
    fn waypoint_model_reaches_and_advances_between_points() {
        let mut manager = GpsManager::new(0.0, 0.0);
        manager.set_movement(MovementModel::Waypoint {
            points: vec![(0.0, 0.000_1), (0.0, 0.000_2)],
            speed_ms: 20.0,
            current_idx: 0,
        });

        manager.tick(1_000);

        assert!((manager.state().longitude - 0.000_179_86).abs() < 0.000_001);
        assert!(matches!(
            manager.movement(),
            MovementModel::Waypoint { current_idx: 1, .. }
        ));
    }

    #[test]
    fn gpx_replay_uses_multiplier_as_samples_per_second() {
        let mut manager = GpsManager::new(0.0, 0.0);
        manager.set_movement(MovementModel::GpxReplay {
            points: vec![(1.0, 1.0), (2.0, 2.0), (3.0, 3.0)],
            speed_multiplier: 2.0,
            current_idx: 0,
        });

        manager.tick(1_000);

        assert_eq!(
            (manager.state().latitude, manager.state().longitude),
            (3.0, 3.0)
        );
        let expected_speed = haversine_m((2.0, 2.0), (3.0, 3.0)) * 2.0 * MPS_TO_KNOTS;
        assert!((manager.state().speed_knots - expected_speed).abs() < 1e-6);
        assert!((manager.state().course_deg - 44.96).abs() < 0.1);
    }

    #[test]
    fn read_emits_one_complete_l76k_cycle_per_second() {
        let mut manager = GpsManager::new(51.5, -0.1);
        let mut byte = [0_u8; 1];

        assert_eq!(manager.read(&mut byte), 0);
        manager.tick(999);
        assert_eq!(manager.read(&mut byte), 0);
        manager.tick(1);

        let output = drain_epoch(&mut manager, 17);
        let sentences: Vec<_> = std::str::from_utf8(&output).unwrap().lines().collect();
        assert_eq!(sentences.len(), 5);
        assert!(sentences[0].starts_with("$GPGGA,"));
        assert!(sentences[1].starts_with("$GPRMC,"));
        assert!(sentences[2].starts_with("$GPGSA,"));
        assert!(sentences[3].starts_with("$GPGSV,2,1,08,"));
        assert!(sentences[4].starts_with("$GPGSV,2,2,08,"));
        assert_eq!(manager.read(&mut byte), 0);

        manager.tick(1_000);
        assert!(!drain_epoch(&mut manager, 256).is_empty());
        assert_eq!(manager.read(&mut byte), 0);
    }

    #[test]
    fn read_enforces_the_9600_baud_epoch_allowance() {
        let mut manager = GpsManager::new(51.5, -0.1);
        manager.state_mut().satellites = u8::MAX;
        manager.tick(1_000);
        let mut buffer = vec![0_u8; UART_BYTES_PER_SECOND * 2];

        assert_eq!(manager.read(&mut buffer), UART_BYTES_PER_SECOND);
        assert_eq!(manager.read(&mut buffer), 0);
    }

    #[test]
    fn set_baud_rate_accepts_l76k_rates_and_rejects_absurd_ones() {
        let mut manager = GpsManager::new(51.5, -0.1);
        assert_eq!(manager.baud_rate(), GPS_BAUD_RATE);

        assert!(manager.set_baud_rate(GPS_BAUD_RATE_FAST));
        assert_eq!(manager.baud_rate(), GPS_BAUD_RATE_FAST);
        assert!(manager.set_baud_rate(GPS_BAUD_RATE));
        assert!(!manager.set_baud_rate(0));
        assert!(!manager.set_baud_rate(1_000_000));
        assert_eq!(manager.baud_rate(), GPS_BAUD_RATE);
    }

    #[test]
    fn read_enforces_the_configured_baud_allowance() {
        let mut manager = GpsManager::new(51.5, -0.1);
        manager.state_mut().satellites = u8::MAX;
        manager.set_baud_rate(GPS_BAUD_RATE_FAST);
        manager.tick(1_000);
        let fast_allowance = uart_bytes_per_second(GPS_BAUD_RATE_FAST);
        let mut buffer = vec![0_u8; fast_allowance * 2];

        assert_eq!(manager.read(&mut buffer), fast_allowance);
        assert_eq!(manager.read(&mut buffer), 0);

        // Reconfiguring to a slower rate restarts output at a sentence
        // boundary and applies the new allowance.
        manager.set_baud_rate(GPS_BAUD_RATE);
        assert_eq!(manager.read(&mut buffer), 0);
        manager.tick(1_000);
        assert_eq!(manager.read(&mut buffer), UART_BYTES_PER_SECOND);
    }

    #[test]
    fn disabling_discards_a_partial_sentence_and_restarts_on_a_new_epoch() {
        let mut manager = GpsManager::new(51.5, -0.1);
        manager.tick(1_000);
        let mut partial = [0_u8; 7];
        assert_eq!(manager.read(&mut partial), partial.len());

        manager.set_enabled(false);
        manager.set_enabled(true);
        assert_eq!(manager.read(&mut partial), 0);
        manager.tick(999);
        assert_eq!(manager.read(&mut partial), 0);
        manager.tick(1);

        assert_eq!(manager.read(&mut partial), partial.len());
        assert_eq!(&partial, b"$GPGGA,");
    }

    #[test]
    fn gpx_replay_speed_and_course_are_reported_in_rmc() {
        let mut manager = GpsManager::new(0.0, 0.0);
        manager.set_movement(MovementModel::GpxReplay {
            points: vec![(0.0, 0.0), (0.0, 0.001)],
            speed_multiplier: 1.0,
            current_idx: 0,
        });

        manager.tick(1_000);

        assert!((manager.state().speed_knots - 216.15).abs() < 0.1);
        assert!((manager.state().course_deg - 90.0).abs() < 1e-10);
        let output = String::from_utf8(drain_epoch(&mut manager, 256)).unwrap();
        let rmc = output
            .lines()
            .find(|line| line.starts_with("$GPRMC"))
            .unwrap();
        assert!(rmc.contains(",216.1,90.0,"));
    }

    #[test]
    fn gpx_replay_uses_track_timestamps_for_telemetry() {
        let mut manager = GpsManager::new(0.0, 0.0);
        manager.set_movement(MovementModel::GpxReplayWithTimestamps {
            points: vec![
                GpxTrackPoint {
                    latitude: 0.0,
                    longitude: 0.0,
                    timestamp_ms: Some(10_000),
                },
                GpxTrackPoint {
                    latitude: 0.0,
                    longitude: 0.001,
                    timestamp_ms: Some(12_000),
                },
            ],
            speed_multiplier: 1.0,
            current_idx: 0,
        });

        manager.tick(1_000);
        assert_eq!(
            (manager.state().latitude, manager.state().longitude),
            (0.0, 0.0)
        );
        manager.tick(1_000);

        assert_eq!(
            (manager.state().latitude, manager.state().longitude),
            (0.0, 0.001)
        );
        assert!((manager.state().speed_knots - 108.07).abs() < 0.1);
        assert!((manager.state().course_deg - 90.0).abs() < 1e-10);
    }

    #[test]
    fn timestamped_gpx_replay_falls_back_to_one_second_samples() {
        let mut manager = GpsManager::new(0.0, 0.0);
        manager.set_movement(MovementModel::GpxReplayWithTimestamps {
            points: vec![
                GpxTrackPoint {
                    latitude: 0.0,
                    longitude: 0.0,
                    timestamp_ms: None,
                },
                GpxTrackPoint {
                    latitude: 0.0,
                    longitude: 0.001,
                    timestamp_ms: None,
                },
            ],
            speed_multiplier: 1.0,
            current_idx: 0,
        });

        manager.tick(1_000);

        assert_eq!(
            (manager.state().latitude, manager.state().longitude),
            (0.0, 0.001)
        );
        assert!((manager.state().speed_knots - 216.15).abs() < 0.1);
    }
}
