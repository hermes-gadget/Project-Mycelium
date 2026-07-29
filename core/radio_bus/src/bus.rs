// The main RadioBus — routes packets between virtual nodes with propagation simulation

use std::collections::HashMap;

use crate::channel::ChannelState;
use crate::propagation;
use crate::types::{RadioChannel, RxPacket, TxEvent};

const DEFAULT_NOISE_FLOOR_DBM: f64 = -120.0;
const CAPTURE_THRESHOLD_DB: f64 = 6.0;

/// A single-threaded virtual LoRa network.
pub struct RadioBus {
    nodes: HashMap<String, NodeState>,
    channels: HashMap<String, ChannelState>,
    antenna_gain_tx_dbi: f64,
    antenna_gain_rx_dbi: f64,
    noise_floor_dbm: f64,
}

struct NodeState {
    position: (f64, f64),
    tx_power_dbm: f64,
    current_channel: RadioChannel,
    inbox: Vec<RxPacket>,
}

impl RadioBus {
    pub fn new() -> Self {
        Self {
            nodes: HashMap::new(),
            channels: HashMap::new(),
            antenna_gain_tx_dbi: 0.0,
            antenna_gain_rx_dbi: 0.0,
            noise_floor_dbm: DEFAULT_NOISE_FLOOR_DBM,
        }
    }

    /// Creates a reproducible bus.
    ///
    /// The current free-space model has no stochastic loss, so every bus is
    /// deterministic. The seed is accepted to keep construction stable when
    /// seeded propagation effects are introduced.
    pub fn with_seed(_seed: u64) -> Self {
        Self::new()
    }

    pub fn register_node(
        &mut self,
        id: String,
        position: (f64, f64),
        tx_power: f64,
        channel: RadioChannel,
    ) {
        let channel_key = channel_key(&channel);
        self.channels
            .entry(channel_key)
            .or_insert_with(|| ChannelState::new(channel.clone()));
        self.nodes.insert(
            id,
            NodeState {
                position,
                tx_power_dbm: tx_power,
                current_channel: channel,
                inbox: Vec::new(),
            },
        );
    }

    pub fn unregister_node(&mut self, id: &str) {
        self.nodes.remove(id);
    }

    pub fn update_position(&mut self, id: &str, position: (f64, f64)) {
        if let Some(node) = self.nodes.get_mut(id) {
            node.position = position;
        }
    }

    /// Broadcasts a packet to every tuned node whose link budget can decode it.
    pub fn broadcast(&mut self, mut tx: TxEvent) {
        let Some(sender) = self.nodes.get(&tx.node_id) else {
            return;
        };

        // Registered node state is authoritative, which also makes position
        // updates apply even when a caller constructed the event earlier.
        tx.position = sender.position;
        tx.tx_power_dbm = sender.tx_power_dbm;
        tx.channel = sender.current_channel.clone();

        let key = channel_key(&tx.channel);
        let mut contenders: Vec<TxEvent> = self
            .channels
            .get(&key)
            .into_iter()
            .flat_map(|state| state.overlapping_transmissions(tx.timestamp_ms, tx.airtime_ms))
            .cloned()
            .collect();
        contenders.push(tx.clone());

        let sensitivity_dbm = propagation::sensitivity(&tx.channel);
        for (receiver_id, receiver) in &mut self.nodes {
            if receiver.current_channel != tx.channel {
                continue;
            }

            remove_contender_packets(&mut receiver.inbox, &contenders);

            // A node cannot receive while it is one of the overlapping transmitters.
            if contenders
                .iter()
                .any(|contender| contender.node_id == *receiver_id)
            {
                continue;
            }

            let mut audible: Vec<(&TxEvent, f64)> = contenders
                .iter()
                .filter_map(|contender| {
                    let distance = propagation::distance_km(contender.position, receiver.position);
                    let rssi = propagation::received_rssi(
                        contender.tx_power_dbm,
                        distance,
                        contender.channel.freq_mhz,
                        self.antenna_gain_tx_dbi,
                        self.antenna_gain_rx_dbi,
                    );
                    (rssi.is_finite() && rssi > sensitivity_dbm).then_some((contender, rssi))
                })
                .collect();

            audible.sort_by(|left, right| right.1.total_cmp(&left.1));
            let winner = match audible.as_slice() {
                [] => None,
                [only] => Some(*only),
                [strongest, second, ..] if strongest.1 - second.1 > CAPTURE_THRESHOLD_DB => {
                    Some(*strongest)
                }
                _ => None,
            };

            if let Some((event, rssi_dbm)) = winner {
                receiver.inbox.push(RxPacket {
                    from_node: event.node_id.clone(),
                    data: event.data.clone(),
                    rssi_dbm,
                    snr_db: rssi_dbm - self.noise_floor_dbm,
                    channel: event.channel.clone(),
                    timestamp_ms: event.timestamp_ms,
                });
            }
        }

        self.channels
            .entry(key)
            .or_insert_with(|| ChannelState::new(tx.channel.clone()))
            .add_transmission(tx);
    }

    /// Drains all packets currently queued for a node.
    pub fn poll(&mut self, node_id: &str) -> Vec<RxPacket> {
        self.nodes
            .get_mut(node_id)
            .map(|node| std::mem::take(&mut node.inbox))
            .unwrap_or_default()
    }

    /// Removes transmissions whose airtime has elapsed.
    pub fn tick(&mut self, now_ms: u64) {
        for channel in self.channels.values_mut() {
            channel.remove_expired(now_ms);
        }
    }
}

impl Default for RadioBus {
    fn default() -> Self {
        Self::new()
    }
}

fn channel_key(channel: &RadioChannel) -> String {
    format!(
        "{:016x}:{}:{}:{}",
        channel.freq_mhz.to_bits(),
        channel.bandwidth_khz,
        channel.spreading_factor,
        channel.coding_rate
    )
}

fn remove_contender_packets(inbox: &mut Vec<RxPacket>, contenders: &[TxEvent]) {
    inbox.retain(|packet| {
        !contenders.iter().any(|contender| {
            packet.from_node == contender.node_id
                && packet.timestamp_ms == contender.timestamp_ms
                && packet.channel == contender.channel
        })
    });
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

    fn transmission(node_id: &str, data: u8, timestamp_ms: u64) -> TxEvent {
        TxEvent {
            node_id: node_id.into(),
            channel: channel(),
            data: vec![data],
            tx_power_dbm: 0.0,
            airtime_ms: 100,
            position: (90.0, 90.0),
            timestamp_ms,
        }
    }

    fn register(bus: &mut RadioBus, id: &str, position: (f64, f64), tx_power_dbm: f64) {
        bus.register_node(id.into(), position, tx_power_dbm, channel());
    }

    #[test]
    fn two_nodes_in_range_exchange_packets_and_poll_drains_inbox() {
        let mut bus = RadioBus::new();
        register(&mut bus, "alice", (51.5074, -0.1278), 14.0);
        register(&mut bus, "bob", (51.5075, -0.1278), 14.0);

        bus.broadcast(transmission("alice", 42, 1_000));
        let packets = bus.poll("bob");

        assert_eq!(packets.len(), 1);
        assert_eq!(packets[0].from_node, "alice");
        assert_eq!(packets[0].data, vec![42]);
        assert!(packets[0].rssi_dbm > propagation::sensitivity(&channel()));
        assert!((packets[0].snr_db - (packets[0].rssi_dbm + 120.0)).abs() < f64::EPSILON);
        assert!(bus.poll("bob").is_empty());
        assert!(bus.poll("alice").is_empty());
    }

    #[test]
    fn nodes_beyond_the_link_budget_do_not_exchange_packets() {
        let mut bus = RadioBus::new();
        register(&mut bus, "alice", (0.0, 0.0), 14.0);
        register(&mut bus, "bob", (0.0, 180.0), 14.0);

        bus.broadcast(transmission("alice", 42, 1_000));

        assert!(bus.poll("bob").is_empty());
    }

    #[test]
    fn simultaneous_equal_strength_transmissions_collide_for_every_receiver() {
        let mut bus = RadioBus::new();
        register(&mut bus, "alice", (0.0, -0.001), 14.0);
        register(&mut bus, "bob", (0.0, 0.001), 14.0);
        register(&mut bus, "carol", (0.0, 0.0), 14.0);

        bus.broadcast(transmission("alice", 1, 1_000));
        bus.broadcast(transmission("bob", 2, 1_000));

        assert!(bus.poll("alice").is_empty());
        assert!(bus.poll("bob").is_empty());
        assert!(bus.poll("carol").is_empty());
    }

    #[test]
    fn capture_effect_delivers_a_signal_more_than_six_db_stronger() {
        let mut bus = RadioBus::with_seed(7);
        register(&mut bus, "weak", (0.0, 0.01), 14.0);
        register(&mut bus, "strong", (0.0, 0.0001), 14.0);
        register(&mut bus, "receiver", (0.0, 0.0), 14.0);

        bus.broadcast(transmission("weak", 1, 1_000));
        bus.broadcast(transmission("strong", 2, 1_000));
        let packets = bus.poll("receiver");

        assert_eq!(packets.len(), 1);
        assert_eq!(packets[0].from_node, "strong");
        assert_eq!(packets[0].data, vec![2]);
    }

    #[test]
    fn updated_position_is_used_and_unknown_nodes_are_ignored() {
        let mut bus = RadioBus::new();
        register(&mut bus, "alice", (0.0, 180.0), 14.0);
        register(&mut bus, "bob", (0.0, 0.0), 14.0);

        bus.update_position("alice", (0.0, 0.0001));
        bus.broadcast(transmission("alice", 1, 0));
        bus.broadcast(transmission("unknown", 2, 200));

        assert_eq!(bus.poll("bob").len(), 1);
        assert!(bus.poll("unknown").is_empty());
    }

    #[test]
    fn unregister_stops_delivery_and_tick_expires_transmissions() {
        let mut bus = RadioBus::new();
        register(&mut bus, "alice", (0.0, 0.0), 14.0);
        register(&mut bus, "bob", (0.0, 0.0001), 14.0);
        register(&mut bus, "carol", (0.0, 0.0002), 14.0);

        bus.broadcast(transmission("alice", 1, 100));
        bus.tick(200);
        bus.broadcast(transmission("bob", 2, 200));
        bus.unregister_node("carol");

        assert_eq!(bus.poll("alice").len(), 1);
        assert!(bus.poll("carol").is_empty());
    }
}
