//! GPS emulation.

use std::collections::VecDeque;

#[derive(Clone, Debug)]
pub struct GpsManager {
    latitude: f64,
    longitude: f64,
    elapsed_ms: u64,
    sentences: VecDeque<String>,
    enabled: bool,
}

impl GpsManager {
    pub fn new(latitude: f64, longitude: f64) -> Self {
        Self {
            latitude,
            longitude,
            elapsed_ms: 0,
            sentences: VecDeque::new(),
            enabled: true,
        }
    }

    /// Advances GPS time and emits a lightweight position sentence once a second.
    pub fn tick(&mut self, delta_ms: u64) {
        if !self.enabled {
            return;
        }
        self.elapsed_ms = self.elapsed_ms.saturating_add(delta_ms);
        while self.elapsed_ms >= 1_000 {
            self.elapsed_ms -= 1_000;
            self.sentences.push_back(format!(
                "$PMYCELIUM,{:.6},{:.6}*00\r\n",
                self.latitude, self.longitude
            ));
        }
    }

    pub fn position(&self) -> (f64, f64) {
        (self.latitude, self.longitude)
    }

    pub fn read_sentence(&mut self) -> Option<String> {
        self.sentences.pop_front()
    }

    pub fn set_enabled(&mut self, enabled: bool) {
        self.enabled = enabled;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tick_emits_position_once_per_second() {
        let mut gps = GpsManager::new(51.5074, -0.1278);
        gps.tick(999);
        assert!(gps.read_sentence().is_none());
        gps.tick(1);
        assert!(gps.read_sentence().unwrap().contains("51.507400,-0.127800"));
    }
}
