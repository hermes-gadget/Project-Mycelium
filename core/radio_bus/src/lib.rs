//! Virtual LoRa radio propagation for Project Mycelium nodes.

pub mod bus;
pub mod channel;
pub mod propagation;
pub mod types;

pub use bus::{RadioBus, RadioBusConfig};
pub use channel::ChannelState;
pub use propagation::{HataEnvironment, PropagationModel};
pub use types::{
    RadioChannel, RxPacket, Sx1262State, TxEvent, SENSITIVITY_DBM, SX1262_DEFAULT_BANDWIDTH_KHZ,
    SX1262_DEFAULT_CODING_RATE, SX1262_DEFAULT_FREQUENCY_MHZ, SX1262_DEFAULT_SPREADING_FACTOR,
    SX1262_DEFAULT_TCXO_VOLTAGE_V, SX1262_DEFAULT_TX_POWER_DBM,
};
