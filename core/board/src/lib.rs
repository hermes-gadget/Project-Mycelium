//! Board-level power, telemetry, and buzzer emulation.

mod buzzer;

use std::collections::HashMap;
use std::sync::{LazyLock, Mutex, MutexGuard};

pub use buzzer::{
    get_buzzer, meshemu_buzzer_beep, meshemu_buzzer_is_playing, meshemu_buzzer_stop,
    register_buzzer, remove_buzzer, SharedVirtualBuzzer, VirtualBuzzer,
};

pub const BD_STARTUP_NORMAL: u8 = 0;
pub const BATTERY_ADC_GPIO: u8 = 4;
pub const PERIPH_PWR_EN_GPIO: u8 = 10;
pub const BUZZER_GPIO: u8 = 46;
pub const ADC_MAX_COUNT: u16 = 4_095;
pub const BATTERY_MV_PER_ADC_COUNT: f64 = 1.61;
pub const TP4054_FULL_MV: u16 = 4_200;

static PERIPHERAL_POWER: LazyLock<Mutex<HashMap<String, bool>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

pub fn peripherals_powered(instance_id: &str) -> bool {
    lock(&PERIPHERAL_POWER)
        .get(instance_id)
        .copied()
        .unwrap_or(true)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub enum Tp4054State {
    Charging = 0,
    Charged = 1,
    NoBattery = 2,
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct LedcChannel {
    gpio: u8,
    period_us: u32,
    high_time_us: u32,
}

#[derive(Clone, Debug, PartialEq)]
pub struct VirtualBoard {
    pub battery_mv: u16,
    pub mcu_temperature: f32,
    pub manufacturer: String,
    pub startup_reason: u8,
    pub external_powered: bool,
    pub periph_pwr_enabled: bool,
    pub instance_id: String,
    ledc_channels: HashMap<u8, LedcChannel>,
}

impl VirtualBoard {
    pub fn new(instance_id: &str, config: BoardConfig) -> Self {
        lock(&PERIPHERAL_POWER).insert(instance_id.to_owned(), config.periph_pwr_enabled);
        Self {
            battery_mv: config.battery_mv,
            mcu_temperature: config.mcu_temperature,
            manufacturer: config.manufacturer,
            startup_reason: config.startup_reason,
            external_powered: config.external_powered,
            periph_pwr_enabled: config.periph_pwr_enabled,
            instance_id: instance_id.to_owned(),
            ledc_channels: HashMap::new(),
        }
    }

    pub fn get_battery_mv(&self) -> u16 {
        self.battery_mv
    }

    /// Reads the 12-bit ADC wired to the battery divider on GPIO4.
    pub fn get_adc(&self, gpio: u8) -> u16 {
        if gpio != BATTERY_ADC_GPIO {
            return 0;
        }
        (f64::from(self.battery_mv) / BATTERY_MV_PER_ADC_COUNT)
            .round()
            .clamp(0.0, f64::from(ADC_MAX_COUNT)) as u16
    }

    pub fn get_temperature(&self) -> f32 {
        self.mcu_temperature
    }

    pub fn set_battery(&mut self, mv: u16) {
        self.battery_mv = mv;
    }

    pub fn set_external_power(&mut self, powered: bool) {
        self.external_powered = powered;
    }

    pub fn charger_state(&self) -> Tp4054State {
        if self.battery_mv == 0 {
            Tp4054State::NoBattery
        } else if self.external_powered && self.battery_mv < TP4054_FULL_MV {
            Tp4054State::Charging
        } else {
            Tp4054State::Charged
        }
    }

    /// Advances TP4054 charging using the same simple mAh-to-mV model as
    /// discharge. Charging stops at the 4.2 V termination voltage.
    pub fn simulate_charge(&mut self, dt_ms: u64, charge_current_ma: f64) {
        if self.charger_state() != Tp4054State::Charging
            || !charge_current_ma.is_finite()
            || charge_current_ma <= 0.0
        {
            return;
        }
        let charged_mah = charge_current_ma * dt_ms as f64 / 3_600_000.0;
        let rise_mv = charged_mah.round().clamp(0.0, f64::from(u16::MAX)) as u16;
        self.battery_mv = self.battery_mv.saturating_add(rise_mv).min(TP4054_FULL_MV);
    }

    pub fn digital_write(&mut self, gpio: u8, high: bool) {
        if gpio != PERIPH_PWR_EN_GPIO {
            return;
        }
        self.periph_pwr_enabled = high;
        lock(&PERIPHERAL_POWER).insert(self.instance_id.clone(), high);
        if !high {
            if let Some(buzzer) = get_buzzer(&self.instance_id) {
                lock(&buzzer).stop();
            }
        }
    }

    pub fn ledc_attach(&mut self, channel: u8, gpio: u8) {
        self.ledc_channels.insert(
            channel,
            LedcChannel {
                gpio,
                period_us: 0,
                high_time_us: 0,
            },
        );
    }

    /// Applies one LEDC PWM waveform and routes GPIO46 transitions to the
    /// buzzer. Frequency is derived from the period and volume from duty.
    pub fn ledc_write(&mut self, channel: u8, period_us: u32, high_time_us: u32) -> bool {
        let Some(ledc) = self.ledc_channels.get_mut(&channel) else {
            return false;
        };
        ledc.period_us = period_us;
        ledc.high_time_us = high_time_us.min(period_us);
        if ledc.gpio != BUZZER_GPIO {
            return true;
        }
        let Some(buzzer) = get_buzzer(&self.instance_id) else {
            return false;
        };
        let mut buzzer = lock(&buzzer);
        if !self.periph_pwr_enabled || period_us == 0 {
            buzzer.stop();
            return true;
        }
        let frequency_hz = (1_000_000_f64 / f64::from(period_us)).round() as u32;
        let duty_cycle = ledc.high_time_us as f32 / period_us as f32;
        buzzer.drive_pwm(frequency_hz, duty_cycle);
        true
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
    pub periph_pwr_enabled: bool,
}

impl Default for BoardConfig {
    fn default() -> Self {
        Self {
            battery_mv: 3_900,
            mcu_temperature: 35.0,
            manufacturer: "Mycelium Virtual T-Deck".to_owned(),
            startup_reason: BD_STARTUP_NORMAL,
            external_powered: false,
            periph_pwr_enabled: true,
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
        assert_eq!(board.get_adc(BATTERY_ADC_GPIO), 2_329);
        assert_eq!(board.get_adc(5), 0);
        assert_eq!(board.get_temperature(), 35.0);
    }

    #[test]
    fn battery_adc_quantizes_and_saturates_at_twelve_bits() {
        let mut board = VirtualBoard::new("adc-node", BoardConfig::default());
        assert_eq!(board.get_adc(BATTERY_ADC_GPIO), 2_422);

        board.set_battery(u16::MAX);
        assert_eq!(board.get_adc(BATTERY_ADC_GPIO), ADC_MAX_COUNT);
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

    #[test]
    fn external_power_drives_tp4054_states_and_charging() {
        let mut board = VirtualBoard::new("charger-node", BoardConfig::default());
        assert_eq!(board.charger_state(), Tp4054State::Charged);

        board.set_external_power(true);
        assert_eq!(board.charger_state(), Tp4054State::Charging);
        board.simulate_charge(3_600_000, 500.0);
        assert_eq!(board.get_battery_mv(), TP4054_FULL_MV);
        assert_eq!(board.charger_state(), Tp4054State::Charged);

        board.set_battery(0);
        assert_eq!(board.charger_state(), Tp4054State::NoBattery);
        board.simulate_charge(3_600_000, 500.0);
        assert_eq!(board.get_battery_mv(), 0);
    }

    #[test]
    fn gpio10_gates_gpio46_ledc_buzzer_without_input_interrupts() {
        let id = "power-pwm-node";
        let buzzer = register_buzzer(id);
        let mut board = VirtualBoard::new(id, BoardConfig::default());
        board.ledc_attach(2, BUZZER_GPIO);

        assert!(board.ledc_write(2, 500, 125));
        assert!(lock(&buzzer).is_playing());
        assert_eq!(lock(&buzzer).frequency_hz(), 2_000);
        assert_eq!(lock(&buzzer).duty_cycle(), 0.25);

        board.digital_write(PERIPH_PWR_EN_GPIO, false);
        assert!(!lock(&buzzer).is_playing());
        assert!(!peripherals_powered(id));
        assert!(board.ledc_write(2, 1_000, 500));
        assert!(!lock(&buzzer).is_playing());

        board.digital_write(PERIPH_PWR_EN_GPIO, true);
        assert!(peripherals_powered(id));
        assert!(board.ledc_write(2, 1_000, 500));
        assert!(lock(&buzzer).is_playing());
        remove_buzzer(id);
    }
}
