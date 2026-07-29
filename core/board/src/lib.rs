//! Board-level power, telemetry, and buzzer emulation.

mod buzzer;

pub use buzzer::{
    get_buzzer, meshemu_buzzer_beep, meshemu_buzzer_is_playing, meshemu_buzzer_stop,
    register_buzzer, remove_buzzer, SharedVirtualBuzzer, VirtualBuzzer,
};

pub const BD_STARTUP_NORMAL: u8 = 0;

#[derive(Clone, Debug, PartialEq)]
pub struct VirtualBoard {
    pub battery_mv: u16,
    pub mcu_temperature: f32,
    pub manufacturer: String,
    pub startup_reason: u8,
    pub external_powered: bool,
    pub instance_id: String,
}

impl VirtualBoard {
    pub fn new(instance_id: &str, config: BoardConfig) -> Self {
        Self {
            battery_mv: config.battery_mv,
            mcu_temperature: config.mcu_temperature,
            manufacturer: config.manufacturer,
            startup_reason: config.startup_reason,
            external_powered: config.external_powered,
            instance_id: instance_id.to_owned(),
        }
    }

    pub fn get_battery_mv(&self) -> u16 {
        self.battery_mv
    }

    pub fn get_temperature(&self) -> f32 {
        self.mcu_temperature
    }

    pub fn set_battery(&mut self, mv: u16) {
        self.battery_mv = mv;
    }

    /// Discharge using a simple one-mAh-per-millivolt virtual battery model.
    ///
    /// For example, a 100 mA load for one hour lowers the reported voltage by
    /// 100 mV. External power prevents discharge.
    pub fn simulate_discharge(&mut self, dt_ms: u64, current_ma: f64) {
        if self.external_powered || !current_ma.is_finite() || current_ma <= 0.0 {
            return;
        }
        let consumed_mah = current_ma * dt_ms as f64 / 3_600_000.0;
        let drop_mv = consumed_mah.round().clamp(0.0, f64::from(u16::MAX)) as u16;
        self.battery_mv = self.battery_mv.saturating_sub(drop_mv);
    }
}

#[derive(Clone, Debug, PartialEq)]
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
            battery_mv: 3_900,
            mcu_temperature: 35.0,
            manufacturer: "Mycelium Virtual T-Deck".to_owned(),
            startup_reason: BD_STARTUP_NORMAL,
            external_powered: false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn battery_can_be_read_and_set() {
        let mut board = VirtualBoard::new("node-1", BoardConfig::default());
        assert_eq!(board.get_battery_mv(), 3_900);

        board.set_battery(3_750);

        assert_eq!(board.get_battery_mv(), 3_750);
        assert_eq!(board.get_temperature(), 35.0);
    }

    #[test]
    fn battery_discharge_saturates_and_external_power_prevents_it() {
        let mut board = VirtualBoard::new("node-1", BoardConfig::default());
        board.simulate_discharge(3_600_000, 100.0);
        assert_eq!(board.get_battery_mv(), 3_800);

        board.external_powered = true;
        board.simulate_discharge(3_600_000, 100.0);
        assert_eq!(board.get_battery_mv(), 3_800);

        board.external_powered = false;
        board.simulate_discharge(3_600_000, 10_000.0);
        assert_eq!(board.get_battery_mv(), 0);
    }
}
