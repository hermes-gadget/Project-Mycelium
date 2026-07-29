//! Board-level power, telemetry, and buzzer emulation.

mod buzzer;
pub mod nvs;
pub mod partition;

use std::collections::HashMap;
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::{LazyLock, Mutex, MutexGuard};

pub use buzzer::{
    get_buzzer, meshemu_buzzer_beep, meshemu_buzzer_is_playing, meshemu_buzzer_stop,
    register_buzzer, remove_buzzer, SharedVirtualBuzzer, VirtualBuzzer,
};
pub use nvs::{
    get_nvs, register_nvs, remove_nvs, SharedVirtualNvs, VirtualNvs, LAUNCHER_NVS_SIZE,
    NVS_NAME_MAX_BYTES, STANDALONE_NVS_SIZE,
};
pub use partition::{
    activate_partition_table, active_partition_table, get_partition_table,
    register_partition_table, remove_partition_table, SharedVirtualPartitionTable,
    VirtualPartition, VirtualPartitionTable,
};

pub const BD_STARTUP_NORMAL: u8 = 0;
pub const BATTERY_ADC_GPIO: u8 = 4;
pub const PERIPH_PWR_EN_GPIO: u8 = 10;
pub const BUZZER_GPIO: u8 = 46;
pub const ADC_MAX_COUNT: u16 = 4_095;
pub const ADC_REFERENCE_MV: f64 = 3_300.0;
pub const BATTERY_DIVIDER_RATIO: f64 = 2.0;
pub const BATTERY_MV_PER_ADC_COUNT: f64 =
    ADC_REFERENCE_MV * BATTERY_DIVIDER_RATIO / (ADC_MAX_COUNT as f64 + 1.0);
pub const TP4054_FULL_MV: u16 = 4_200;
pub const DEFAULT_PSRAM_SIZE_BYTES: u32 = 8_388_608;
pub const SLEEP_WAKE_CAUSE_UNKNOWN: u8 = 0;
pub const SLEEP_WAKE_CAUSE_TIMER: u8 = 1;
pub const SLEEP_WAKE_CAUSE_EXT1: u8 = 2;
pub const SLEEP_WAKE_CAUSE_TIMER_EXT1: u8 = 3;

static LAST_BOOT_PHASE: AtomicU8 = AtomicU8::new(0);

// Representative ESP32-S3 ADC1 11 dB eFuse calibration point. Combined with
// Espressif's curve-fit coefficients below, a 2.1 V GPIO4 divider input
// produces about 2345 raw counts and reproduces the documented 4.2 V → 3.78 V
// failure of the naive 3.3 V / 4096 conversion.
const ADC_EFUSE_CAL_VOLTAGE_MV: f64 = 850.0;
const ADC_EFUSE_CAL_RAW_COUNT: f64 = 929.0;

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

pub fn set_boot_phase(phase: u8) {
    LAST_BOOT_PHASE.store(phase, Ordering::Release);
}

pub fn last_boot_phase() -> u8 {
    LAST_BOOT_PHASE.load(Ordering::Acquire)
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
    pub psram_size_bytes: u32,
    pub manufacturer: String,
    pub startup_reason: u8,
    pub external_powered: bool,
    pub periph_pwr_enabled: bool,
    pub adc_calibrated: bool,
    pub instance_id: String,
    psram_used_bytes: u32,
    psram_region: Vec<u8>,
    rtc_gpio_holds: HashMap<u8, bool>,
    ledc_channels: HashMap<u8, LedcChannel>,
}

impl VirtualBoard {
    pub fn new(instance_id: &str, config: BoardConfig) -> Self {
        lock(&PERIPHERAL_POWER).insert(instance_id.to_owned(), config.periph_pwr_enabled);
        Self {
            battery_mv: config.battery_mv,
            mcu_temperature: config.mcu_temperature,
            psram_size_bytes: DEFAULT_PSRAM_SIZE_BYTES,
            manufacturer: config.manufacturer,
            startup_reason: config.startup_reason,
            external_powered: config.external_powered,
            periph_pwr_enabled: config.periph_pwr_enabled,
            adc_calibrated: config.adc_calibrated,
            instance_id: instance_id.to_owned(),
            psram_used_bytes: 0,
            psram_region: Vec::new(),
            rtc_gpio_holds: HashMap::new(),
            ledc_channels: HashMap::new(),
        }
    }

    pub fn get_battery_mv(&self) -> u16 {
        self.battery_mv
    }

    /// Reads a 12-bit ADC count from the battery divider on GPIO4.
    ///
    /// Uncalibrated mode exposes the nonlinear ESP32-S3 raw count. Calibrated
    /// mode applies the same ADC1/11 dB curve fitting used by
    /// `analogReadMilliVolts()` and returns the equivalent linearized count.
    pub fn get_adc(&self, gpio: u8) -> u16 {
        if gpio != BATTERY_ADC_GPIO {
            return 0;
        }
        let pin_mv = f64::from(self.battery_mv) / BATTERY_DIVIDER_RATIO;
        let raw_count = s3_raw_count_for_pin_mv(pin_mv);
        if !self.adc_calibrated {
            return raw_count;
        }
        if raw_count == ADC_MAX_COUNT && pin_mv >= s3_calibrated_pin_mv(ADC_MAX_COUNT) {
            return ADC_MAX_COUNT;
        }

        let corrected_battery_mv = s3_calibrated_pin_mv(raw_count) * BATTERY_DIVIDER_RATIO;
        (corrected_battery_mv / BATTERY_MV_PER_ADC_COUNT)
            .round()
            .clamp(0.0, f64::from(ADC_MAX_COUNT)) as u16
    }

    pub fn get_temperature(&self) -> f32 {
        self.mcu_temperature
    }

    pub fn set_battery(&mut self, mv: u16) {
        self.battery_mv = mv;
    }

    pub fn psram_found(&self) -> bool {
        self.psram_size_bytes > 0
    }

    pub fn psram_used_bytes(&self) -> u32 {
        self.psram_used_bytes.min(self.psram_size_bytes)
    }

    pub fn psram_free_bytes(&self) -> u32 {
        self.psram_size_bytes
            .saturating_sub(self.psram_used_bytes())
    }

    /// Reserves bytes from external RAM so host adapters can model firmware
    /// allocations and expose the resulting pressure through free-PSRAM
    /// telemetry.
    pub fn reserve_psram(&mut self, bytes: u32) -> bool {
        if bytes > self.psram_free_bytes() {
            return false;
        }
        self.psram_used_bytes = self.psram_used_bytes.saturating_add(bytes);
        true
    }

    pub fn release_psram(&mut self, bytes: u32) {
        self.psram_used_bytes = self.psram_used_bytes.saturating_sub(bytes);
    }

    /// Writes a deterministic pattern into an allocatable PSRAM region,
    /// verifies every byte, restores the previous contents, and releases the
    /// temporary reservation.
    pub fn psram_readback_test(&mut self) -> bool {
        const READBACK_BYTES: u32 = 64;

        let test_bytes = self.psram_free_bytes().min(READBACK_BYTES);
        if test_bytes == 0 || !self.reserve_psram(test_bytes) {
            return false;
        }

        let start = self.psram_used_bytes.saturating_sub(test_bytes) as usize;
        let end = start.saturating_add(test_bytes as usize);
        if self.psram_region.len() < end {
            self.psram_region.resize(end, 0);
        }
        let previous = self.psram_region[start..end].to_vec();
        for (index, byte) in self.psram_region[start..end].iter_mut().enumerate() {
            *byte = 0xA5 ^ (index as u8).wrapping_mul(0x3D);
        }
        let verified = self.psram_region[start..end]
            .iter()
            .enumerate()
            .all(|(index, byte)| *byte == (0xA5 ^ (index as u8).wrapping_mul(0x3D)));
        self.psram_region[start..end].copy_from_slice(&previous);
        self.release_psram(test_bytes);
        verified
    }

    pub fn rtc_gpio_hold(&mut self, gpio: u8, level: bool) {
        self.rtc_gpio_holds.insert(gpio, level);
    }

    pub fn rtc_gpio_hold_level(&self, gpio: u8) -> Option<bool> {
        self.rtc_gpio_holds.get(&gpio).copied()
    }

    pub fn set_adc_calibration(&mut self, calibrated: bool) {
        self.adc_calibrated = calibrated;
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
    pub adc_calibrated: bool,
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
            adc_calibrated: true,
        }
    }
}

/// Espressif's ESP32-S3 ADC1/11 dB curve-fit error polynomial.
fn s3_adc_error_mv(first_step_mv: f64) -> f64 {
    let x = first_step_mv;
    -0.644_403_418_269_478 - 0.064_433_488_864_753_6 * x + 0.000_129_789_144_761_1 * x.powi(2)
        - 0.000_000_070_769_718 * x.powi(3)
        + 0.000_000_000_013_515 * x.powi(4)
}

fn s3_calibrated_pin_mv(raw_count: u16) -> f64 {
    if raw_count == 0 {
        return 0.0;
    }
    let first_step_mv = f64::from(raw_count) * ADC_EFUSE_CAL_VOLTAGE_MV / ADC_EFUSE_CAL_RAW_COUNT;
    first_step_mv - s3_adc_error_mv(first_step_mv)
}

fn s3_raw_count_for_pin_mv(pin_mv: f64) -> u16 {
    if !pin_mv.is_finite() || pin_mv <= 0.0 {
        return 0;
    }
    if pin_mv >= s3_calibrated_pin_mv(ADC_MAX_COUNT) {
        return ADC_MAX_COUNT;
    }

    let mut low = 0_u16;
    let mut high = ADC_MAX_COUNT;
    while low + 1 < high {
        let middle = low + (high - low) / 2;
        if s3_calibrated_pin_mv(middle) < pin_mv {
            low = middle;
        } else {
            high = middle;
        }
    }
    let low_error = (s3_calibrated_pin_mv(low) - pin_mv).abs();
    let high_error = (s3_calibrated_pin_mv(high) - pin_mv).abs();
    if low_error <= high_error {
        low
    } else {
        high
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
        assert_eq!(board.get_adc(BATTERY_ADC_GPIO), 2_327);
        assert_eq!(board.get_adc(5), 0);
        assert_eq!(board.get_temperature(), 35.0);
    }

    #[test]
    fn psram_defaults_to_eight_megabytes_and_tracks_pressure() {
        let mut board = VirtualBoard::new("psram-node", BoardConfig::default());

        assert!(board.psram_found());
        assert_eq!(board.psram_size_bytes, DEFAULT_PSRAM_SIZE_BYTES);
        assert_eq!(board.psram_used_bytes(), 0);
        assert_eq!(board.psram_free_bytes(), DEFAULT_PSRAM_SIZE_BYTES);
        assert!(board.reserve_psram(1_048_576));
        assert_eq!(board.psram_used_bytes(), 1_048_576);
        assert_eq!(
            board.psram_free_bytes(),
            DEFAULT_PSRAM_SIZE_BYTES - 1_048_576
        );
        assert!(!board.reserve_psram(DEFAULT_PSRAM_SIZE_BYTES));

        board.release_psram(524_288);
        assert_eq!(board.psram_used_bytes(), 524_288);
        board.release_psram(u32::MAX);
        assert_eq!(board.psram_used_bytes(), 0);
    }

    #[test]
    fn psram_readback_uses_allocatable_memory_without_leaking_it() {
        let mut board = VirtualBoard::new("psram-readback", BoardConfig::default());
        assert!(board.reserve_psram(123_456));
        let free_before = board.psram_free_bytes();

        assert!(board.psram_readback_test());
        assert_eq!(board.psram_free_bytes(), free_before);

        assert!(board.reserve_psram(free_before));
        assert!(!board.psram_readback_test());
        board.psram_size_bytes = 0;
        assert!(!board.psram_found());
        assert_eq!(board.psram_free_bytes(), 0);
        assert!(!board.psram_readback_test());
    }

    #[test]
    fn rtc_gpio_holds_remember_each_pin_level() {
        let mut board = VirtualBoard::new("rtc-hold-node", BoardConfig::default());
        assert_eq!(board.rtc_gpio_hold_level(9), None);

        board.rtc_gpio_hold(9, true);
        board.rtc_gpio_hold(45, false);

        assert_eq!(board.rtc_gpio_hold_level(9), Some(true));
        assert_eq!(board.rtc_gpio_hold_level(45), Some(false));
    }

    #[test]
    fn boot_phase_survives_board_recreation() {
        set_boot_phase(17);
        let board = VirtualBoard::new("boot-phase-node", BoardConfig::default());
        drop(board);
        let _restarted = VirtualBoard::new("boot-phase-node", BoardConfig::default());

        assert_eq!(last_boot_phase(), 17);
    }

    #[test]
    fn battery_adc_quantizes_and_saturates_at_twelve_bits() {
        let mut board = VirtualBoard::new("adc-node", BoardConfig::default());
        assert!(board.adc_calibrated);
        assert_eq!(board.get_adc(BATTERY_ADC_GPIO), 2_420);

        board.set_battery(u16::MAX);
        assert_eq!(board.get_adc(BATTERY_ADC_GPIO), ADC_MAX_COUNT);
    }

    #[test]
    fn adc_curve_reproduces_uncalibrated_full_cell_under_read() {
        let mut board = VirtualBoard::new(
            "adc-curve-node",
            BoardConfig {
                battery_mv: TP4054_FULL_MV,
                ..BoardConfig::default()
            },
        );

        board.set_adc_calibration(false);
        let uncalibrated_mv = f64::from(board.get_adc(BATTERY_ADC_GPIO)) * BATTERY_MV_PER_ADC_COUNT;
        assert!((uncalibrated_mv - 3_780.0).abs() < 5.0);

        board.set_adc_calibration(true);
        let calibrated_mv = f64::from(board.get_adc(BATTERY_ADC_GPIO)) * BATTERY_MV_PER_ADC_COUNT;
        assert!((calibrated_mv - 4_200.0).abs() < 5.0);
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
