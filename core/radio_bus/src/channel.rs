// Channel abstraction — models a single radio channel
// Tracks active transmissions and detects collisions

use crate::types::{RadioChannel, TxEvent};

/// Runtime state for a single set of LoRa modulation parameters.
pub struct ChannelState {
    pub channel: RadioChannel,
    active_transmissions: Vec<TxEvent>,
}

impl ChannelState {
    pub fn new(channel: RadioChannel) -> Self {
        Self {
            channel,
            active_transmissions: Vec::new(),
        }
    }

    pub fn is_busy(&self) -> bool {
        !self.active_transmissions.is_empty()
    }

    pub fn add_transmission(&mut self, tx: TxEvent) {
        self.active_transmissions.push(tx);
    }

    /// Reports whether the requested receive interval overlaps an active transmission.
    pub fn check_collision(&self, rx_time_ms: u64, rx_duration_ms: u32) -> bool {
        self.overlapping_transmissions(rx_time_ms, rx_duration_ms)
            .next()
            .is_some()
    }

    pub(crate) fn overlapping_transmissions(
        &self,
        start_ms: u64,
        duration_ms: u32,
    ) -> impl Iterator<Item = &TxEvent> {
        self.active_transmissions.iter().filter(move |tx| {
            intervals_overlap(start_ms, duration_ms, tx.timestamp_ms, tx.airtime_ms)
        })
    }

    /// Removes transmissions whose airtime has elapsed by `now_ms`.
    pub fn remove_expired(&mut self, now_ms: u64) {
        self.active_transmissions
            .retain(|tx| tx.timestamp_ms.saturating_add(tx.airtime_ms as u64) > now_ms);
    }
}

fn intervals_overlap(
    first_start_ms: u64,
    first_duration_ms: u32,
    second_start_ms: u64,
    second_duration_ms: u32,
) -> bool {
    if first_duration_ms == 0 || second_duration_ms == 0 {
        return false;
    }

    let first_end_ms = first_start_ms.saturating_add(first_duration_ms as u64);
    let second_end_ms = second_start_ms.saturating_add(second_duration_ms as u64);

    first_start_ms < second_end_ms && second_start_ms < first_end_ms
}

#[cfg(test)]
mod tests {
    use super::*;

    fn channel() -> RadioChannel {
        RadioChannel {
            freq_mhz: 915.0,
            bandwidth_khz: 125,
            spreading_factor: 7,
            coding_rate: 5,
        }
    }

    fn transmission(timestamp_ms: u64, airtime_ms: u32) -> TxEvent {
        TxEvent {
            node_id: "sender".into(),
            channel: channel(),
            data: vec![1, 2, 3],
            tx_power_dbm: 14.0,
            airtime_ms,
            position: (0.0, 0.0),
            timestamp_ms,
        }
    }

    #[test]
    fn detects_only_overlapping_transmission_intervals() {
        let mut state = ChannelState::new(channel());
        assert!(!state.is_busy());

        state.add_transmission(transmission(100, 100));

        assert!(state.is_busy());
        assert!(state.check_collision(50, 51));
        assert!(state.check_collision(150, 10));
        assert!(!state.check_collision(0, 100));
        assert!(!state.check_collision(200, 10));
    }

    #[test]
    fn zero_duration_does_not_collide() {
        let mut state = ChannelState::new(channel());
        state.add_transmission(transmission(100, 100));

        assert!(!state.check_collision(150, 0));
    }

    #[test]
    fn cleanup_removes_transmissions_at_their_end_time() {
        let mut state = ChannelState::new(channel());
        state.add_transmission(transmission(100, 100));

        state.remove_expired(199);
        assert!(state.is_busy());

        state.remove_expired(200);
        assert!(!state.is_busy());
    }
}
