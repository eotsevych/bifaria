use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::Sample;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{channel, Sender};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};
use tauri::{AppHandle, Emitter, Manager};

/// Input and output device names from one pass over the devices. On Linux each pass opens every
/// ALSA device (hw:, plughw:, dmix, pulse…) and probes its formats, so callers that need both lists
/// use this instead of the two functions below.
pub fn list_devices() -> (Vec<String>, Vec<String>) {
    let (mut inputs, mut outputs) = (Vec::new(), Vec::new());
    // cpal's input_devices() and output_devices() are filters over devices(), so this sees them all.
    if let Ok(devices) = cpal::default_host().devices() {
        for d in devices {
            let Ok(name) = d.name() else { continue };
            let name = name.trim().to_string();
            if name.is_empty() {
                continue;
            }
            let has_input = d.default_input_config().is_ok()
                || d.supported_input_configs().map(|mut c| c.next().is_some()).unwrap_or(false);
            let has_output = d.default_output_config().is_ok()
                || d.supported_output_configs().map(|mut c| c.next().is_some()).unwrap_or(false);
            if has_input && !inputs.contains(&name) {
                inputs.push(name.clone());
            }
            if has_output && !outputs.contains(&name) {
                outputs.push(name);
            }
        }
    }
    (inputs, outputs)
}

pub fn list_input_devices() -> Vec<String> {
    list_devices().0
}

pub fn list_output_devices() -> Vec<String> {
    list_devices().1
}

pub fn resolve_input_device(device_name: Option<&str>) -> Option<cpal::Device> {
    resolve_input_device_reporting_fallback(device_name).map(|(device, _)| device)
}

/// Like `resolve_input_device`, but also returns the requested device name when it wasn't found and
/// the system default was used instead (e.g. the chosen USB mic was unplugged).
fn resolve_input_device_reporting_fallback(device_name: Option<&str>) -> Option<(cpal::Device, Option<String>)> {
    let host = cpal::default_host();
    let mut missing = None;
    if let Some(clean) = chosen_input_name(device_name) {
        if let Some(found) = find_input_device(&host, clean) {
            return Some((found, None));
        }
        missing = Some(clean.to_string());
    }
    host.default_input_device().map(|d| (d, missing))
}

/// The microphone the user picked by name, or None when the setting means the system default.
fn chosen_input_name(device_name: Option<&str>) -> Option<&str> {
    let clean = device_name?.trim();
    if clean == "Default" || clean == "System default" || clean == "System Default" || clean.is_empty() {
        return None;
    }
    Some(clean)
}

fn find_input_device(host: &cpal::Host, name: &str) -> Option<cpal::Device> {
    if let Ok(devs) = host.devices() {
        for d in devs {
            if d.name().map(|n| n.trim() == name).unwrap_or(false) {
                return Some(d);
            }
        }
    }
    host.input_devices().ok()?.find(|d| d.name().map(|n| n.trim() == name).unwrap_or(false))
}

/// The chosen microphone's name when one is picked (not the system default) but isn't connected.
pub fn missing_input_device(device_name: Option<&str>) -> Option<String> {
    let clean = chosen_input_name(device_name)?;
    match find_input_device(&cpal::default_host(), clean) {
        Some(_) => None,
        None => Some(clean.to_string()),
    }
}

/// Seconds of recording after which the microphone must have delivered real audio.
pub const MIC_CHECK_AFTER: Duration = Duration::from_millis(2500);
/// Longest Stop waits for the audio captured before the stop to arrive from the callback. It
/// usually arrives within one buffer (~10-20 ms); Bluetooth mics deliver in bigger batches.
const STOP_FLUSH_MAX: Duration = Duration::from_millis(120);

/// When the user asked to stop, and whether the callback has delivered audio from past that moment
/// (so everything before it is in the buffer).
#[derive(Default)]
struct StopMark {
    at: Option<Instant>,
    reached: bool,
}
/// Once audio has flowed, this long without new samples means the device went away.
const MIC_STALL_AFTER: Duration = Duration::from_millis(1500);
/// Once real audio has flowed, this long of pure digital silence means the microphone stopped
/// delivering sound (natural pauses always keep a noise floor).
const MIC_WENT_SILENT_AFTER: Duration = Duration::from_secs(3);
/// Exact zeros are a stronger signal than near-silence: a working microphone never delivers a full
/// second of them. This is what macOS feeds when a Bluetooth headset drops its call-audio (SCO) link.
const MIC_EXACT_ZERO_AFTER: Duration = Duration::from_secs(1);
/// How many times one recording may reopen a microphone that went dead.
const MAX_STREAM_RESTARTS: u32 = 2;
/// A live microphone always has a noise floor above this; below it the OS is feeding zeros.
const MIC_SILENCE_PEAK: f32 = 1e-4;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MicProblem {
    /// The stream opened but no samples arrived.
    NoAudio,
    /// Samples arrived but they are pure digital silence (muted, or blocked by privacy settings).
    Silent,
    /// Audio flowed, then stopped (typically unplugged mid-recording).
    Stalled,
    /// Real audio flowed, then only digital silence arrived (the device stopped delivering sound).
    WentSilent,
    /// The audio system reported the device as gone.
    Disconnected,
}

impl MicProblem {
    /// Problems found before any usable audio arrived. Those cancel the recording; later ones only
    /// warn, so speech captured before the problem is still transcribed.
    pub fn is_startup(self) -> bool {
        matches!(self, MicProblem::NoAudio | MicProblem::Silent)
    }
}

/// What the health check noticed in the latest audio.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MicEvent {
    Problem(MicProblem),
    /// Audio is flowing again after a mid-recording problem.
    Recovered,
}

/// A microphone failure detected while recording, reported to the controller.
#[derive(Clone, Debug)]
pub struct MicReport {
    pub problem: MicProblem,
    /// Device actually recorded from.
    pub device: String,
    /// The user's chosen device, when it was missing and `device` is the fallback default.
    pub missing_device: Option<String>,
    /// The stream is being reopened to recover from this problem.
    pub reconnecting: bool,
}

/// Decides from the incoming audio whether the microphone works. Fed by the level monitor.
#[derive(Debug)]
struct MicHealth {
    started: Instant,
    samples: usize,
    peak: f32,
    last_audio: Option<Instant>,
    /// Start of the current stretch of digital silence.
    dead_since: Option<Instant>,
    /// Start of the current stretch of exact zeros.
    exact_zero_since: Option<Instant>,
    /// The mid-recording problem currently reported, until audio recovers.
    dropout: Option<MicProblem>,
    /// After a reopen: when to report that the reopened stream is still dead.
    resume_deadline: Option<Instant>,
    checked: bool,
}

impl MicHealth {
    fn new(started: Instant) -> Self {
        Self {
            started,
            samples: 0,
            peak: 0.0,
            last_audio: None,
            dead_since: None,
            exact_zero_since: None,
            dropout: None,
            resume_deadline: None,
            checked: false,
        }
    }

    /// For a stream reopened after `problem`: no startup check (speech was already captured), and
    /// the first real audio reports recovery.
    fn resumed(started: Instant, problem: MicProblem) -> Self {
        Self {
            checked: true,
            dropout: Some(problem),
            resume_deadline: Some(started + MIC_WENT_SILENT_AFTER),
            ..Self::new(started)
        }
    }

    fn observe(&mut self, new_samples: &[f32], now: Instant) -> Option<MicEvent> {
        let has_new = !new_samples.is_empty();
        let all_dead = new_samples.iter().all(|s| !s.is_finite() || s.abs() < MIC_SILENCE_PEAK);
        let all_exact_zero = new_samples.iter().all(|s| *s == 0.0);
        if has_new {
            self.samples += new_samples.len();
            self.peak = new_samples.iter().filter(|s| s.is_finite()).fold(self.peak, |p, s| p.max(s.abs()));
            self.last_audio = Some(now);
            self.dead_since = if all_dead { Some(self.dead_since.unwrap_or(now)) } else { None };
            self.exact_zero_since = if all_exact_zero { Some(self.exact_zero_since.unwrap_or(now)) } else { None };
        }

        if !self.checked {
            if now.duration_since(self.started) < MIC_CHECK_AFTER {
                return None;
            }
            self.checked = true;
            if self.samples == 0 {
                return Some(MicEvent::Problem(MicProblem::NoAudio));
            }
            if self.peak < MIC_SILENCE_PEAK {
                return Some(MicEvent::Problem(MicProblem::Silent));
            }
            return None;
        }

        if let Some(problem) = self.dropout {
            if has_new && !all_dead {
                self.dropout = None;
                self.resume_deadline = None;
                return Some(MicEvent::Recovered);
            }
            // A reopened stream that still delivers nothing: report it again (another reopen, or a warning).
            if self.resume_deadline.is_some_and(|at| now >= at) {
                self.resume_deadline = None;
                return Some(MicEvent::Problem(problem));
            }
            return None;
        }
        let stalled = self.last_audio.is_some_and(|last| now.duration_since(last) >= MIC_STALL_AFTER);
        let went_silent = self.dead_since.is_some_and(|since| now.duration_since(since) >= MIC_WENT_SILENT_AFTER)
            || self.exact_zero_since.is_some_and(|since| now.duration_since(since) >= MIC_EXACT_ZERO_AFTER);
        let problem = if stalled {
            MicProblem::Stalled
        } else if went_silent {
            MicProblem::WentSilent
        } else {
            return None;
        };
        self.dropout = Some(problem);
        Some(MicEvent::Problem(problem))
    }
}

/// Seconds a fresh recording may go without speech before the user is warned; 0 = off.
static NO_SPEECH_ALERT_SECS: AtomicU64 = AtomicU64::new(0);

/// Sets the no-speech alert for the recordings started from now on (0 turns it off).
pub fn set_no_speech_alert(secs: u64) {
    NO_SPEECH_ALERT_SECS.store(secs, Ordering::Relaxed);
}

/// How often the speech watch runs the VAD, and how much already checked audio each run re-reads
/// so a word cut by the previous run's end is still recognised.
const SPEECH_CHECK_EVERY: Duration = Duration::from_secs(1);
const SPEECH_CHECK_OVERLAP: usize = 8000; // 0.5 s at 16 kHz

/// What the speech watch noticed.
#[derive(Debug, PartialEq, Eq)]
enum SpeechEvent {
    /// No speech since the recording started, for the alert time.
    NotHeard,
    /// Speech arrived after the warning was shown.
    HeardAfterWarning,
}

/// Watches a fresh recording for speech, to warn when none is heard (usually a muted mic that
/// still sends a faint noise floor, which the mic health check can't tell from a quiet room).
struct SpeechWatch {
    after: Duration,
    started: Instant,
    next_check: Instant,
    /// Buffer position of the stream's first sample, and up to where audio has been checked.
    start_len: usize,
    checked_to: usize,
    warned: bool,
}

impl SpeechWatch {
    fn new(started: Instant, after: Duration, start_len: usize) -> Self {
        Self { after, started, next_check: started + SPEECH_CHECK_EVERY, start_len, checked_to: start_len, warned: false }
    }

    /// Checks the audio since the last run, once per `SPEECH_CHECK_EVERY`. Returns what changed,
    /// and whether watching is over (speech was heard).
    fn poll(&mut self, buffer: &Mutex<Vec<f32>>, now: Instant, has_speech: impl Fn(&[f32]) -> bool) -> (Option<SpeechEvent>, bool) {
        if now < self.next_check {
            return (None, false);
        }
        self.next_check = now + SPEECH_CHECK_EVERY;
        let chunk = match buffer.lock() {
            Ok(b) => {
                let from = self.checked_to.saturating_sub(SPEECH_CHECK_OVERLAP).max(self.start_len).min(b.len());
                self.checked_to = b.len();
                b[from..].to_vec()
            }
            Err(_) => return (None, false),
        };
        if has_speech(&chunk) {
            return (self.warned.then_some(SpeechEvent::HeardAfterWarning), true);
        }
        if !self.warned && now.duration_since(self.started) >= self.after {
            self.warned = true;
            return (Some(SpeechEvent::NotHeard), false);
        }
        (None, false)
    }
}

/// Sends a device-gone report to the controller once per recording.
fn report_mic_problem(app_handle: &AppHandle, reported: &AtomicBool, report: MicReport) {
    if reported.swap(true, Ordering::SeqCst) {
        return;
    }
    log::warn!("Microphone health check failed: {:?}", report);
    crate::app_controller::report_mic_problem(app_handle, report);
}

static MIC_TEST_STOP: Mutex<Option<Sender<()>>> = Mutex::new(None);

pub fn stop_mic_test() {
    if let Ok(mut lock) = MIC_TEST_STOP.lock() {
        if let Some(tx) = lock.take() {
            let _ = tx.send(());
        }
    }
}

pub fn start_mic_test(app_handle: AppHandle, device_name: Option<String>) -> Result<(), String> {
    stop_mic_test();

    let (stop_tx, stop_rx) = channel::<()>();
    let (init_tx, init_rx) = channel::<Result<(), String>>();

    if let Ok(mut lock) = MIC_TEST_STOP.lock() {
        *lock = Some(stop_tx);
    }

    thread::spawn(move || {
        let device = match resolve_input_device(device_name.as_deref()) {
            Some(d) => d,
            None => {
                let _ = init_tx.send(Err("No audio input device found".to_string()));
                return;
            }
        };

        let supported_config = match device.default_input_config() {
            Ok(c) => c,
            Err(e) => match device.supported_input_configs().ok().and_then(|mut iter| iter.next()) {
                Some(conf) => conf.with_max_sample_rate(),
                None => {
                    let _ = init_tx.send(Err(format!("Failed to get default input config: {}", e)));
                    return;
                }
            },
        };

        let sample_format = supported_config.sample_format();
        let channels = supported_config.channels() as usize;
        let latest_rms = Arc::new(Mutex::new(0.0f32));
        let rms_write = Arc::clone(&latest_rms);
        let running = Arc::new(AtomicBool::new(true));
        let running_cb = Arc::clone(&running);

        let err_fn = |err| {
            log::warn!("Mic test stream error: {}", err);
        };

        let stream_result = match sample_format {
            cpal::SampleFormat::F32 => {
                device.build_input_stream(
                    &supported_config.into(),
                    move |data: &[f32], _: &_| {
                        if !running_cb.load(Ordering::Relaxed) || data.is_empty() {
                            return;
                        }
                        let mut sum_sq = 0.0f32;
                        let step = channels.max(1);
                        let mut count = 0;
                        for i in (0..data.len()).step_by(step) {
                            let s = data[i];
                            sum_sq += s * s;
                            count += 1;
                        }
                        let rms = if count > 0 { (sum_sq / count as f32).sqrt() } else { 0.0 };
                        if let Ok(mut r) = rms_write.lock() {
                            *r = rms;
                        }
                    },
                    err_fn,
                    None,
                )
            }
            cpal::SampleFormat::I16 => {
                device.build_input_stream(
                    &supported_config.into(),
                    move |data: &[i16], _: &_| {
                        if !running_cb.load(Ordering::Relaxed) || data.is_empty() {
                            return;
                        }
                        let mut sum_sq = 0.0f32;
                        let step = channels.max(1);
                        let mut count = 0;
                        for i in (0..data.len()).step_by(step) {
                            let s = data[i].to_float_sample();
                            sum_sq += s * s;
                            count += 1;
                        }
                        let rms = if count > 0 { (sum_sq / count as f32).sqrt() } else { 0.0 };
                        if let Ok(mut r) = rms_write.lock() {
                            *r = rms;
                        }
                    },
                    err_fn,
                    None,
                )
            }
            fmt => {
                let _ = init_tx.send(Err(format!("Unsupported format: {:?}", fmt)));
                return;
            }
        };

        let stream = match stream_result {
            Ok(s) => s,
            Err(e) => {
                let _ = init_tx.send(Err(format!("Failed to build input stream: {}", e)));
                return;
            }
        };

        if let Err(e) = stream.play() {
            let _ = init_tx.send(Err(format!("Failed to play input stream: {}", e)));
            return;
        }

        let _ = init_tx.send(Ok(()));

        let mut peak_hold = 0.0f32;
        loop {
            if stop_rx.recv_timeout(Duration::from_millis(40)).is_ok() {
                break;
            }

            let rms = latest_rms.lock().map(|r| *r).unwrap_or(0.0);
            let level = (rms * 14.0).clamp(0.0, 1.0);
            peak_hold = (peak_hold * 0.93).max(level);

            let _ = app_handle.emit("mic-test-level", serde_json::json!({
                "level": level,
                "peak": peak_hold
            }));
        }

        running.store(false, Ordering::SeqCst);
        let _ = stream.pause();
    });

    init_rx
        .recv()
        .map_err(|e| format!("Mic test thread did not respond: {}", e))?
}

enum AudioCmd {
    Start {
        app_handle: AppHandle,
        device_name: Option<String>,
        /// Noise reduction is on: the audio is denoised while it's recorded.
        denoise: bool,
        reply: Sender<Result<StreamInfo, String>>,
    },
    Stop {
        /// Noise reduction mix, None when off.
        denoise: Option<f32>,
        reply: Sender<CapturedAudio>,
    },
    Cancel,
    /// Sent by a stream's own monitor: reopen it because it went dead. Ignored unless `generation`
    /// is still the current stream (a stop or a newer stream wins).
    Reopen { generation: u64, problem: MicProblem },
}

/// One finished recording.
pub struct CapturedAudio {
    /// Leveled 16 kHz mono: what VAD and the speech models get.
    pub speech: Vec<f32>,
    /// Unleveled mono at the microphone's own rate, kept for playback quality.
    native: Vec<f32>,
    native_rate: u32,
    plan: crate::leveler::LevelPlan,
    /// What the microphone delivered, measured before leveling and noise reduction.
    pub stats: CaptureStats,
}

/// The input stream a recording opened.
#[derive(Clone, Debug)]
pub struct StreamInfo {
    pub device: String,
    pub sample_rate: u32,
    pub channels: u16,
    /// The user's chosen device, when it was missing and `device` is the fallback default.
    pub missing_device: Option<String>,
}

/// Facts about the raw 16 kHz capture, for the stop log: how much audio arrived and whether parts of
/// it were dead (digital silence, invalid values), which shows a microphone that quietly stopped.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct CaptureStats {
    pub captured_sec: f32,
    /// Total digital silence, in seconds (a live microphone never produces it).
    pub zero_sec: f32,
    /// Longest stretch of digital silence and where it started, in seconds.
    pub longest_zero_sec: f32,
    pub longest_zero_at_sec: f32,
    /// NaN or infinite samples.
    pub invalid_samples: usize,
}

impl CaptureStats {
    /// Measures 16 kHz mono samples. Silence is counted in 10 ms blocks.
    pub fn measure(samples: &[f32]) -> Self {
        const BLOCK: usize = 160;
        let rate = 16000.0;
        let mut stats = Self { captured_sec: samples.len() as f32 / rate, ..Self::default() };
        stats.invalid_samples = samples.iter().filter(|s| !s.is_finite()).count();
        let (mut run_start, mut run_len) = (0usize, 0usize);
        for (i, block) in samples.chunks(BLOCK).enumerate() {
            let dead = block.iter().all(|s| s.is_finite() && s.abs() < MIC_SILENCE_PEAK);
            if dead {
                if run_len == 0 {
                    run_start = i * BLOCK;
                }
                run_len += block.len();
                stats.zero_sec += block.len() as f32 / rate;
                if run_len as f32 / rate > stats.longest_zero_sec {
                    stats.longest_zero_sec = run_len as f32 / rate;
                    stats.longest_zero_at_sec = run_start as f32 / rate;
                }
            } else {
                run_len = 0;
            }
        }
        stats
    }
}

impl CapturedAudio {
    /// `live` is the 48 kHz `native` audio's denoising done while recording, if it was.
    fn new(speech_raw: Vec<f32>, native: Vec<f32>, native_rate: u32, denoise: Option<f32>, live: Option<crate::denoise::LiveDenoise>) -> Self {
        let (speech_raw, native) = match denoise {
            // Denoise the full-bandwidth copy, then derive the 16 kHz speech from it.
            Some(wet) if !native.is_empty() => {
                let clean = match live {
                    Some(live) if native_rate == 48000 => live.finish(&native, wet),
                    _ => crate::denoise::denoise(&native, native_rate, wet),
                };
                let speech = crate::denoise::resample(&clean, native_rate, 16000).unwrap_or(speech_raw);
                (speech, clean)
            }
            Some(wet) => (crate::denoise::denoise(&speech_raw, 16000, wet), native),
            None => (speech_raw, native),
        };
        let (speech, plan) = crate::leveler::level(&speech_raw, 16000);
        Self { speech, native, native_rate, plan, stats: CaptureStats::default() }
    }

    pub fn empty() -> Self {
        Self { speech: Vec::new(), native: Vec::new(), native_rate: 16000, plan: Default::default(), stats: CaptureStats::default() }
    }

    pub fn level_plan(&self) -> crate::leveler::LevelPlan {
        self.plan
    }

    /// Leveled mono at the microphone's own rate, and that rate, for history playback.
    pub fn playback(&self) -> (Vec<f32>, u32) {
        if self.native.is_empty() {
            return (self.speech.clone(), 16000);
        }
        (crate::leveler::apply(&self.native, self.native_rate, self.plan), self.native_rate)
    }
}

pub struct AudioRecorder {
    cmd_tx: Sender<AudioCmd>,
    is_recording: Arc<AtomicBool>,
    /// Moment the user asked to stop; audio captured after it is dropped.
    stop_at: Arc<Mutex<StopMark>>,
    /// When the start chime has died away, if it played; its tones are removed from the audio
    /// captured until then.
    chime_until: Arc<Mutex<Option<Instant>>>,
    /// This recording's audio at 16 kHz, filled by the stream callback. Cleared on every start.
    buffer: Arc<Mutex<Vec<f32>>>,
}

impl AudioRecorder {
    pub fn new() -> Self {
        let (cmd_tx, cmd_rx) = channel::<AudioCmd>();
        let is_recording = Arc::new(AtomicBool::new(false));
        let is_rec_thread = Arc::clone(&is_recording);
        let stop_at = Arc::new(Mutex::new(StopMark::default()));
        let stop_at_thread = Arc::clone(&stop_at);
        let chime_until = Arc::new(Mutex::new(None::<Instant>));
        let chime_until_thread = Arc::clone(&chime_until);
        let reopen_tx = cmd_tx.clone();
        let buffer = Arc::new(Mutex::new(Vec::<f32>::new()));
        let buffer_thread = Arc::clone(&buffer);

        // Dedicated audio recording thread so cpal::Stream stays on one thread
        thread::spawn(move || {
            let mut active_stream: Option<OpenStream> = None;
            // The last recording's stream, paused, for the next one (see keep_warm).
            let mut warm: Option<OpenStream> = None;
            let buffer = buffer_thread;
            let native_buffer = Arc::new(Mutex::new(Vec::<f32>::new()));
            let mut native_rate = 16000;
            // A reopened stream at another rate makes the full-bandwidth copy inconsistent; playback
            // then falls back to the 16 kHz speech audio.
            let mut native_valid = true;
            // Identifies the current stream; bumped by every start, reopen, stop and cancel so a
            // stale monitor or Reopen request is ignored.
            let generation = Arc::new(AtomicU64::new(0));
            let mut current: Option<(AppHandle, Option<String>)> = None;
            // When the stream delivering the buffer's first sample started (the start chime's
            // window in the buffer is measured from here).
            let mut capture_started: Option<Instant> = None;
            let mut restarts = 0u32;
            // Denoising of this recording's 48 kHz audio done while recording (see feed_live_denoise).
            let mut live: Option<crate::denoise::LiveDenoise> = None;
            let mut live_wanted = false;
            // The start chime was already notched out of the 48 kHz audio, before it was denoised.
            let mut native_chime_done = false;

            loop {
                let cmd = match cmd_rx.recv_timeout(LIVE_DENOISE_TICK) {
                    Ok(cmd) => cmd,
                    Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                        if live_wanted && native_valid && native_rate == 48000 && is_rec_thread.load(Ordering::SeqCst) {
                            feed_live_denoise(&mut live, &mut native_chime_done, &native_buffer, &chime_until_thread, capture_started);
                        }
                        continue;
                    }
                    Err(_) => break,
                };
                match cmd {
                    AudioCmd::Start { app_handle, device_name, denoise, reply } => {
                        live = None;
                        live_wanted = denoise;
                        native_chime_done = false;
                        stop_mic_test();
                        // A stream still open here was never stopped; it would keep appending to
                        // the shared buffers alongside the new one and double the audio.
                        if let Some(stale) = active_stream.take() {
                            let _ = stale.stream.pause();
                            log::warn!("Closed a microphone stream left open by an earlier recording");
                        }
                        if let Ok(mut b) = buffer.lock() {
                            b.clear();
                        }
                        if let Ok(mut b) = native_buffer.lock() {
                            b.clear();
                        }
                        if let Ok(mut s) = stop_at_thread.lock() {
                            *s = StopMark::default();
                        }
                        if let Ok(mut c) = chime_until_thread.lock() {
                            *c = None;
                        }

                        // Mark recording active before starting stream so initial audio buffers are captured immediately
                        is_rec_thread.store(true, Ordering::SeqCst);
                        let open_started = Instant::now();
                        restarts = 0;
                        native_valid = true;
                        current = Some((app_handle.clone(), device_name.clone()));
                        let control = StreamControl {
                            reopen_tx: reopen_tx.clone(),
                            generation: Arc::clone(&generation),
                            id: generation.fetch_add(1, Ordering::SeqCst) + 1,
                            resumed_from: None,
                            can_reopen: true,
                        };

                        let resumed = warm.take().and_then(|w| resume_stream(w, &device_name, control.id));
                        let was_warm = resumed.is_some();
                        let res = match resumed {
                            Some(open) => {
                                let info = open.info.clone();
                                spawn_monitor(Arc::clone(&is_rec_thread), Arc::clone(&buffer), app_handle, info.clone(), control);
                                active_stream = Some(open);
                                Ok(info)
                            }
                            None => start_stream_inner(
                                &mut active_stream,
                                Arc::clone(&buffer),
                                Arc::clone(&native_buffer),
                                Arc::clone(&is_rec_thread),
                                Arc::clone(&stop_at_thread),
                                app_handle,
                                device_name,
                                control,
                            ),
                        };

                        match &res {
                            Ok(info) => {
                                native_rate = info.sample_rate;
                                capture_started = Some(Instant::now());
                                log::info!(
                                    "Microphone stream \"{}\" {} in {} ms at {} Hz, {} ch",
                                    info.device,
                                    if was_warm { "resumed" } else { "opened" },
                                    open_started.elapsed().as_millis(),
                                    info.sample_rate,
                                    info.channels
                                );
                            }
                            Err(_) => is_rec_thread.store(false, Ordering::SeqCst),
                        }
                        let _ = reply.send(res);
                    }
                    AudioCmd::Stop { denoise, reply } => {
                        // Wait for the audio captured before the stop to arrive from the callback. Frames
                        // captured after the stop are discarded there (see frames_before_stop).
                        let flush_started = Instant::now();
                        while flush_started.elapsed() < STOP_FLUSH_MAX
                            && !stop_at_thread.lock().map(|s| s.reached).unwrap_or(true)
                        {
                            thread::sleep(Duration::from_millis(5));
                        }
                        is_rec_thread.store(false, Ordering::SeqCst);
                        generation.fetch_add(1, Ordering::SeqCst);
                        current = None;
                        // A stream that had to be reopened mid-recording isn't trusted for the next one.
                        if let Some(open) = active_stream.take() {
                            warm = if restarts == 0 {
                                keep_warm(open)
                            } else {
                                let _ = open.stream.pause();
                                None
                            };
                        }
                        let mut speech = buffer.lock().map(|mut b| std::mem::take(&mut *b)).unwrap_or_default();
                        let stats = CaptureStats::measure(&speech);
                        // At 16 kHz the speech buffer already is the native audio.
                        let native = native_buffer.lock().map(|mut b| std::mem::take(&mut *b)).unwrap_or_default();
                        let mut native = if native_rate == 16000 || !native_valid { Vec::new() } else { native };
                        let chime_until = chime_until_thread.lock().ok().and_then(|mut c| c.take());
                        if let (Some(until), Some(started)) = (chime_until, capture_started.take()) {
                            let window = until.saturating_duration_since(started).as_secs_f64();
                            remove_start_chime(&mut speech, 16000, (window * 16000.0) as usize);
                            if !native_chime_done {
                                remove_start_chime(&mut native, native_rate, (window * native_rate as f64) as usize);
                            }
                        }
                        // Usable only when it saw exactly this audio: same stream, chime already out.
                        let live = live.take().filter(|l| {
                            native_valid && native_rate == 48000 && l.fed() <= native.len() && (native_chime_done || chime_until.is_none())
                        });
                        let while_recording = live.is_some();
                        let processing_started = Instant::now();
                        let mut captured = CapturedAudio::new(speech, native, native_rate, denoise, live);
                        captured.stats = stats;
                        let plan = captured.level_plan();
                        log::info!(
                            "Noise reduction {:?}{}, voice leveling gain {:+.1} dB, make-up {:+.1} dB, in {} ms",
                            denoise,
                            if while_recording { " (mostly done while recording)" } else { "" },
                            plan.gain_db,
                            plan.makeup_db,
                            processing_started.elapsed().as_millis()
                        );
                        let _ = reply.send(captured);
                    }
                    AudioCmd::Cancel => {
                        live = None;
                        is_rec_thread.store(false, Ordering::SeqCst);
                        generation.fetch_add(1, Ordering::SeqCst);
                        current = None;
                        // Cancelled recordings include microphone failures: the next one opens afresh.
                        warm = None;
                        if let Some(open) = active_stream.take() {
                            let _ = open.stream.pause();
                        }
                        if let Ok(mut b) = buffer.lock() {
                            b.clear();
                        }
                        if let Ok(mut b) = native_buffer.lock() {
                            b.clear();
                        }
                    }
                    AudioCmd::Reopen { generation: requested, problem } => {
                        let still_current = is_rec_thread.load(Ordering::SeqCst) && requested == generation.load(Ordering::SeqCst);
                        let Some((app_handle, device_name)) = current.clone().filter(|_| still_current) else {
                            continue;
                        };
                        // Closing and reopening the input makes macOS bring back e.g. a Bluetooth
                        // headset's dropped call-audio link. Audio keeps appending to the same buffers.
                        if let Some(open) = active_stream.take() {
                            let _ = open.stream.pause();
                        }
                        restarts += 1;
                        let reopen_started = Instant::now();
                        let control = StreamControl {
                            reopen_tx: reopen_tx.clone(),
                            generation: Arc::clone(&generation),
                            id: generation.fetch_add(1, Ordering::SeqCst) + 1,
                            resumed_from: Some(problem),
                            can_reopen: restarts < MAX_STREAM_RESTARTS,
                        };
                        let captured_sec = buffer.lock().map(|b| b.len() as f32 / 16000.0).unwrap_or_default();
                        let res = start_stream_inner(
                            &mut active_stream,
                            Arc::clone(&buffer),
                            Arc::clone(&native_buffer),
                            Arc::clone(&is_rec_thread),
                            Arc::clone(&stop_at_thread),
                            app_handle.clone(),
                            device_name,
                            control,
                        );
                        let data_dir = crate::settings::get_data_dir();
                        match res {
                            Ok(info) => {
                                if info.sample_rate != native_rate {
                                    native_valid = false;
                                }
                                crate::pipeline_logger::log_stage_event(
                                    &data_dir,
                                    "REC_RESTART",
                                    &format!(
                                        "Reopened the microphone after {:?} at {:.1} s of audio (attempt {}/{}): \"{}\" at {} Hz in {} ms",
                                        problem,
                                        captured_sec,
                                        restarts,
                                        MAX_STREAM_RESTARTS,
                                        info.device,
                                        info.sample_rate,
                                        reopen_started.elapsed().as_millis()
                                    ),
                                );
                            }
                            Err(e) => {
                                crate::pipeline_logger::log_stage_event(
                                    &data_dir,
                                    "REC_RESTART_FAILED",
                                    &format!("Could not reopen the microphone after {:?} (attempt {}): {}", problem, restarts, e),
                                );
                                crate::app_controller::report_mic_problem(
                                    &app_handle,
                                    MicReport {
                                        problem: MicProblem::Disconnected,
                                        device: "the microphone".to_string(),
                                        missing_device: None,
                                        reconnecting: false,
                                    },
                                );
                            }
                        }
                    }
                }
            }
        });

        Self {
            cmd_tx,
            is_recording,
            stop_at,
            chime_until,
            buffer,
        }
    }

    /// Waits for the microphone's first audio of the recording just started, up to `max`, and says
    /// how long it took. None on timeout or when the recording ended first.
    pub fn wait_for_first_audio(&self, max: Duration) -> Option<Duration> {
        let started = Instant::now();
        while started.elapsed() < max && self.is_recording() {
            if self.buffer.lock().map(|b| !b.is_empty()).unwrap_or(false) {
                return Some(started.elapsed());
            }
            thread::sleep(Duration::from_millis(5));
        }
        None
    }

    /// The start chime is playing now: its tones are removed from the audio captured until it has
    /// died away (open speakers feed it back into the mic).
    pub fn mark_start_chime(&self, chime: Duration) {
        if let Ok(mut c) = self.chime_until.lock() {
            *c = Some(Instant::now() + chime + START_CHIME_TAIL);
        }
    }

    /// The current recording's raw 16 kHz audio from sample `from` on, as captured so far; empty
    /// once it has stopped.
    pub fn speech_since(&self, from: usize) -> Vec<f32> {
        self.buffer.lock().map(|b| b.get(from..).map(<[f32]>::to_vec).unwrap_or_default()).unwrap_or_default()
    }

    /// When the start chime died away, if it played in this recording.
    pub fn chime_until(&self) -> Option<Instant> {
        self.chime_until.lock().ok().and_then(|c| *c)
    }

    pub fn is_recording(&self) -> bool {
        self.is_recording.load(Ordering::SeqCst)
    }

    /// `denoise`: noise reduction is on, so the audio is denoised while it's recorded.
    pub fn start_recording(&self, app_handle: AppHandle, device_name: Option<String>, denoise: bool) -> Result<StreamInfo, String> {
        let (reply_tx, reply_rx) = channel();
        self.cmd_tx
            .send(AudioCmd::Start {
                app_handle,
                device_name,
                denoise,
                reply: reply_tx,
            })
            .map_err(|e| format!("Failed to send start command to audio thread: {}", e))?;

        reply_rx
            .recv()
            .map_err(|e| format!("Audio thread did not respond: {}", e))?
    }

    /// `denoise` is the noise reduction mix (see `denoise::strength_wet`), None when off.
    pub fn stop_recording(&self, denoise: Option<f32>) -> CapturedAudio {
        if let Ok(mut s) = self.stop_at.lock() {
            *s = StopMark { at: Some(Instant::now()), reached: false };
        }
        let (reply_tx, reply_rx) = channel();
        if self.cmd_tx.send(AudioCmd::Stop { denoise, reply: reply_tx }).is_ok() {
            reply_rx.recv().unwrap_or_else(|_| CapturedAudio::empty())
        } else {
            CapturedAudio::empty()
        }
    }

    pub fn cancel_recording(&self) {
        let _ = self.cmd_tx.send(AudioCmd::Cancel);
    }
}

/// An input stream and what's needed to play it again for the next recording.
struct OpenStream {
    stream: cpal::Stream,
    info: StreamInfo,
    /// The microphone the recording asked for (None: the system default).
    requested: Option<String>,
    resampler: Arc<Mutex<ResamplerState>>,
    /// Generation of the recording this stream serves, which its error callback reports for.
    id: Arc<AtomicU64>,
    /// When it was paused, and `DEVICE_CHANGES` then.
    paused: Option<(Instant, u64)>,
}

/// A paused stream is played again only this long after the last recording.
const WARM_STREAM_FOR: Duration = Duration::from_secs(30 * 60);
/// Bumped whenever the audio devices change, so a paused stream on a changed setup isn't reused.
static DEVICE_CHANGES: AtomicU64 = AtomicU64::new(0);

/// Pauses a stream that finished a recording, to be played again by the next one: that skips
/// opening the microphone (about 150 ms). A stream that can't pause is closed instead.
fn keep_warm(mut open: OpenStream) -> Option<OpenStream> {
    if let Err(e) = open.stream.pause() {
        log::warn!("Couldn't pause the microphone stream, closing it: {}", e);
        return None;
    }
    open.paused = Some((Instant::now(), DEVICE_CHANGES.load(Ordering::SeqCst)));
    Some(open)
}

/// The paused stream playing again for recording `id`, when it's still what opening the
/// microphone now would give: same choice, device and format, no device changes, not too old.
fn resume_stream(open: OpenStream, device_name: &Option<String>, id: u64) -> Option<OpenStream> {
    let (paused_at, changes) = open.paused?;
    if &open.requested != device_name || paused_at.elapsed() > WARM_STREAM_FOR || changes != DEVICE_CHANGES.load(Ordering::SeqCst) {
        return None;
    }
    let (device, missing) = resolve_input_device_reporting_fallback(device_name.as_deref())?;
    let config = device.default_input_config().ok()?;
    let same = device.name().ok().as_deref() == Some(open.info.device.as_str())
        && missing == open.info.missing_device
        && config.sample_rate().0 == open.info.sample_rate
        && config.channels() == open.info.channels;
    if !same {
        return None;
    }
    if let Ok(mut r) = open.resampler.lock() {
        *r = ResamplerState::new();
    }
    open.id.store(id, Ordering::SeqCst);
    open.stream.play().ok()?;
    Some(OpenStream { paused: None, ..open })
}

/// How a stream's monitor may ask the recorder thread to reopen it.
struct StreamControl {
    reopen_tx: Sender<AudioCmd>,
    /// The recorder's current stream generation; this stream stops monitoring once it moves on.
    generation: Arc<AtomicU64>,
    /// This stream's generation.
    id: u64,
    /// Set when this stream replaces one that went dead.
    resumed_from: Option<MicProblem>,
    /// Whether another reopen is allowed for this recording.
    can_reopen: bool,
}

#[allow(clippy::too_many_arguments)]
fn start_stream_inner(
    active_stream: &mut Option<OpenStream>,
    buffer: Arc<Mutex<Vec<f32>>>,
    native_buffer: Arc<Mutex<Vec<f32>>>,
    is_rec: Arc<AtomicBool>,
    stop_at: Arc<Mutex<StopMark>>,
    app_handle: AppHandle,
    device_name: Option<String>,
    control: StreamControl,
) -> Result<StreamInfo, String> {
    let (device, missing_device) = resolve_input_device_reporting_fallback(device_name.as_deref())
        .ok_or_else(|| "no microphone is connected".to_string())?;
    let device_label = device.name().unwrap_or_else(|_| "Default microphone".to_string());
    if let Some(missing) = &missing_device {
        log::warn!("Microphone \"{}\" not found, recording from default \"{}\"", missing, device_label);
    }
    // One report per recording, whether it comes from the stream error callback or the level monitor.
    let reported = Arc::new(AtomicBool::new(false));

    let supported_config = match device.default_input_config() {
        Ok(c) => c,
        Err(e) => match device.supported_input_configs().ok().and_then(|mut iter| iter.next()) {
            Some(conf) => conf.with_max_sample_rate(),
            None => return Err(format!("Failed to get default input config: {}", e)),
        },
    };

    let sample_rate = supported_config.sample_rate().0;
    let channels = supported_config.channels() as usize;
    let sample_format = supported_config.sample_format();
    let stream_info = StreamInfo {
        device: device_label.clone(),
        sample_rate,
        channels: channels as u16,
        missing_device: missing_device.clone(),
    };

    let resampler_state = Arc::new(Mutex::new(ResamplerState::new()));
    let resampler_clone = Arc::clone(&resampler_state);
    let buffer_clone = Arc::clone(&buffer);
    let native_clone = Arc::clone(&native_buffer);
    let is_rec_cb = Arc::clone(&is_rec);
    let stop_at_cb = Arc::clone(&stop_at);

    let err_app = app_handle.clone();
    let err_reported = Arc::clone(&reported);
    let err_rec = Arc::clone(&is_rec);
    let err_reopen_tx = control.reopen_tx.clone();
    let err_generation = Arc::clone(&control.generation);
    // Shared so a stream resumed for a later recording reports for that one (see resume_stream).
    let stream_id = Arc::new(AtomicU64::new(control.id));
    let err_stream_id = Arc::clone(&stream_id);
    let err_can_reopen = control.can_reopen;
    let err_report = MicReport {
        problem: MicProblem::Disconnected,
        device: device_label.clone(),
        missing_device: missing_device.clone(),
        reconnecting: err_can_reopen,
    };
    let err_fn = move |err: cpal::StreamError| {
        log::error!("Audio stream error: {}", err);
        let err_id = err_stream_id.load(Ordering::SeqCst);
        let is_current = err_generation.load(Ordering::SeqCst) == err_id;
        if matches!(err, cpal::StreamError::DeviceNotAvailable) && err_rec.load(Ordering::Relaxed) && is_current {
            report_mic_problem(&err_app, &err_reported, err_report.clone());
            if err_can_reopen {
                // Reopen picks the chosen mic again, or the system default if it's gone for good.
                let _ = err_reopen_tx.send(AudioCmd::Reopen { generation: err_id, problem: MicProblem::Disconnected });
            }
        }
    };

    let stream = match sample_format {
        cpal::SampleFormat::F32 => {
            let resampler_cb = Arc::clone(&resampler_clone);
            device.build_input_stream(
                &supported_config.into(),
                move |data: &[f32], info: &cpal::InputCallbackInfo| {
                    if !is_rec_cb.load(Ordering::Relaxed) {
                        return;
                    }
                    let keep = frames_before_stop(info, data.len() / channels.max(1), sample_rate, &stop_at_cb);
                    let data = &data[..keep * channels];
                    process_input_samples(data, channels, sample_rate, &resampler_cb, &buffer_clone, &native_clone);
                },
                err_fn,
                None,
            )
        }
        cpal::SampleFormat::I16 => {
            let resampler_cb = Arc::clone(&resampler_clone);
            device.build_input_stream(
                &supported_config.into(),
                move |data: &[i16], info: &cpal::InputCallbackInfo| {
                    if !is_rec_cb.load(Ordering::Relaxed) {
                        return;
                    }
                    let keep = frames_before_stop(info, data.len() / channels.max(1), sample_rate, &stop_at_cb);
                    let floats: Vec<f32> = data[..keep * channels].iter().map(|&s| s.to_float_sample()).collect();
                    process_input_samples(&floats, channels, sample_rate, &resampler_cb, &buffer_clone, &native_clone);
                },
                err_fn,
                None,
            )
        }
        fmt => {
            return Err(format!("Unsupported audio sample format: {:?}", fmt));
        }
    }
    .map_err(|e| format!("Failed to build input audio stream: {}", e))?;

    stream
        .play()
        .map_err(|e| format!("Failed to play input audio stream: {}", e))?;

    *active_stream = Some(OpenStream {
        stream,
        info: stream_info.clone(),
        requested: device_name,
        resampler: resampler_state,
        id: stream_id,
        paused: None,
    });
    spawn_monitor(is_rec, buffer, app_handle, stream_info.clone(), control);
    Ok(stream_info)
}

/// Monitoring thread for the pill's equalizer (loudness plus pitch-band energy of the latest audio)
/// and the microphone health check: real audio must arrive within MIC_CHECK_AFTER and keep coming.
fn spawn_monitor(is_rec: Arc<AtomicBool>, buffer: Arc<Mutex<Vec<f32>>>, app_handle: AppHandle, info: StreamInfo, control: StreamControl) {
    let (device_label, missing_device) = (info.device, info.missing_device);
    let is_rec_monitor = is_rec;
    let buf_monitor = buffer;
    // A stream reopened mid-recording doesn't restart the no-speech alert.
    let no_speech_after = match NO_SPEECH_ALERT_SECS.load(Ordering::Relaxed) {
        secs if secs > 0 && control.resumed_from.is_none() => Some(Duration::from_secs(secs)),
        _ => None,
    };
    thread::spawn(move || {
        let analyzer = SpectrumAnalyzer::new();
        let mut window = vec![0.0f32; SPECTRUM_WINDOW];
        // A reopened stream appends to the existing buffer: only audio from here on is this stream's.
        let mut last_len = buf_monitor.lock().map(|b| b.len()).unwrap_or(0);
        let mut health = match control.resumed_from {
            Some(problem) => MicHealth::resumed(Instant::now(), problem),
            None => MicHealth::new(Instant::now()),
        };
        let is_current = || control.generation.load(Ordering::SeqCst) == control.id;
        let mut speech = no_speech_after.map(|after| SpeechWatch::new(Instant::now(), after, last_len));

        while is_rec_monitor.load(Ordering::Relaxed) && is_current() {
            thread::sleep(Duration::from_millis(40));

            let (has_new_audio, problem) = match buf_monitor.lock() {
                Ok(b) => {
                    let new_samples = &b[last_len.min(b.len())..];
                    let problem = health.observe(new_samples, Instant::now());
                    let has_new = b.len() > last_len;
                    if has_new {
                        last_len = b.len();
                        let n = b.len().min(SPECTRUM_WINDOW);
                        window.fill(0.0);
                        window[SPECTRUM_WINDOW - n..].copy_from_slice(&b[b.len() - n..]);
                    }
                    (has_new, problem)
                }
                Err(_) => (false, None),
            };

            match problem {
                Some(MicEvent::Problem(problem)) if is_rec_monitor.load(Ordering::Relaxed) && is_current() => {
                    // A microphone that died mid-recording is reopened (up to MAX_STREAM_RESTARTS);
                    // problems before any audio arrived cancel the recording instead.
                    let reopen = !problem.is_startup() && control.can_reopen;
                    let report = MicReport {
                        problem,
                        device: device_label.clone(),
                        missing_device: missing_device.clone(),
                        reconnecting: reopen,
                    };
                    log::warn!("Microphone health check: {:?}", report);
                    crate::app_controller::report_mic_problem(&app_handle, report);
                    if reopen {
                        let _ = control.reopen_tx.send(AudioCmd::Reopen { generation: control.id, problem });
                        break; // the replacement stream gets its own monitor
                    }
                    if problem.is_startup() {
                        break; // the recording is cancelled
                    }
                }
                Some(MicEvent::Recovered) if is_rec_monitor.load(Ordering::Relaxed) => {
                    log::info!("Microphone audio recovered on \"{}\"", device_label);
                    // A reopened stream that landed on the system default because the chosen mic is gone.
                    let switched_to = missing_device.as_ref().map(|_| device_label.clone());
                    crate::app_controller::report_mic_recovered(&app_handle, switched_to);
                }
                _ => {}
            }

            if let Some(watch) = speech.as_mut() {
                let (event, done) = watch.poll(&buf_monitor, Instant::now(), crate::vad::has_speech);
                if is_rec_monitor.load(Ordering::Relaxed) && is_current() {
                    match event {
                        Some(SpeechEvent::NotHeard) => crate::app_controller::report_no_speech(&app_handle, watch.after),
                        Some(SpeechEvent::HeardAfterWarning) => crate::app_controller::report_speech_heard(&app_handle),
                        None => {}
                    }
                }
                if done {
                    speech = None;
                }
            }

            let levels = if has_new_audio { analyzer.analyze(&window) } else { AudioLevels::silent() };
            let _ = app_handle.emit("audio-level", levels);
        }
    });
}

/// How often the recorder denoises the audio that arrived meanwhile.
const LIVE_DENOISE_TICK: Duration = Duration::from_millis(200);
/// With no start chime marked by then, none will be (it's marked within START_CHIME_MAX_WAIT).
const LIVE_DENOISE_AFTER: Duration = Duration::from_secs(3);
/// After the chime window ends: its fade-out plus the input's latency.
const LIVE_DENOISE_CHIME_MARGIN: Duration = Duration::from_millis(300);

/// Denoises the 48 kHz audio recorded since the last call, so stopping has little left to do.
/// It starts once the start chime's part of the audio is final; the chime is notched out of the
/// recording's start first, as stopping would have done before denoising.
fn feed_live_denoise(
    live: &mut Option<crate::denoise::LiveDenoise>,
    chime_done: &mut bool,
    native: &Mutex<Vec<f32>>,
    chime_until: &Mutex<Option<Instant>>,
    started: Option<Instant>,
) {
    let Some(started) = started else {
        return;
    };
    if live.is_none() {
        let chime = chime_until.lock().ok().and_then(|c| *c);
        let settled = match chime {
            Some(until) => Instant::now() > until + LIVE_DENOISE_CHIME_MARGIN,
            None => started.elapsed() > LIVE_DENOISE_AFTER,
        };
        if !settled {
            return;
        }
        if let Some(until) = chime {
            let Ok(mut b) = native.lock() else {
                return;
            };
            let window = until.saturating_duration_since(started).as_secs_f64();
            remove_start_chime(&mut b, 48000, (window * 48000.0) as usize);
            *chime_done = true;
        }
        *live = Some(crate::denoise::LiveDenoise::new());
    }
    let Some(l) = live.as_mut() else {
        return;
    };
    // Copied out so the audio callback never waits on the denoising.
    let more = match native.lock() {
        Ok(b) => b.get(l.fed()..).map(<[f32]>::to_vec).unwrap_or_default(),
        Err(_) => return,
    };
    l.feed(&more);
}

/// Added to the start chime's length: output latency plus the room's echo.
const START_CHIME_TAIL: Duration = Duration::from_millis(250);
/// How sharply the start chime's tones are notched: about 60-280 Hz wide, so speech keeps nearly
/// everything around them.
const CHIME_NOTCH_Q: f32 = 12.0;
/// Fade from notched back to untouched audio after the chime window, so no click is heard.
const CHIME_FADE_SEC: f64 = 0.05;

/// Notches the start chime's tones out of the first `window` samples, then fades back to the
/// untouched audio. Speech captured meanwhile is kept: only narrow bands around the tones go.
pub fn remove_start_chime(samples: &mut [f32], sample_rate: u32, window: usize) {
    if samples.is_empty() || window == 0 || sample_rate == 0 {
        return;
    }
    let fade = (CHIME_FADE_SEC * sample_rate as f64) as usize;
    let end = (window + fade).min(samples.len());
    let mut notches: Vec<Biquad> = crate::sound::START_CHIME_TONES
        .iter()
        .filter(|&&hz| hz < sample_rate as f32 * 0.45)
        .map(|&hz| Biquad::notch(hz, sample_rate, CHIME_NOTCH_Q))
        .collect();
    for (i, sample) in samples[..end].iter_mut().enumerate() {
        let filtered = notches.iter_mut().fold(*sample, |x, notch| notch.process(x));
        let wet = if i < window { 1.0 } else { 1.0 - (i - window) as f32 / fade.max(1) as f32 };
        *sample = filtered * wet + *sample * (1.0 - wet);
    }
}

/// A second-order IIR filter (RBJ cookbook), direct form I.
struct Biquad {
    b: [f32; 3],
    a: [f32; 2],
    x: [f32; 2],
    y: [f32; 2],
}

impl Biquad {
    fn notch(hz: f32, sample_rate: u32, q: f32) -> Self {
        let w0 = 2.0 * std::f32::consts::PI * hz / sample_rate as f32;
        let alpha = w0.sin() / (2.0 * q);
        let a0 = 1.0 + alpha;
        let cos = w0.cos();
        Self {
            b: [1.0 / a0, -2.0 * cos / a0, 1.0 / a0],
            a: [-2.0 * cos / a0, (1.0 - alpha) / a0],
            x: [0.0; 2],
            y: [0.0; 2],
        }
    }

    fn process(&mut self, x: f32) -> f32 {
        let y = self.b[0] * x + self.b[1] * self.x[0] + self.b[2] * self.x[1] - self.a[0] * self.y[0] - self.a[1] * self.y[1];
        self.x = [x, self.x[0]];
        self.y = [y, self.y[0]];
        y
    }
}

/// Number of leading frames in this callback that were captured before the user pressed stop.
/// Returns all frames while recording; once stop is requested, frames captured afterwards
/// (including the stop earcon picked up by the mic) are cut off.
fn frames_before_stop(
    info: &cpal::InputCallbackInfo,
    frames: usize,
    sample_rate: u32,
    stop_at: &Mutex<StopMark>,
) -> usize {
    let Ok(mut mark) = stop_at.lock() else {
        return frames;
    };
    let Some(stop) = mark.at else {
        return frames;
    };
    let ts = info.timestamp();
    let latency = ts.callback.duration_since(&ts.capture).unwrap_or_default();
    let Some(first_capture) = Instant::now().checked_sub(latency) else {
        mark.reached = true;
        return frames;
    };
    if stop <= first_capture {
        mark.reached = true;
        return 0;
    }
    let captured_before = (stop - first_capture).as_secs_f64() * sample_rate as f64;
    let keep = (captured_before as usize).min(frames);
    if keep < frames {
        mark.reached = true;
    }
    keep
}

#[derive(Debug)]
pub struct ResamplerState {
    pub acc: Vec<f32>,
    pub phase: f64,
}

impl ResamplerState {
    pub fn new() -> Self {
        Self {
            acc: Vec::with_capacity(4096),
            phase: 0.0,
        }
    }
}

fn process_input_samples(
    data: &[f32],
    channels: usize,
    src_rate: u32,
    resampler: &Arc<Mutex<ResamplerState>>,
    target_buffer: &Arc<Mutex<Vec<f32>>>,
    native_buffer: &Arc<Mutex<Vec<f32>>>,
) {
    if channels == 0 || data.is_empty() {
        return;
    }

    let mono_count = data.len() / channels;
    let mut mono_chunk = Vec::with_capacity(mono_count);
    for frame in data.chunks_exact(channels) {
        let sum: f32 = frame.iter().sum();
        mono_chunk.push(sum / channels as f32);
    }

    if src_rate != 16000 {
        if let Ok(mut native) = native_buffer.lock() {
            native.extend_from_slice(&mono_chunk);
        }
    }
    resample_mono_to_16k(&mono_chunk, src_rate, resampler, target_buffer);
}

/// Resamples a whole mono recording to 16 kHz, e.g. a saved history file for the Lab.
pub fn resample_to_16k(samples: &[f32], src_rate: u32) -> Vec<f32> {
    if src_rate == 16000 {
        return samples.to_vec();
    }
    let resampler = Arc::new(Mutex::new(ResamplerState::new()));
    let target = Arc::new(Mutex::new(Vec::with_capacity(samples.len() * 16000 / src_rate.max(1) as usize)));
    resample_mono_to_16k(samples, src_rate, &resampler, &target);
    let out = target.lock().map(|mut t| std::mem::take(&mut *t)).unwrap_or_default();
    out
}

fn resample_mono_to_16k(
    mono_chunk: &[f32],
    src_rate: u32,
    resampler: &Arc<Mutex<ResamplerState>>,
    target_buffer: &Arc<Mutex<Vec<f32>>>,
) {
    if src_rate == 16000 {
        if let Ok(mut target) = target_buffer.lock() {
            target.extend_from_slice(&mono_chunk);
        }
    } else {
        if let Ok(mut state) = resampler.lock() {
            state.acc.extend_from_slice(&mono_chunk);

            let ratio = src_rate as f64 / 16000.0;
            // Anti-aliasing low-pass cutoff at 7.2 kHz (target Nyquist is 8.0 kHz)
            let fc = (7200.0 / src_rate as f64).min(0.48);
            const RADIUS: i32 = 16; // 32-tap windowed sinc filter
            let radius_f = RADIUS as f64;

            let mut resampled = Vec::new();
            let mut current_pos = state.phase;

            let acc_len = state.acc.len();
            while current_pos + radius_f < acc_len as f64 {
                let center_idx = current_pos.round() as i32;
                let mut sum = 0.0f32;
                let mut weight_sum = 0.0f32;

                for k in (center_idx - RADIUS)..=(center_idx + RADIUS) {
                    if k >= 0 && (k as usize) < acc_len {
                        let diff = k as f64 - current_pos;
                        // Bandlimited sinc filter with anti-aliasing cutoff
                        let sinc_val = sinc(2.0 * fc * diff);
                        // Hann window
                        let win = 0.5 * (1.0 + (std::f64::consts::PI * diff / radius_f).cos());
                        let w = (sinc_val * win) as f32;

                        sum += state.acc[k as usize] * w;
                        weight_sum += w;
                    }
                }

                if weight_sum.abs() > 1e-6 {
                    resampled.push(sum / weight_sum);
                } else {
                    let idx_clamped = (current_pos.round() as usize).min(acc_len.saturating_sub(1));
                    resampled.push(state.acc[idx_clamped]);
                }

                current_pos += ratio;
            }

            // Keep samples required for the next window tail
            let consumed = current_pos.floor() as usize;
            if consumed > RADIUS as usize {
                let remove_count = consumed - RADIUS as usize;
                let cur_len = state.acc.len();
                state.acc.drain(0..remove_count.min(cur_len));
                state.phase = current_pos - remove_count as f64;
            } else {
                state.phase = current_pos;
            }

            if !resampled.is_empty() {
                if let Ok(mut target) = target_buffer.lock() {
                    target.extend_from_slice(&resampled);
                }
            }
        }
    }
}

#[inline]
fn sinc(x: f64) -> f64 {
    if x.abs() < 1e-9 {
        1.0
    } else {
        let px = std::f64::consts::PI * x;
        px.sin() / px
    }
}

/// Samples per spectrum frame: 512 at 16 kHz = 32 ms, 31.25 Hz per frequency bin.
const SPECTRUM_WINDOW: usize = 512;
const SPECTRUM_RATE: f32 = 16000.0;
/// Pitch bands, low to high: voice fundamentals, vowels, consonants, sibilants and breath.
const PITCH_BANDS_HZ: [(f32, f32); 4] = [(100.0, 350.0), (350.0, 1200.0), (1200.0, 3500.0), (3500.0, 7000.0)];
/// Speech energy falls off with frequency, so higher bands get a boost (dB) to register visibly.
const BAND_TILT_DB: [f32; 4] = [0.0, 5.0, 10.0, 14.0];
/// A band this many dB below the loudest band reads as zero.
const BAND_RANGE_DB: f32 = 24.0;
/// Below this loudness the pill treats the input as silence.
const SILENCE_VOLUME: f32 = 0.03;

/// Payload of the `audio-level` event that drives the pill's equalizer.
#[derive(Clone, Debug, serde::Serialize)]
pub struct AudioLevels {
    /// Overall loudness, 0..1.
    pub volume: f32,
    /// Pitch energy per bar, 0..1, mirrored from the centre:
    /// [high, high-mid, mid, low, mid, high-mid, high].
    pub bands: [f32; 7],
}

impl AudioLevels {
    pub fn silent() -> Self {
        Self { volume: 0.0, bands: [0.0; 7] }
    }
}

/// Splits short audio frames into pitch bands with a Hann-windowed DFT over the bins of interest.
struct SpectrumAnalyzer {
    hann: Vec<f32>,
    cos: Vec<f32>,
    sin: Vec<f32>,
    band_bins: [(usize, usize); 4],
}

impl SpectrumAnalyzer {
    fn new() -> Self {
        let n = SPECTRUM_WINDOW;
        let tau = 2.0 * std::f32::consts::PI;
        let hann = (0..n).map(|i| 0.5 - 0.5 * (tau * i as f32 / (n - 1) as f32).cos()).collect();
        let cos = (0..n).map(|i| (tau * i as f32 / n as f32).cos()).collect();
        let sin = (0..n).map(|i| (tau * i as f32 / n as f32).sin()).collect();
        let bin_hz = SPECTRUM_RATE / n as f32;
        let band_bins = PITCH_BANDS_HZ.map(|(lo, hi)| ((lo / bin_hz).ceil() as usize, (hi / bin_hz).floor() as usize));
        Self { hann, cos, sin, band_bins }
    }

    fn analyze(&self, frame: &[f32]) -> AudioLevels {
        let n = SPECTRUM_WINDOW;
        debug_assert_eq!(frame.len(), n);
        let rms = (frame.iter().map(|x| x * x).sum::<f32>() / n as f32).sqrt();
        // Same speech sensitivity curve the pill has always used.
        let volume = (rms * 14.0).clamp(0.0, 1.0);
        if volume < SILENCE_VOLUME {
            return AudioLevels { volume: 0.0, bands: [0.0; 7] };
        }

        let windowed: Vec<f32> = frame.iter().zip(&self.hann).map(|(x, w)| x * w).collect();
        let band_db = self.band_bins.map(|(lo, hi)| {
            let mut power = 0.0f32;
            for k in lo..=hi {
                let (mut re, mut im) = (0.0f32, 0.0f32);
                for (t, &x) in windowed.iter().enumerate() {
                    let idx = (k * t) % n;
                    re += x * self.cos[idx];
                    im -= x * self.sin[idx];
                }
                power += re * re + im * im;
            }
            10.0 * (power / (hi - lo + 1) as f32 + 1e-12).log10()
        });

        let tilted: Vec<f32> = band_db.iter().zip(BAND_TILT_DB).map(|(db, tilt)| db + tilt).collect();
        let loudest = tilted.iter().cloned().fold(f32::MIN, f32::max);
        let weight = ((volume - SILENCE_VOLUME) / 0.06).clamp(0.0, 1.0);
        let energy: Vec<f32> = tilted
            .iter()
            .map(|db| ((db - (loudest - BAND_RANGE_DB)) / BAND_RANGE_DB).clamp(0.0, 1.0) * weight)
            .collect();
        let [low, mid, high_mid, high] = [energy[0], energy[1], energy[2], energy[3]];
        AudioLevels { volume, bands: [high, high_mid, mid, low, mid, high_mid, high] }
    }
}

#[cfg(target_os = "macos")]
mod coreaudio_listener {
    use std::ffi::c_void;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::mpsc::Sender;
    use std::sync::OnceLock;

    #[repr(C)]
    struct AudioObjectPropertyAddress {
        m_selector: u32,
        m_scope: u32,
        m_element: u32,
    }

    #[link(name = "CoreAudio", kind = "framework")]
    extern "C" {
        fn AudioObjectAddPropertyListener(
            in_object_id: u32,
            in_address: *const AudioObjectPropertyAddress,
            in_listener: unsafe extern "C" fn(u32, u32, *const AudioObjectPropertyAddress, *mut c_void) -> i32,
            in_client_data: *mut c_void,
        ) -> i32;
    }

    static SENDER: OnceLock<Sender<()>> = OnceLock::new();
    static REGISTERED: AtomicBool = AtomicBool::new(false);

    unsafe extern "C" fn on_audio_hardware_change(
        _in_object_id: u32,
        _in_number_addresses: u32,
        _in_addresses: *const AudioObjectPropertyAddress,
        _in_client_data: *mut c_void,
    ) -> i32 {
        if let Some(tx) = SENDER.get() {
            let _ = tx.send(());
        }
        0
    }

    pub fn register_listener(tx: Sender<()>) {
        if REGISTERED.swap(true, Ordering::SeqCst) {
            return;
        }
        let _ = SENDER.set(tx);

        const K_AUDIO_OBJECT_SYSTEM_OBJECT: u32 = 1;
        const K_AUDIO_OBJECT_PROPERTY_SCOPE_GLOBAL: u32 = 0x676c6f62; // 'glob'
        const K_AUDIO_OBJECT_PROPERTY_ELEMENT_MAIN: u32 = 0;

        // 'dev#' = 0x64657623 (device list changed: plug / unplug)
        // 'dIn ' = 0x64496e20 (default input device changed)
        // 'dOut' = 0x644f7574 (default output device changed)
        let selectors = [
            0x64657623, // 'dev#'
            0x64496e20, // 'dIn '
            0x644f7574, // 'dOut'
        ];

        for &sel in &selectors {
            let address = AudioObjectPropertyAddress {
                m_selector: sel,
                m_scope: K_AUDIO_OBJECT_PROPERTY_SCOPE_GLOBAL,
                m_element: K_AUDIO_OBJECT_PROPERTY_ELEMENT_MAIN,
            };
            unsafe {
                let status = AudioObjectAddPropertyListener(
                    K_AUDIO_OBJECT_SYSTEM_OBJECT,
                    &address,
                    on_audio_hardware_change,
                    std::ptr::null_mut(),
                );
                if status != 0 {
                    log::warn!("CoreAudio: failed to add property listener for selector 0x{:08x}: {}", sel, status);
                }
            }
        }
    }
}

pub fn start_device_monitor(app_handle: AppHandle) {
    let (tx, rx) = channel::<()>();

    #[cfg(target_os = "macos")]
    coreaudio_listener::register_listener(tx);

    thread::Builder::new()
        .name("audio-device-monitor".into())
        .spawn(move || {
            // Nothing sends here off macOS; holding the sender keeps the channel from disconnecting.
            #[cfg(not(target_os = "macos"))]
            let _tx = tx;
            let (mut last_inputs, mut last_outputs) = list_devices();

            loop {
                // macOS tells us when devices change; elsewhere the lists are checked every 2 seconds.
                #[cfg(target_os = "macos")]
                let received_event = match rx.recv() {
                    Ok(_) => true,
                    Err(_) => break,
                };
                #[cfg(not(target_os = "macos"))]
                let received_event = match rx.recv_timeout(Duration::from_secs(2)) {
                    Ok(_) => true,
                    Err(std::sync::mpsc::RecvTimeoutError::Timeout) => false,
                    Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
                };

                if received_event {
                    // Debounce: allow macOS CoreAudio HAL to settle device state (e.g. bluetooth handshake)
                    thread::sleep(Duration::from_millis(200));
                    // Drain any queued events that arrived during debounce
                    while rx.try_recv().is_ok() {}
                }

                let (current_inputs, current_outputs) = list_devices();

                if current_inputs != last_inputs || current_outputs != last_outputs {
                    DEVICE_CHANGES.fetch_add(1, Ordering::SeqCst);
                    log::info!(
                        "Audio devices changed: inputs={:?}, outputs={:?}",
                        current_inputs,
                        current_outputs
                    );

                    let settings = if let Some(state) = app_handle.try_state::<crate::AppState>() {
                        state.controller.get_settings()
                    } else {
                        crate::settings::AppSettings::load()
                    };

                    crate::tray::update_tray_audio_devices_with_lists(
                        &app_handle,
                        &settings,
                        &current_inputs,
                        &current_outputs,
                    );

                    let _ = app_handle.emit(
                        "audio-devices-changed",
                        serde_json::json!({
                            "input_devices": &current_inputs,
                            "output_devices": &current_outputs,
                        }),
                    );

                    last_inputs = current_inputs;
                    last_outputs = current_outputs;
                }
            }
        })
        .expect("failed to spawn audio-device-monitor thread");
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sine(hz: f32, rate: u32, secs: f32, amplitude: f32) -> Vec<f32> {
        (0..(rate as f32 * secs) as usize)
            .map(|i| amplitude * (2.0 * std::f32::consts::PI * hz * i as f32 / rate as f32).sin())
            .collect()
    }

    fn rms(s: &[f32]) -> f32 {
        (s.iter().map(|x| x * x).sum::<f32>() / s.len().max(1) as f32).sqrt()
    }

    #[test]
    fn start_chime_tones_are_removed_and_voice_is_kept() {
        let rate = 16000;
        let window = rate as usize / 2;
        for &tone in &crate::sound::START_CHIME_TONES {
            let mut chime = sine(tone, rate, 1.0, 0.3);
            remove_start_chime(&mut chime, rate, window);
            // Skip the filter's first 50 ms while it settles.
            let after = rms(&chime[800..window]) / rms(&sine(tone, rate, 0.5, 0.3)[800..]);
            assert!(after < 0.1, "{tone} Hz left at {after}");
        }
        // A voice pitch and a vowel formant away from the tones pass nearly untouched.
        for voice in [150.0, 500.0, 2000.0] {
            let original = sine(voice, rate, 1.0, 0.3);
            let mut kept = original.clone();
            remove_start_chime(&mut kept, rate, window);
            let ratio = rms(&kept[800..window]) / rms(&original[800..window]);
            assert!(ratio > 0.85, "{voice} Hz kept only {ratio}");
        }
    }

    #[test]
    fn audio_after_the_chime_window_is_untouched() {
        let rate = 48000;
        let original = sine(878.0, rate, 1.0, 0.3);
        let mut out = original.clone();
        let window = rate as usize / 4;
        remove_start_chime(&mut out, rate, window);
        let fade_end = window + (CHIME_FADE_SEC * rate as f64) as usize;
        assert_eq!(out[fade_end..], original[fade_end..]);
    }

    #[test]
    fn speech_watch_warns_once_then_clears_when_speech_arrives() {
        let t0 = Instant::now();
        let buffer = Mutex::new(vec![0.0f32; 16000]);
        let mut watch = SpeechWatch::new(t0, Duration::from_secs(5), 0);
        let quiet = |_: &[f32]| false;
        assert_eq!(watch.poll(&buffer, ms(t0, 500), quiet), (None, false), "not due yet");
        assert_eq!(watch.poll(&buffer, ms(t0, 1000), quiet), (None, false), "before the alert time");
        assert_eq!(watch.poll(&buffer, ms(t0, 5000), quiet), (Some(SpeechEvent::NotHeard), false));
        assert_eq!(watch.poll(&buffer, ms(t0, 6000), quiet), (None, false), "warns only once");
        assert_eq!(watch.poll(&buffer, ms(t0, 7000), |_| true), (Some(SpeechEvent::HeardAfterWarning), true));
    }

    #[test]
    fn speech_watch_stops_quietly_when_speech_comes_first() {
        let t0 = Instant::now();
        let buffer = Mutex::new(vec![0.0f32; 16000]);
        let mut watch = SpeechWatch::new(t0, Duration::from_secs(5), 0);
        assert_eq!(watch.poll(&buffer, ms(t0, 1000), |_| true), (None, true));
    }

    #[test]
    fn speech_watch_rereads_a_little_already_checked_audio() {
        let t0 = Instant::now();
        let buffer = Mutex::new(vec![0.0f32; 20000]);
        let mut watch = SpeechWatch::new(t0, Duration::from_secs(5), 4000);
        let seen = std::cell::Cell::new(0);
        watch.poll(&buffer, ms(t0, 1000), |c: &[f32]| { seen.set(c.len()); false });
        assert_eq!(seen.get(), 16000, "only this stream's audio");
        buffer.lock().unwrap().extend(vec![0.0f32; 16000]);
        watch.poll(&buffer, ms(t0, 2000), |c: &[f32]| { seen.set(c.len()); false });
        assert_eq!(seen.get(), 16000 + SPEECH_CHECK_OVERLAP);
    }

    #[test]
    fn default_microphone_settings_name_no_device() {
        for setting in [None, Some(""), Some("  "), Some("Default"), Some("System default"), Some("System Default")] {
            assert_eq!(chosen_input_name(setting), None, "{setting:?}");
            assert_eq!(missing_input_device(setting), None, "{setting:?}");
        }
        assert_eq!(chosen_input_name(Some(" HATOR Signify ")), Some("HATOR Signify"));
    }

    fn tone(hz: f32, amplitude: f32) -> Vec<f32> {
        (0..SPECTRUM_WINDOW)
            .map(|i| amplitude * (2.0 * std::f32::consts::PI * hz * i as f32 / SPECTRUM_RATE).sin())
            .collect()
    }

    #[test]
    fn low_tone_lifts_the_centre_bar() {
        let levels = SpectrumAnalyzer::new().analyze(&tone(200.0, 0.2));
        assert!(levels.volume > 0.5);
        assert_eq!(levels.bands[3], 1.0, "{:?}", levels.bands);
        assert!(levels.bands[0] < 0.5, "{:?}", levels.bands);
        assert_eq!(levels.bands[2], levels.bands[4], "bars mirror around the centre");
    }

    #[test]
    fn high_tone_lifts_the_edge_bars() {
        let levels = SpectrumAnalyzer::new().analyze(&tone(5000.0, 0.2));
        assert_eq!(levels.bands[0], 1.0, "{:?}", levels.bands);
        assert_eq!(levels.bands[6], 1.0, "{:?}", levels.bands);
        assert!(levels.bands[3] < 0.5, "{:?}", levels.bands);
    }

    fn ms(t0: Instant, ms: u64) -> Instant {
        t0 + Duration::from_millis(ms)
    }

    #[test]
    fn mic_health_waits_for_the_check_window() {
        let t0 = Instant::now();
        let mut h = MicHealth::new(t0);
        assert_eq!(h.observe(&[], ms(t0, 1000)), None);
        assert_eq!(h.observe(&[], ms(t0, 2400)), None);
    }

    #[test]
    fn mic_health_flags_no_audio_and_silence() {
        let t0 = Instant::now();
        let mut none = MicHealth::new(t0);
        assert_eq!(none.observe(&[], ms(t0, 2600)), Some(MicEvent::Problem(MicProblem::NoAudio)));

        let mut zeros = MicHealth::new(t0);
        assert_eq!(zeros.observe(&[0.0; 1600], ms(t0, 500)), None);
        assert_eq!(zeros.observe(&[0.0; 1600], ms(t0, 2600)), Some(MicEvent::Problem(MicProblem::Silent)));
    }

    #[test]
    fn mic_health_passes_a_live_mic_then_flags_a_stall() {
        let t0 = Instant::now();
        let mut h = MicHealth::new(t0);
        let noise = [0.003f32, -0.002, 0.004, -0.001];
        assert_eq!(h.observe(&noise, ms(t0, 500)), None);
        assert_eq!(h.observe(&noise, ms(t0, 2600)), None, "live mic passes the check");
        assert_eq!(h.observe(&[], ms(t0, 3500)), None, "short gap is fine");
        assert_eq!(h.observe(&[], ms(t0, 4200)), Some(MicEvent::Problem(MicProblem::Stalled)));
        assert_eq!(h.observe(&[], ms(t0, 5000)), None, "reported once");
        assert_eq!(h.observe(&noise, ms(t0, 5100)), Some(MicEvent::Recovered));
    }

    #[test]
    fn mic_health_flags_audio_that_turns_into_digital_silence() {
        let t0 = Instant::now();
        let mut h = MicHealth::new(t0);
        let voice = [0.2f32, -0.15, 0.1, -0.05];
        assert_eq!(h.observe(&voice, ms(t0, 1000)), None);
        assert_eq!(h.observe(&voice, ms(t0, 2600)), None);
        // Samples keep arriving, but they are all exact zeros (a dropped Bluetooth call-audio link).
        assert_eq!(h.observe(&[0.0; 640], ms(t0, 3000)), None);
        assert_eq!(h.observe(&[0.0; 640], ms(t0, 3800)), None, "under 1 s of exact zeros");
        assert_eq!(h.observe(&[0.0; 640], ms(t0, 4100)), Some(MicEvent::Problem(MicProblem::WentSilent)));
        assert_eq!(h.observe(&[0.0; 640], ms(t0, 4500)), None, "reported once");
        assert_eq!(h.observe(&voice, ms(t0, 5000)), Some(MicEvent::Recovered));
    }

    #[test]
    fn mic_health_needs_3_s_of_near_silence_that_is_not_exact_zeros() {
        let t0 = Instant::now();
        let mut h = MicHealth::new(t0);
        let voice = [0.2f32, -0.15, 0.1, -0.05];
        let faint = [0.00002f32, -0.00001, 0.00003, -0.00002];
        assert_eq!(h.observe(&voice, ms(t0, 2600)), None);
        assert_eq!(h.observe(&faint, ms(t0, 3000)), None);
        assert_eq!(h.observe(&faint, ms(t0, 5500)), None, "near-silence under 3 s");
        assert_eq!(h.observe(&faint, ms(t0, 6100)), Some(MicEvent::Problem(MicProblem::WentSilent)));
    }

    #[test]
    fn reopened_stream_recovers_on_first_real_audio_or_reports_again() {
        let t0 = Instant::now();
        let voice = [0.2f32, -0.15, 0.1, -0.05];
        let mut back = MicHealth::resumed(t0, MicProblem::WentSilent);
        assert_eq!(back.observe(&[0.0; 640], ms(t0, 400)), None, "no startup check after a reopen");
        assert_eq!(back.observe(&voice, ms(t0, 700)), Some(MicEvent::Recovered));

        let mut still_dead = MicHealth::resumed(t0, MicProblem::WentSilent);
        assert_eq!(still_dead.observe(&[0.0; 640], ms(t0, 2900)), None);
        assert_eq!(still_dead.observe(&[0.0; 640], ms(t0, 3100)), Some(MicEvent::Problem(MicProblem::WentSilent)));
        assert_eq!(still_dead.observe(&[0.0; 640], ms(t0, 3500)), None, "reported once");
    }

    #[test]
    fn mic_health_ignores_quiet_but_live_pauses() {
        let t0 = Instant::now();
        let mut h = MicHealth::new(t0);
        let room_tone = [0.0004f32, -0.0003, 0.0005, -0.0002];
        for step in 1..=200 {
            assert_eq!(h.observe(&room_tone, ms(t0, step * 50)), None);
        }
    }

    #[test]
    fn capture_stats_find_the_dead_stretch() {
        // 2 s of voice, then 3 s of zeros, then 1 s of voice, with two invalid samples.
        let mut samples: Vec<f32> = (0..32000).map(|i| (i as f32 * 0.05).sin() * 0.2).collect();
        samples.extend(std::iter::repeat(0.0).take(48000));
        samples.extend((0..16000).map(|i| (i as f32 * 0.05).sin() * 0.2));
        samples[100] = f32::NAN;
        samples[200] = f32::INFINITY;
        let stats = CaptureStats::measure(&samples);
        assert!((stats.captured_sec - 6.0).abs() < 1e-3);
        assert!((stats.zero_sec - 3.0).abs() < 0.02, "{stats:?}");
        assert!((stats.longest_zero_sec - 3.0).abs() < 0.02, "{stats:?}");
        assert!((stats.longest_zero_at_sec - 2.0).abs() < 0.02, "{stats:?}");
        assert_eq!(stats.invalid_samples, 2);
    }

    #[test]
    fn silence_has_no_band_energy() {
        let levels = SpectrumAnalyzer::new().analyze(&vec![0.0; SPECTRUM_WINDOW]);
        assert_eq!(levels.volume, 0.0);
        assert_eq!(levels.bands, [0.0; 7]);
    }

    #[test]
    fn denoised_recording_keeps_length_and_level() {
        let native: Vec<f32> = (0..48000 * 2)
            .map(|i| {
                let t = i as f32 / 48000.0;
                let phase = 2.0 * std::f32::consts::PI * (150.0 * t + 60.0 * t * t);
                0.02 * (phase.sin() + 0.5 * (3.0 * phase).sin())
            })
            .collect();
        let speech_raw = resample_to_16k(&native, 48000);
        let captured = CapturedAudio::new(speech_raw.clone(), native.clone(), 48000, Some(0.9), None);

        assert!((captured.speech.len() as i64 - speech_raw.len() as i64).abs() <= 32, "{} vs {}", captured.speech.len(), speech_raw.len());
        let (playback, rate) = captured.playback();
        assert_eq!((playback.len(), rate), (native.len(), 48000));
        let level = crate::leveler::speech_level_db(&captured.speech, 16000).unwrap();
        assert!((level + 18.0).abs() < 2.0, "speech level {level}");
    }

    #[test]
    fn test_sinc_resampler_48k_to_16k() {
        let resampler = Arc::new(Mutex::new(ResamplerState::new()));
        let target = Arc::new(Mutex::new(Vec::new()));
        let native = Arc::new(Mutex::new(Vec::new()));

        // Generate 4800 samples at 48000 Hz = 0.1 second of 1000 Hz tone
        let mut input = Vec::with_capacity(4800);
        for i in 0..4800 {
            let t = i as f32 / 48000.0;
            input.push((2.0 * std::f32::consts::PI * 1000.0 * t).sin());
        }

        // Feed in 3 chunks to test streaming continuity
        for chunk in input.chunks(1600) {
            process_input_samples(chunk, 1, 48000, &resampler, &target, &native);
        }

        assert_eq!(native.lock().unwrap().len(), 4800, "native-rate copy keeps every input sample");
        let output = target.lock().unwrap().clone();
        // 0.1 sec at 16000 Hz is ~1600 samples (give or take filter margin)
        assert!(output.len() >= 1550 && output.len() <= 1620, "Output length: {}", output.len());
        for &sample in &output {
            assert!(!sample.is_nan());
            assert!(sample.abs() <= 1.2);
        }
    }
}


