// The main RadioBus — routes packets between virtual nodes with propagation simulation.

use std::collections::HashMap;

use crate::propagation::{self, PropagationModel};
use crate::types::{
    RadioChannel, RxPacket, TxEvent, SX1262_DIO2_RX_LOSS_DB, SX1262_DIO2_TX_LOSS_DB,
};

const THERMAL_NOISE_DENSITY_DBM_PER_HZ: f64 = -174.0;
const DEFAULT_NOISE_FIGURE_DB: f64 = 6.0;
const CAPTURE_THRESHOLD_DB: f64 = 6.0;

/// Physical parameters shared by the virtual radio network.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RadioBusConfig {
    pub propagation_model: PropagationModel,
    pub antenna_gain_tx_dbi: f64,
    pub antenna_gain_rx_dbi: f64,
    /// Feedline, enclosure, polarization, and other unmodelled losses.
    pub system_loss_db: f64,
    /// Receiver noise figure added to integrated thermal noise.
    pub noise_figure_db: f64,
    /// Signals this close to a receiver's center frequency contribute
    /// interference, even when their modulation settings are not decodable.
    pub frequency_tolerance_khz: f64,
    /// Initial DIO2 RF-switch configuration for newly registered radios.
    ///
    /// Upstream SX1262 libraries default this off. T-Deck firmware must enable
    /// it or the external antenna switch introduces substantial TX/RX loss.
    pub dio2_as_rf_switch: bool,
}

impl Default for RadioBusConfig {
    fn default() -> Self {
        Self {
            propagation_model: PropagationModel::TwoRayGround {
                tx_height_m: 1.5,
                rx_height_m: 1.5,
            },
            antenna_gain_tx_dbi: 0.0,
            antenna_gain_rx_dbi: 0.0,
            system_loss_db: 6.0,
            noise_figure_db: DEFAULT_NOISE_FIGURE_DB,
            frequency_tolerance_khz: 125.0,
            dio2_as_rf_switch: false,
        }
    }
}

/// A single-threaded virtual LoRa network.
pub struct RadioBus {
    nodes: HashMap<String, NodeState>,
    transmissions: Vec<ScheduledTransmission>,
    config: RadioBusConfig,
}

struct NodeState {
    position: (f64, f64),
    tx_power_dbm: f64,
    current_channel: RadioChannel,
    inbox: Vec<RxPacket>,
    tx_busy_until_ms: u64,
    rx_enabled: bool,
    dio2_as_rf_switch: bool,
}

#[derive(Clone)]
struct ScheduledTransmission {
    event: TxEvent,
    finalized: bool,
}

impl RadioBus {
    pub fn new() -> Self {
        Self::with_config(RadioBusConfig::default())
    }

    pub fn with_config(config: RadioBusConfig) -> Self {
        Self {
            nodes: HashMap::new(),
            transmissions: Vec::new(),
            config,
        }
    }

    /// Creates a reproducible bus.
    ///
    /// All currently supplied propagation models are deterministic. The seed
    /// remains part of the API for future stochastic fading models.
    pub fn with_seed(_seed: u64) -> Self {
        Self::new()
    }

    pub fn config(&self) -> RadioBusConfig {
        self.config
    }

    pub fn set_propagation_model(&mut self, model: PropagationModel) {
        self.config.propagation_model = model;
    }

    pub fn set_system_loss_db(&mut self, system_loss_db: f64) {
        if system_loss_db.is_finite() && system_loss_db >= 0.0 {
            self.config.system_loss_db = system_loss_db;
        }
    }

    pub fn set_frequency_tolerance_khz(&mut self, tolerance_khz: f64) {
        if tolerance_khz.is_finite() && tolerance_khz >= 0.0 {
            self.config.frequency_tolerance_khz = tolerance_khz;
        }
    }

    pub fn register_node(
        &mut self,
        id: String,
        position: (f64, f64),
        tx_power: f64,
        channel: RadioChannel,
    ) {
        self.nodes.insert(
            id,
            NodeState {
                position,
                tx_power_dbm: tx_power,
                current_channel: channel,
                inbox: Vec::new(),
                tx_busy_until_ms: 0,
                rx_enabled: true,
                dio2_as_rf_switch: self.config.dio2_as_rf_switch,
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

    /// Enables or disables packet reception for one node. Entering the
    /// disabled state clears the hardware receive FIFO, matching the radio
    /// reset that accompanies ESP32 deep sleep.
    pub fn set_receive_enabled(&mut self, id: &str, enabled: bool) {
        if let Some(node) = self.nodes.get_mut(id) {
            node.rx_enabled = enabled;
            if !enabled {
                node.inbox.clear();
            }
        }
    }

    pub fn set_dio2_as_rf_switch(&mut self, id: &str, as_rf_switch: bool) {
        if let Some(node) = self.nodes.get_mut(id) {
            node.dio2_as_rf_switch = as_rf_switch;
        }
    }

    pub fn dio2_as_rf_switch(&self, id: &str) -> Option<bool> {
        self.nodes.get(id).map(|node| node.dio2_as_rf_switch)
    }

    /// Schedules a packet for delivery after its airtime has elapsed.
    ///
    /// Returns false for an unknown sender or when that radio is already
    /// transmitting at the requested start time.
    pub fn broadcast(&mut self, mut tx: TxEvent) -> bool {
        let Some(sender) = self.nodes.get_mut(&tx.node_id) else {
            return false;
        };
        if tx.timestamp_ms < sender.tx_busy_until_ms {
            return false;
        }

        // Registered node state is authoritative, which also makes position
        // updates apply even when a caller constructed the event earlier.
        tx.position = sender.position;
        tx.tx_power_dbm = sender.tx_power_dbm
            - if sender.dio2_as_rf_switch {
                0.0
            } else {
                SX1262_DIO2_TX_LOSS_DB
            };
        tx.channel = sender.current_channel.clone();
        sender.tx_busy_until_ms = tx.timestamp_ms.saturating_add(u64::from(tx.airtime_ms));
        self.transmissions.push(ScheduledTransmission {
            event: tx,
            finalized: false,
        });
        true
    }

    pub fn is_send_complete(&self, node_id: &str, now_ms: u64) -> bool {
        self.nodes
            .get(node_id)
            .is_some_and(|node| now_ms >= node.tx_busy_until_ms)
    }

    /// Drains all packets whose transmissions have completed for a node.
    pub fn poll(&mut self, node_id: &str) -> Vec<RxPacket> {
        self.nodes
            .get_mut(node_id)
            .map(|node| std::mem::take(&mut node.inbox))
            .unwrap_or_default()
    }

    /// Advances radio time, resolving completed transmissions before removing
    /// collision history that is no longer needed.
    pub fn tick(&mut self, now_ms: u64) {
        let all_events: Vec<TxEvent> = self
            .transmissions
            .iter()
            .map(|scheduled| scheduled.event.clone())
            .collect();
        let completed_indices: Vec<usize> = self
            .transmissions
            .iter()
            .enumerate()
            .filter_map(|(index, scheduled)| {
                (!scheduled.finalized && transmission_end(&scheduled.event) <= now_ms)
                    .then_some(index)
            })
            .collect();

        for index in completed_indices {
            let event = self.transmissions[index].event.clone();
            self.deliver_completed(&event, &all_events);
            self.transmissions[index].finalized = true;
        }

        // Keep finalized transmissions while an unfinished overlapping packet
        // still needs them for collision evaluation.
        let unfinished: Vec<TxEvent> = self
            .transmissions
            .iter()
            .filter(|scheduled| !scheduled.finalized)
            .map(|scheduled| scheduled.event.clone())
            .collect();
        self.transmissions.retain(|scheduled| {
            !scheduled.finalized
                || unfinished
                    .iter()
                    .any(|event| transmissions_overlap(&scheduled.event, event))
        });
    }

    fn deliver_completed(&mut self, event: &TxEvent, all_events: &[TxEvent]) {
        let config = self.config;
        for (receiver_id, receiver) in &mut self.nodes {
            if *receiver_id == event.node_id
                || !receiver.rx_enabled
                || receiver.current_channel != event.channel
            {
                continue;
            }

            // SX1262 is half-duplex.
            if all_events
                .iter()
                .any(|other| other.node_id == *receiver_id && transmissions_overlap(event, other))
            {
                continue;
            }

            let distance = propagation::distance_km(event.position, receiver.position);
            let receiver_loss_db = if receiver.dio2_as_rf_switch {
                0.0
            } else {
                SX1262_DIO2_RX_LOSS_DB
            };
            let Some(rssi_dbm) = propagation::received_rssi_with_model(
                event.tx_power_dbm,
                distance,
                event.channel.freq_mhz,
                config.antenna_gain_tx_dbi,
                config.antenna_gain_rx_dbi,
                config.system_loss_db,
                config.propagation_model,
            )
            .map(|rssi| rssi - receiver_loss_db) else {
                continue;
            };
            if !rssi_dbm.is_finite() || rssi_dbm < propagation::sensitivity(&event.channel) {
                continue;
            }

            let interfering_rssi: Vec<f64> = all_events
                .iter()
                .filter(|other| {
                    !same_transmission(event, other)
                        && transmissions_overlap(event, other)
                        && frequency_overlaps(
                            other.channel.freq_mhz,
                            receiver.current_channel.freq_mhz,
                            config.frequency_tolerance_khz,
                        )
                })
                .filter_map(|other| {
                    let distance = propagation::distance_km(other.position, receiver.position);
                    propagation::received_rssi_with_model(
                        other.tx_power_dbm,
                        distance,
                        other.channel.freq_mhz,
                        config.antenna_gain_tx_dbi,
                        config.antenna_gain_rx_dbi,
                        config.system_loss_db,
                        config.propagation_model,
                    )
                    .map(|rssi| rssi - receiver_loss_db)
                    .filter(|rssi| rssi.is_finite())
                })
                .collect();

            let interference_dbm = sum_dbm(&interfering_rssi);
            if interference_dbm.is_some_and(|level| rssi_dbm - level <= CAPTURE_THRESHOLD_DB) {
                continue;
            }

            let noise_floor_dbm =
                thermal_noise_floor_dbm(event.channel.bandwidth_khz, config.noise_figure_db);
            let noise_and_interference_dbm = sum_dbm(&[
                noise_floor_dbm,
                interference_dbm.unwrap_or(f64::NEG_INFINITY),
            ])
            .unwrap_or(noise_floor_dbm);
            receiver.inbox.push(RxPacket {
                from_node: event.node_id.clone(),
                data: event.data.clone(),
                rssi_dbm,
                // Retain the established field/API name, but report SINR when
                // overlapping transmissions contribute interference.
                snr_db: rssi_dbm - noise_and_interference_dbm,
                channel: event.channel.clone(),
                timestamp_ms: transmission_end(event),
            });
        }
    }
}

impl Default for RadioBus {
    fn default() -> Self {
        Self::new()
    }
}

fn transmission_end(event: &TxEvent) -> u64 {
    event
        .timestamp_ms
        .saturating_add(u64::from(event.airtime_ms))
}

fn transmissions_overlap(first: &TxEvent, second: &TxEvent) -> bool {
    if first.airtime_ms == 0 || second.airtime_ms == 0 {
        return false;
    }
    first.timestamp_ms < transmission_end(second) && second.timestamp_ms < transmission_end(first)
}

fn same_transmission(first: &TxEvent, second: &TxEvent) -> bool {
    first.node_id == second.node_id
        && first.timestamp_ms == second.timestamp_ms
        && first.channel == second.channel
}

fn frequency_overlaps(first_mhz: f64, second_mhz: f64, tolerance_khz: f64) -> bool {
    (first_mhz - second_mhz).abs() * 1_000.0 <= tolerance_khz
}

fn thermal_noise_floor_dbm(bandwidth_khz: u16, noise_figure_db: f64) -> f64 {
    let bandwidth_hz = f64::from(bandwidth_khz) * 1_000.0;
    THERMAL_NOISE_DENSITY_DBM_PER_HZ + 10.0 * bandwidth_hz.log10() + noise_figure_db
}

fn sum_dbm(levels: &[f64]) -> Option<f64> {
    let milliwatts: f64 = levels
        .iter()
        .filter(|level| level.is_finite())
        .map(|level| 10_f64.powf(level / 10.0))
        .sum();
    (milliwatts > 0.0).then(|| 10.0 * milliwatts.log10())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn channel_at(freq_mhz: f64) -> RadioChannel {
        RadioChannel {
            freq_mhz,
            bandwidth_khz: 125,
            spreading_factor: 7,
            coding_rate: 5,
        }
    }

    fn channel() -> RadioChannel {
        channel_at(915.0)
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
        bus.set_dio2_as_rf_switch(id, true);
    }

    #[test]
    fn dio2_rf_switch_defaults_off_and_models_tx_and_rx_loss() {
        let mut bus = RadioBus::with_config(RadioBusConfig {
            propagation_model: PropagationModel::FreeSpace,
            system_loss_db: 0.0,
            ..RadioBusConfig::default()
        });
        assert!(!bus.config().dio2_as_rf_switch);
        bus.register_node("sender".into(), (0.0, 0.0), 14.0, channel());
        bus.register_node("receiver".into(), (0.0, 0.001), 14.0, channel());
        assert_eq!(bus.dio2_as_rf_switch("sender"), Some(false));

        bus.set_dio2_as_rf_switch("receiver", true);
        assert!(bus.broadcast(transmission("sender", 1, 1_000)));
        bus.tick(1_100);
        let lossy_tx_rssi = bus.poll("receiver")[0].rssi_dbm;

        bus.set_dio2_as_rf_switch("sender", true);
        assert!(bus.broadcast(transmission("sender", 2, 1_100)));
        bus.tick(1_200);
        let normal_rssi = bus.poll("receiver")[0].rssi_dbm;
        assert!((normal_rssi - lossy_tx_rssi - SX1262_DIO2_TX_LOSS_DB).abs() < 0.001);

        bus.set_dio2_as_rf_switch("receiver", false);
        assert!(bus.broadcast(transmission("sender", 3, 1_200)));
        bus.tick(1_300);
        let lossy_rx_rssi = bus.poll("receiver")[0].rssi_dbm;
        assert!((normal_rssi - lossy_rx_rssi - SX1262_DIO2_RX_LOSS_DB).abs() < 0.001);
    }

    #[test]
    fn packet_is_delivered_only_at_end_of_airtime() {
        let mut bus = RadioBus::new();
        register(&mut bus, "alice", (51.5074, -0.1278), 14.0);
        register(&mut bus, "bob", (51.5075, -0.1278), 14.0);

        assert!(bus.broadcast(transmission("alice", 42, 1_000)));
        assert!(bus.poll("bob").is_empty());
        bus.tick(1_099);
        assert!(bus.poll("bob").is_empty());
        bus.tick(1_100);

        let packets = bus.poll("bob");
        assert_eq!(packets.len(), 1);
        assert_eq!(packets[0].data, vec![42]);
        assert_eq!(packets[0].timestamp_ms, 1_100);
    }

    #[test]
    fn disabled_receiver_drops_packets_until_reenabled() {
        let mut bus = RadioBus::new();
        register(&mut bus, "alice", (51.5074, -0.1278), 14.0);
        register(&mut bus, "bob", (51.5075, -0.1278), 14.0);
        bus.set_receive_enabled("bob", false);

        assert!(bus.broadcast(transmission("alice", 1, 1_000)));
        bus.tick(1_100);
        assert!(bus.poll("bob").is_empty());

        bus.set_receive_enabled("bob", true);
        assert!(bus.broadcast(transmission("alice", 2, 1_100)));
        bus.tick(1_200);
        assert_eq!(bus.poll("bob")[0].data, vec![2]);
    }

    #[test]
    fn transmitter_is_busy_and_overlapping_send_is_rejected() {
        let mut bus = RadioBus::new();
        register(&mut bus, "alice", (0.0, 0.0), 14.0);

        assert!(bus.broadcast(transmission("alice", 1, 1_000)));
        assert!(!bus.is_send_complete("alice", 1_099));
        assert!(!bus.broadcast(transmission("alice", 2, 1_099)));
        assert!(bus.is_send_complete("alice", 1_100));
        assert!(bus.broadcast(transmission("alice", 3, 1_100)));
    }

    #[test]
    fn collision_result_does_not_depend_on_poll_timing() {
        let mut bus = RadioBus::new();
        register(&mut bus, "alice", (0.0, -0.001), 14.0);
        register(&mut bus, "bob", (0.0, 0.001), 14.0);
        register(&mut bus, "carol", (0.0, 0.0), 14.0);

        assert!(bus.broadcast(transmission("alice", 1, 1_000)));
        assert!(bus.poll("carol").is_empty());
        assert!(bus.broadcast(transmission("bob", 2, 1_000)));
        assert!(bus.poll("carol").is_empty());
        bus.tick(1_100);
        assert!(bus.poll("carol").is_empty());
    }

    #[test]
    fn capture_effect_delivers_a_signal_more_than_six_db_stronger() {
        let mut bus = RadioBus::with_seed(7);
        register(&mut bus, "weak", (0.0, 0.01), 14.0);
        register(&mut bus, "strong", (0.0, 0.0001), 14.0);
        register(&mut bus, "receiver", (0.0, 0.0), 14.0);

        assert!(bus.broadcast(transmission("weak", 1, 1_000)));
        assert!(bus.broadcast(transmission("strong", 2, 1_000)));
        bus.tick(1_100);
        let packets = bus.poll("receiver");

        assert_eq!(packets.len(), 1);
        assert_eq!(packets[0].from_node, "strong");
    }

    #[test]
    fn nearby_frequency_with_different_modulation_still_interferes() {
        let mut bus = RadioBus::with_config(RadioBusConfig {
            propagation_model: PropagationModel::FreeSpace,
            system_loss_db: 0.0,
            frequency_tolerance_khz: 2.0,
            ..RadioBusConfig::default()
        });
        register(&mut bus, "wanted", (0.0, -0.001), 14.0);
        register(&mut bus, "receiver", (0.0, 0.0), 14.0);
        bus.register_node(
            "interferer".into(),
            (0.0, 0.001),
            14.0,
            RadioChannel {
                spreading_factor: 8,
                ..channel_at(915.001)
            },
        );
        bus.set_dio2_as_rf_switch("interferer", true);

        assert!(bus.broadcast(transmission("wanted", 1, 1_000)));
        let mut interference = transmission("interferer", 2, 1_000);
        interference.channel = channel_at(915.001);
        assert!(bus.broadcast(interference));
        bus.tick(1_100);

        assert!(bus.poll("receiver").is_empty());
    }

    #[test]
    fn aggregate_below_sensitivity_interferers_can_corrupt_a_packet() {
        let mut bus = RadioBus::with_config(RadioBusConfig {
            propagation_model: PropagationModel::FixedRange {
                max_distance_km: 10.0,
            },
            system_loss_db: 0.0,
            ..RadioBusConfig::default()
        });
        register(&mut bus, "wanted", (0.0, 0.001), -118.0);
        register(&mut bus, "interferer-a", (0.0, 0.001), -125.0);
        register(&mut bus, "interferer-b", (0.0, 0.001), -125.0);
        register(&mut bus, "receiver", (0.0, 0.0), 14.0);

        // Each interferer is below the SF7 sensitivity threshold (-123 dBm),
        // but together they exceed the six-decibel capture margin.
        assert!(bus.broadcast(transmission("wanted", 1, 1_000)));
        assert!(bus.broadcast(transmission("interferer-a", 2, 1_000)));
        assert!(bus.broadcast(transmission("interferer-b", 3, 1_000)));
        bus.tick(1_100);

        assert!(bus.poll("receiver").is_empty());
    }

    #[test]
    fn reported_snr_is_sinr_when_capture_succeeds() {
        let mut bus = RadioBus::with_config(RadioBusConfig {
            propagation_model: PropagationModel::FixedRange {
                max_distance_km: 10.0,
            },
            system_loss_db: 0.0,
            ..RadioBusConfig::default()
        });
        register(&mut bus, "wanted", (0.0, 0.001), -100.0);
        register(&mut bus, "interferer", (0.0, 0.001), -110.0);
        register(&mut bus, "receiver", (0.0, 0.0), 14.0);

        assert!(bus.broadcast(transmission("wanted", 1, 1_000)));
        assert!(bus.broadcast(transmission("interferer", 2, 1_000)));
        bus.tick(1_100);

        let packets = bus.poll("receiver");
        assert_eq!(packets.len(), 1);
        assert_eq!(packets[0].from_node, "wanted");
        // Interference dominates the roughly -117 dBm thermal noise floor.
        assert!(packets[0].snr_db > 9.0 && packets[0].snr_db < 10.0);
    }

    #[test]
    fn wider_bandwidth_raises_integrated_thermal_noise() {
        let narrow = thermal_noise_floor_dbm(125, 6.0);
        let wide = thermal_noise_floor_dbm(500, 6.0);

        assert!((narrow - -117.0309).abs() < 0.001);
        assert!((wide - -111.0103).abs() < 0.001);
        assert!((wide - narrow - 6.0206).abs() < 0.001);
    }

    #[test]
    fn fixed_range_model_applies_a_simple_cutoff() {
        let mut bus = RadioBus::with_config(RadioBusConfig {
            propagation_model: PropagationModel::FixedRange {
                max_distance_km: 1.0,
            },
            system_loss_db: 0.0,
            ..RadioBusConfig::default()
        });
        register(&mut bus, "alice", (0.0, 0.0), 14.0);
        register(&mut bus, "near", (0.0, 0.005), 14.0);
        register(&mut bus, "far", (0.0, 0.02), 14.0);

        assert!(bus.broadcast(transmission("alice", 1, 0)));
        bus.tick(100);
        assert_eq!(bus.poll("near").len(), 1);
        assert!(bus.poll("far").is_empty());
    }
}
