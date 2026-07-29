use std::collections::HashMap;
use std::f32::consts::TAU;
use std::ffi::{c_char, CStr};
use std::sync::mpsc::{self, Sender};
use std::sync::{Arc, LazyLock, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use sdl2::audio::{AudioCallback, AudioSpecDesired};
use tracing::{info, warn};

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
    playing: bool,
    frequency_hz: u32,
    start_time: Instant,
    duration_ms: u64,
    audio_enabled: bool,
    audio_tx: Option<Sender<AudioCommand>>,
}

enum AudioCommand {
    Beep { frequency_hz: u32 },
    Stop,
}

#[derive(Default)]
struct ToneState {
    frequency_hz: u32,
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
            *sample = self.phase.sin() * 0.20;
            self.phase = (self.phase + phase_step) % TAU;
        }
    }
}

fn start_audio_worker() -> Option<Sender<AudioCommand>> {
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

            let Ok((_sdl, _audio, device)) = result else {
                let _ = ready_tx.send(false);
                return;
            };
            device.resume();
            if ready_tx.send(true).is_err() {
                return;
            }

            while let Ok(command) = command_rx.recv() {
                let mut state = lock(&state);
                match command {
                    AudioCommand::Beep { frequency_hz } => {
                        state.frequency_hz = frequency_hz;
                        state.playing = true;
                    }
                    AudioCommand::Stop => state.playing = false,
                }
            }
        })
        .ok()?;

    ready_rx
        .recv_timeout(Duration::from_millis(500))
        .ok()
        .filter(|ready| *ready)
        .map(|_| command_tx)
}

impl VirtualBuzzer {
    pub fn new() -> Self {
        let audio_tx = start_audio_worker();
        if audio_tx.is_none() {
            warn!("host audio unavailable; buzzer will be trace-only");
        }

        Self {
            playing: false,
            frequency_hz: 0,
            start_time: Instant::now(),
            duration_ms: 0,
            audio_enabled: audio_tx.is_some(),
            audio_tx,
        }
    }

    /// Starts a sine-wave tone, replacing any tone already in progress.
    pub fn beep(&mut self, frequency_hz: u32, duration_ms: u64) {
        self.stop();
        self.frequency_hz = frequency_hz;
        self.duration_ms = duration_ms;
        self.start_time = Instant::now();
        self.playing = frequency_hz > 0 && duration_ms > 0;

        info!(
            "🔊 BUZZER: {}Hz for {}ms",
            self.frequency_hz, self.duration_ms
        );

        if !self.playing || !self.audio_enabled {
            return;
        }

        let Some(audio_tx) = self.audio_tx.as_ref() else {
            return;
        };
        if audio_tx.send(AudioCommand::Beep { frequency_hz }).is_err() {
            self.audio_enabled = false;
            self.audio_tx = None;
            warn!("host buzzer audio worker stopped; using trace-only mode");
        }
    }

    /// Stops any currently playing tone.
    pub fn stop(&mut self) {
        if let Some(audio_tx) = self.audio_tx.as_ref() {
            let _ = audio_tx.send(AudioCommand::Stop);
        }
        self.playing = false;
    }

    /// Updates elapsed-time state and reports whether a tone is active.
    pub fn is_playing(&mut self) -> bool {
        if self.playing && self.start_time.elapsed() >= Duration::from_millis(self.duration_ms) {
            self.stop();
        }
        self.playing
    }

    pub fn frequency_hz(&self) -> u32 {
        self.frequency_hz
    }

    pub fn duration_ms(&self) -> u64 {
        self.duration_ms
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
        buzzer.beep(880, 1);
        std::thread::sleep(Duration::from_millis(5));

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
