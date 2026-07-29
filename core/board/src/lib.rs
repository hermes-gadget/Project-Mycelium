//! Board-level emulation.

mod buzzer;

pub use buzzer::{
    get_buzzer, meshemu_buzzer_beep, meshemu_buzzer_is_playing, meshemu_buzzer_stop,
    register_buzzer, remove_buzzer, SharedVirtualBuzzer, VirtualBuzzer,
};

#[derive(Clone, Debug)]
pub struct BoardConfig {
    pub battery_mv: u16,
    pub mcu_temperature: f32,
    pub manufacturer: String,
    pub startup_reason: u8,
    pub external_powered: bool,
}

impl Default for BoardConfig {
    fn default() -> Self {
        Self {
            battery_mv: 3_700,
            mcu_temperature: 35.0,
            manufacturer: "Mycelium Virtual T-Deck".to_owned(),
            startup_reason: 0,
            external_powered: false,
        }
    }
}

#[derive(Debug)]
pub struct VirtualBoard {
    instance_id: String,
    config: BoardConfig,
}

impl VirtualBoard {
    pub fn new(instance_id: impl Into<String>, config: BoardConfig) -> Self {
        Self {
            instance_id: instance_id.into(),
            config,
        }
    }

    pub fn instance_id(&self) -> &str {
        &self.instance_id
    }

    pub fn battery_mv(&self) -> u16 {
        self.config.battery_mv
    }

    pub fn config(&self) -> &BoardConfig {
        &self.config
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn board_creation_uses_supplied_configuration() {
        let board = VirtualBoard::new(
            "node1",
            BoardConfig {
                battery_mv: 4_100,
                ..BoardConfig::default()
            },
        );

        assert_eq!(board.instance_id(), "node1");
        assert_eq!(board.battery_mv(), 4_100);
    }
}
