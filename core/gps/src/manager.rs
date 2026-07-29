use std::f64::consts::PI;

use crate::nmea::GpsState;

const EARTH_RADIUS_M: f64 = 6_371_000.0;
const MPS_TO_KNOTS: f64 = 1.943_844_492_440_6;

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
}

pub struct GpsManager {
    state: GpsState,
    movement: MovementModel,
    sentence_buffer: Vec<u8>,
    next_sentence: usize,
    replay_progress: f64,
}

impl GpsManager {
    pub fn new(lat: f64, lon: f64) -> Self {
        Self {
            state: GpsState::new(lat, lon),
            movement: MovementModel::Static,
            sentence_buffer: Vec::new(),
            next_sentence: 0,
            replay_progress: 0.0,
        }
    }

    pub fn state(&self) -> &GpsState {
        &self.state
    }

    pub fn state_mut(&mut self) -> &mut GpsState {
        &mut self.state
    }

    pub fn movement(&self) -> &MovementModel {
        &self.movement
    }

    pub fn set_movement(&mut self, movement: MovementModel) {
        self.movement = movement;
        self.replay_progress = 0.0;
    }

    /// Update position based on the movement model and elapsed time.
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
                self.state.speed_knots = 0.0;
                if points.is_empty() || !speed_multiplier.is_finite() {
                    return;
                }
                self.replay_progress += dt_s * speed_multiplier.max(0.0);
                let steps = self.replay_progress.floor() as usize;
                if steps == 0 {
                    return;
                }
                self.replay_progress -= steps as f64;
                let old = (self.state.latitude, self.state.longitude);
                *current_idx = current_idx.saturating_add(steps).min(points.len() - 1);
                let new = points[*current_idx];
                self.state.latitude = new.0;
                self.state.longitude = new.1;
                self.state.course_deg = initial_bearing_deg(old, new);
            }
        }
    }

    /// Read bytes from the next rotating NMEA sentence.
    ///
    /// Returns zero when GPS output is disabled or `buf` is empty. A sentence
    /// may be returned over multiple calls when the destination is small.
    pub fn read(&mut self, buf: &mut [u8]) -> usize {
        if !self.state.enabled || buf.is_empty() {
            return 0;
        }
        if self.sentence_buffer.is_empty() {
            let sentence = match self.next_sentence {
                0 => self.state.generate_gga(),
                1 => self.state.generate_rmc(),
                2 => self.state.generate_gsa(),
                _ => self.state.generate_gsv(),
            };
            self.next_sentence = (self.next_sentence + 1) % 4;
            self.sentence_buffer = sentence.into_bytes();
        }
        let len = self.sentence_buffer.len().min(buf.len());
        buf[..len].copy_from_slice(&self.sentence_buffer[..len]);
        self.sentence_buffer.drain(..len);
        len
    }
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
    }

    #[test]
    fn read_rotates_sentences_and_respects_enabled_state() {
        let mut manager = GpsManager::new(51.5, -0.1);
        let mut buffer = [0_u8; 256];
        let len = manager.read(&mut buffer);
        assert!(std::str::from_utf8(&buffer[..len])
            .unwrap()
            .starts_with("$GPGGA,"));

        manager.state_mut().enabled = false;
        assert_eq!(manager.read(&mut buffer), 0);
    }
}
