use std::collections::HashMap;
use std::f32::consts::TAU;
use std::ffi::{c_char, CStr};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Sender};
use std::sync::{Arc, LazyLock, Mutex, MutexGuard};
use std::time::Duration;

use sdl2::audio::{AudioCallback, AudioSpecDesired};
use tracing::{info, warn};

use crate::peripherals_powered;

pub type SharedVirtualBuzzer = Arc<Mutex<VirtualBuzzer>>;

static BUZZERS: LazyLock<Mutex<HashMap<String, SharedVirtualBuzzer>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// A virtual tone buzzer backed by the host's default audio device.
///
/// Audio initialization is best-effort so the emulator remains usable on
/// headless systems. Tone lifecycle state is maintained even without audio.
pub struct VirtualBuzzer {
    playing: Arc<AtomicBool>,
    frequency_hz: u32,
    duration_ms: u64,
    duty_cycle: f32,
    audio_tx: Option<Sender<AudioCommand>>,
}

enum AudioCommand {
    Beep {
        frequency_hz: u32,
        duty_cycle: f32,
        duration_ms: Option<u64>,
    },
    Stop,
}

#[derive(Default)]
struct ToneState {
    frequency_hz: u32,
    duty_cycle: f32,
    playing: bool,
}

struct SineCallback {
    state: Arc<Mutex<ToneState>>,
    phase: f32,
    sample_rate: f32,
}

impl AudioCallback for SineCallback {
    type Channel = f32;

    fn callback(&mut self, output: &mut [f32]) {
        let state = lock(&self.state);
        if !state.playing || state.frequency_hz == 0 {
            output.fill(0.0);
            return;
        }
        let phase_step = TAU * state.frequency_hz as f32 / self.sample_rate;
        for sample in output {
            *sample = self.phase.sin() * state.duty_cycle;
            self.phase = (self.phase + phase_step) % TAU;
        }
    }
}

fn start_audio_worker(playing: Arc<AtomicBool>) -> Option<(Sender<AudioCommand>, bool)> {
    let (command_tx, command_rx) = mpsc::channel();
    let (ready_tx, ready_rx) = mpsc::sync_channel(0);

    std::thread::Builder::new()
        .name("mycelium-buzzer-audio".to_owned())
        .spawn(move || {
            let state = Arc::new(Mutex::new(ToneState::default()));
            let result = (|| -> Result<_, String> {
                let sdl = sdl2::init()?;
                let audio = sdl.audio()?;
                let desired = AudioSpecDesired {
                    freq: Some(44_100),
                    channels: Some(1),
                    samples: Some(1_024),
                };
                let callback_state = Arc::clone(&state);
                let device = audio.open_playback(None, &desired, move |spec| SineCallback {
                    state: callback_state,
                    phase: 0.0,
                    sample_rate: spec.freq as f32,
                })?;
                Ok((sdl, audio, device))
            })();

            let audio_resources = result.ok();
            let audio_enabled = audio_resources.is_some();
            if let Some((_, _, device)) = audio_resources.as_ref() {
                device.resume();
            }
            // Keep the timer worker alive even when the caller timed out while
            // host audio was initializing. The channel remains useful for
            // lifecycle state in trace-only/headless mode.
            let _ = ready_tx.send(audio_enabled);

            let mut deadline: Option<std::time::Instant> = None;
            loop {
                let timeout = deadline.map_or(Duration::from_secs(3600), |deadline| {
                    deadline
                        .checked_duration_since(std::time::Instant::now())
                        .unwrap_or_default()
                });
                match command_rx.recv_timeout(timeout) {
                    Ok(AudioCommand::Beep {
                        frequency_hz,
                        duty_cycle,
                        duration_ms,
                    }) => {
                        let mut state = lock(&state);
                        state.frequency_hz = frequency_hz;
                        state.duty_cycle = duty_cycle;
                        state.playing = true;
                        playing.store(true, Ordering::Release);
                        deadline = duration_ms
                            .filter(|duration| *duration > 0)
                            .map(|duration| {
                                std::time::Instant::now() + Duration::from_millis(duration)
                            });
                    }
                    Ok(AudioCommand::Stop) => {
                        lock(&state).playing = false;
                        deadline = None;
                    }
                    Err(mpsc::RecvTimeoutError::Timeout) => {
                        lock(&state).playing = false;
                        playing.store(false, Ordering::Release);
                        deadline = None;
                    }
                    Err(mpsc::RecvTimeoutError::Disconnected) => break,
                }
            }
        })
        .ok()?;

    let audio_enabled = ready_rx
        .recv_timeout(Duration::from_millis(500))
        .unwrap_or(false);
    Some((command_tx, audio_enabled))
}

impl VirtualBuzzer {
    pub fn new() -> Self {
        let playing = Arc::new(AtomicBool::new(false));
        let worker = start_audio_worker(Arc::clone(&playing));
        if worker.is_none() {
            warn!("host audio unavailable; buzzer will be trace-only");
        }
        if worker.as_ref().is_some_and(|(_, enabled)| !enabled) {
            warn!("host audio unavailable; buzzer will be trace-only");
        }
        let audio_tx = worker.map(|(sender, _)| sender);

        Self {
            playing,
            frequency_hz: 0,
            duration_ms: 0,
            duty_cycle: 0.0,
            audio_tx,
        }
    }

    /// Starts a sine-wave tone, replacing any tone already in progress.
    pub fn beep(&mut self, frequency_hz: u32, duration_ms: u64) {
        self.start_tone(frequency_hz, 0.5, Some(duration_ms));
    }

    /// Drives the buzzer from a PWM waveform. A zero or full duty cycle has no
    /// transitions and therefore produces no tone.
    pub fn drive_pwm(&mut self, frequency_hz: u32, duty_cycle: f32) {
        self.start_tone(frequency_hz, duty_cycle, None);
    }

    fn start_tone(&mut self, frequency_hz: u32, duty_cycle: f32, duration_ms: Option<u64>) {
        self.stop();
        self.frequency_hz = frequency_hz;
        self.duration_ms = duration_ms.unwrap_or(0);
        self.duty_cycle = duty_cycle.clamp(0.0, 1.0);
        let has_transitions = self.duty_cycle > 0.0 && self.duty_cycle < 1.0;
        let has_duration = duration_ms.is_none_or(|duration| duration > 0);
        let playing = frequency_hz > 0 && has_transitions && has_duration;
        self.playing.store(playing, Ordering::Release);

        info!(
            "🔊 BUZZER: {}Hz at {:.1}% duty{}",
            self.frequency_hz,
            self.duty_cycle * 100.0,
            duration_ms.map_or_else(String::new, |duration| format!(" for {duration}ms"))
        );

        if !playing {
            return;
        }

        if let Some(audio_tx) = self.audio_tx.as_ref() {
            if audio_tx
                .send(AudioCommand::Beep {
                    frequency_hz,
                    duty_cycle: self.duty_cycle,
                    duration_ms,
                })
                .is_err()
            {
                self.audio_tx = None;
                self.playing.store(false, Ordering::Release);
                warn!("host buzzer audio worker stopped; using trace-only mode");
            }
        }
    }

    /// Stops any currently playing tone.
    pub fn stop(&mut self) {
        if let Some(audio_tx) = self.audio_tx.as_ref() {
            let _ = audio_tx.send(AudioCommand::Stop);
        }
        self.playing.store(false, Ordering::Release);
    }

    /// Reports whether a tone is active. Timed tones stop independently.
    pub fn is_playing(&self) -> bool {
        self.playing.load(Ordering::Acquire)
    }

    pub fn frequency_hz(&self) -> u32 {
        self.frequency_hz
    }

    pub fn duration_ms(&self) -> u64 {
        self.duration_ms
    }

    pub fn duty_cycle(&self) -> f32 {
        self.duty_cycle
    }
}

impl Default for VirtualBuzzer {
    fn default() -> Self {
        Self::new()
    }
}

pub fn register_buzzer(instance_id: &str) -> SharedVirtualBuzzer {
    let buzzer = Arc::new(Mutex::new(VirtualBuzzer::new()));
    lock(&BUZZERS).insert(instance_id.to_owned(), Arc::clone(&buzzer));
    buzzer
}

pub fn get_buzzer(instance_id: &str) -> Option<SharedVirtualBuzzer> {
    lock(&BUZZERS).get(instance_id).cloned()
}

pub fn remove_buzzer(instance_id: &str) -> Option<SharedVirtualBuzzer> {
    lock(&BUZZERS).remove(instance_id)
}

unsafe fn parse_instance_id(instance_id: *const c_char) -> Option<String> {
    if instance_id.is_null() {
        return None;
    }
    let id = unsafe { CStr::from_ptr(instance_id) }.to_str().ok()?;
    (!id.is_empty()).then(|| id.to_owned())
}

/// Starts a tone for a registered emulator instance.
///
/// # Safety
///
/// `instance_id` must point to a valid NUL-terminated string for this call.
#[no_mangle]
pub unsafe extern "C" fn meshemu_buzzer_beep(
    instance_id: *const c_char,
    frequency_hz: u32,
    duration_ms: u32,
) {
    let Some(id) = (unsafe { parse_instance_id(instance_id) }) else {
        return;
    };
    if !peripherals_powered(&id) {
        return;
    }
    if let Some(buzzer) = get_buzzer(&id) {
        lock(&buzzer).beep(frequency_hz, u64::from(duration_ms));
    }
}

/// Stops the tone for a registered emulator instance.
///
/// # Safety
///
/// `instance_id` must point to a valid NUL-terminated string for this call.
#[no_mangle]
pub unsafe extern "C" fn meshemu_buzzer_stop(instance_id: *const c_char) {
    let Some(id) = (unsafe { parse_instance_id(instance_id) }) else {
        return;
    };
    if let Some(buzzer) = get_buzzer(&id) {
        lock(&buzzer).stop();
    }
}

/// Reports whether a registered emulator instance is currently sounding.
///
/// # Safety
///
/// `instance_id` must point to a valid NUL-terminated string for this call.
#[no_mangle]
pub unsafe extern "C" fn meshemu_buzzer_is_playing(instance_id: *const c_char) -> bool {
    let Some(id) = (unsafe { parse_instance_id(instance_id) }) else {
        return false;
    };
    get_buzzer(&id).is_some_and(|buzzer| lock(&buzzer).is_playing())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn beep_stop_lifecycle() {
        let mut buzzer = VirtualBuzzer::new();
        buzzer.beep(440, 1_000);

        assert!(buzzer.is_playing());
        assert_eq!(buzzer.frequency_hz(), 440);
        assert_eq!(buzzer.duration_ms(), 1_000);

        buzzer.stop();
        assert!(!buzzer.is_playing());
    }

    #[test]
    fn tone_auto_stops_after_duration() {
        let mut buzzer = VirtualBuzzer::new();
        buzzer.beep(880, 10);
        std::thread::sleep(Duration::from_millis(30));

        assert!(!buzzer.is_playing());
    }

    #[test]
    fn pwm_frequency_and_volume_follow_period_and_duty() {
        let mut buzzer = VirtualBuzzer::new();
        buzzer.drive_pwm(2_000, 0.25);

        assert!(buzzer.is_playing());
        assert_eq!(buzzer.frequency_hz(), 2_000);
        assert_eq!(buzzer.duty_cycle(), 0.25);

        buzzer.drive_pwm(2_000, 1.0);
        assert!(!buzzer.is_playing());
    }

    #[test]
    fn ffi_controls_registered_instance_buzzer() {
        let id = std::ffi::CString::new("buzzer-test").unwrap();
        let buzzer = register_buzzer("buzzer-test");

        unsafe {
            meshemu_buzzer_beep(id.as_ptr(), 523, 1_000);
            assert!(meshemu_buzzer_is_playing(id.as_ptr()));
            meshemu_buzzer_stop(id.as_ptr());
            assert!(!meshemu_buzzer_is_playing(id.as_ptr()));
        }

        assert!(!lock(&buzzer).is_playing());
        remove_buzzer("buzzer-test");
    }
}
