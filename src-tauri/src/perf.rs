//! Performance log for spotting slowdowns over time.
//!
//! Every dictation appends one JSON line to `logs/performance.jsonl` with the time spent in each
//! pipeline stage plus CPU, memory, swap and thermal readings sampled while it was processed. A
//! heartbeat line is added every few minutes so slow drift (e.g. memory growth while idle) shows up
//! between dictations too. A run that fails or is cancelled is still written (on drop), with
//! `outcome` saying so and its last stage showing where it stopped.

use serde::Serialize;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, OnceLock};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};
use sysinfo::{ProcessRefreshKind, ProcessesToUpdate, System};

use crate::settings::get_data_dir;

const SAMPLE_INTERVAL: Duration = Duration::from_millis(250);
const HEARTBEAT_INTERVAL: Duration = Duration::from_secs(300);
/// performance.jsonl is renamed to performance.1.jsonl once it reaches this size.
const MAX_LOG_BYTES: u64 = 10 * 1024 * 1024;

static APP_START: OnceLock<Instant> = OnceLock::new();
static DICTATIONS: AtomicU64 = AtomicU64::new(0);

/// Pipeline stages in order. A run that ends early files its unfinished time under the stage after
/// the last one it completed.
const STAGE_ORDER: [&str; 11] = [
    "stop_capture",
    "audio_prep",
    "vad",
    "save_audio",
    "model_ready",
    "inference",
    "post_process",
    "translation",
    "paste",
    "history_db",
    "finalize",
];

/// Records the app start time and starts the heartbeat. Call once at startup.
pub fn init() {
    APP_START.get_or_init(Instant::now);
    let _ = thread::Builder::new().name("bifaria-perf-heartbeat".to_string()).spawn(|| {
        let mut probe = Probe::new();
        loop {
            thread::sleep(HEARTBEAT_INTERVAL);
            let reading = probe.read();
            let context = SystemContext::collect();
            append(&serde_json::json!({
                "type": "heartbeat",
                "ts": crate::pipeline_logger::current_timestamp(),
                "app_version": env!("CARGO_PKG_VERSION"),
                "app_uptime_sec": uptime_sec(),
                "dictations_since_launch": DICTATIONS.load(Ordering::Relaxed),
                // CPU averaged over the whole interval since the previous heartbeat.
                "app_cpu_pct": reading.app_cpu_pct,
                "app_rss_mb": reading.app_rss_mb,
                "system_cpu_pct": reading.system_cpu_pct,
                "available_mem_mb": reading.available_mem_mb,
                "swap_used_mb": reading.swap_used_mb,
                "context": context,
            }));
        }
    });
}

fn uptime_sec() -> u64 {
    APP_START.get().map(|t| t.elapsed().as_secs()).unwrap_or(0)
}

/// One dictation's timings and resource usage. Stages are recorded as laps: each `lap` covers the
/// time since the previous one.
pub struct PerfRun {
    started: Instant,
    last_lap: Instant,
    /// When the run ended, if `set_outcome` ended it before the pipeline returned (e.g. while an
    /// error stays on the pill).
    ended: Option<Instant>,
    stages: Vec<(&'static str, u64)>,
    sampler: Option<Sampler>,
    outcome: &'static str,
    pub model: String,
    pub model_load_mode: String,
    pub recording_sec: f32,
    /// The microphone that recorded, and its sample rate.
    pub input_device: Option<(String, u32)>,
    pub audio_sec: f32,
    pub speech_sec: f32,
    /// VAD found no speech and kept the whole recording, so `speech_sec` is the full audio.
    pub vad_no_speech: bool,
    /// Time the model itself spent on inference (from the model); the rest of the `inference`
    /// lap was spent waiting for the model to load or for its lock.
    pub inference_ms: Option<u64>,
    pub threads: Option<i32>,
    pub translation: Option<String>,
    /// Languages translated, as codes ("uk → en", "auto → en" when the spoken one wasn't known).
    pub translation_pair: Option<String>,
    /// The LLM that translated, or Apple's "fast" / "quality" model.
    pub translation_model: Option<String>,
    /// Time the translation service took, and characters sent and received (0 received when it
    /// failed). Set when a service was called.
    pub translation_ms: Option<u64>,
    pub translation_chars: Option<(usize, usize)>,
    pub text_chars: usize,
}

impl PerfRun {
    /// Starts timing at the moment the recording stopped. `stop_capture_ms` is how long stopping
    /// the recorder took (flush, denoise, resample), measured before the run existed.
    pub fn start(stop_capture_ms: u64) -> Self {
        let now = Instant::now();
        DICTATIONS.fetch_add(1, Ordering::Relaxed);
        PerfRun {
            started: now,
            last_lap: now,
            ended: None,
            stages: vec![("stop_capture", stop_capture_ms)],
            sampler: Some(Sampler::start()),
            outcome: "aborted",
            model: String::new(),
            model_load_mode: String::new(),
            recording_sec: 0.0,
            input_device: None,
            audio_sec: 0.0,
            speech_sec: 0.0,
            vad_no_speech: false,
            inference_ms: None,
            threads: None,
            translation: None,
            translation_pair: None,
            translation_model: None,
            translation_ms: None,
            translation_chars: None,
            text_chars: 0,
        }
    }

    /// Ends the current stage, named `stage`, and starts the next one.
    pub fn lap(&mut self, stage: &'static str) {
        let now = Instant::now();
        self.stages.push((stage, now.duration_since(self.last_lap).as_millis() as u64));
        self.last_lap = now;
    }

    /// How the run ended, for runs that return early ("cancelled", "failed", ...). Ends the run
    /// now: the time since the last lap goes to the stage that was in progress, and whatever the
    /// pipeline does afterwards (holding an error on the pill) isn't counted. Written on drop.
    pub fn set_outcome(&mut self, outcome: &'static str) {
        self.outcome = outcome;
        if self.ended.is_some() {
            return;
        }
        let now = Instant::now();
        let in_progress = self
            .stages
            .last()
            .and_then(|(last, _)| STAGE_ORDER.iter().position(|s| s == last))
            .and_then(|i| STAGE_ORDER.get(i + 1));
        let ms = now.duration_since(self.last_lap).as_millis() as u64;
        if let (Some(stage), true) = (in_progress, ms > 0) {
            self.stages.push((stage, ms));
        }
        self.last_lap = now;
        self.ended = Some(now);
        if let Some(sampler) = &self.sampler {
            sampler.request_stop();
        }
    }

    /// Ends the run and writes it. A run dropped without this is written as "aborted" unless
    /// `set_outcome` said otherwise.
    pub fn finish(mut self, outcome: &'static str) {
        self.outcome = outcome;
    }

    fn write(&mut self) {
        let end = self.ended.unwrap_or_else(Instant::now);
        let total_ms = end.duration_since(self.started).as_millis() as u64 + self.stages[0].1;
        let usage = self.sampler.take().map(Sampler::stop).unwrap_or_default();
        let stages: serde_json::Map<String, serde_json::Value> =
            self.stages.iter().map(|(name, ms)| (name.to_string(), (*ms).into())).collect();
        let model_wait_ms = self.inference_ms.and_then(|inf| {
            self.stages.iter().find(|(n, _)| *n == "inference").map(|(_, wall)| wall.saturating_sub(inf))
        });
        // Seconds of speech per second of inference; a falling value is the clearest sign of slowdown.
        let speed_factor = self
            .inference_ms
            .filter(|ms| *ms > 0)
            .map(|ms| (self.speech_sec as f64 * 1000.0 / ms as f64 * 10.0).round() / 10.0);

        let record = serde_json::json!({
            "type": "dictation",
            "ts": crate::pipeline_logger::current_timestamp(),
            "app_version": env!("CARGO_PKG_VERSION"),
            "outcome": self.outcome,
            "total_ms": total_ms,
            "stages_ms": stages,
            "inference_ms": self.inference_ms,
            "model_wait_ms": model_wait_ms,
            "speed_factor": speed_factor,
            "model": self.model,
            "model_load_mode": self.model_load_mode,
            "threads": self.threads,
            "recording_sec": round1(self.recording_sec),
            "audio_sec": round1(self.audio_sec),
            "speech_sec": round1(self.speech_sec),
            "input_device": self.input_device.as_ref().map(|(name, _)| name),
            "input_rate_hz": self.input_device.as_ref().map(|(_, rate)| rate),
            "vad_no_speech": self.vad_no_speech,
            "translation": self.translation,
            "translation_pair": self.translation_pair,
            "translation_model": self.translation_model,
            "translation_ms": self.translation_ms,
            "translation_in_chars": self.translation_chars.map(|(sent, _)| sent),
            "translation_out_chars": self.translation_chars.map(|(_, got)| got),
            "text_chars": self.text_chars,
            "app_uptime_sec": uptime_sec(),
            "dictations_since_launch": DICTATIONS.load(Ordering::Relaxed),
            "usage": usage,
            "context": SystemContext::collect(),
        });
        append(&record);

        let stage_list: Vec<String> = self.stages.iter().map(|(n, ms)| format!("{} {}", n, ms)).collect();
        crate::pipeline_logger::log_stage_event(
            &get_data_dir(),
            "PERF",
            &format!(
                "{} in {} ms [{}] | app CPU avg {:.0}% peak {:.0}% | app RAM peak {} MB | system CPU avg {:.0}% | free RAM min {} MB",
                self.outcome,
                total_ms,
                stage_list.join(", "),
                usage.app_cpu_avg_pct,
                usage.app_cpu_peak_pct,
                usage.app_rss_peak_mb,
                usage.system_cpu_avg_pct,
                usage.available_mem_min_mb,
            ),
        );
    }
}

impl Drop for PerfRun {
    fn drop(&mut self) {
        self.write();
    }
}

fn round1(v: f32) -> f64 {
    (v as f64 * 10.0).round() / 10.0
}

fn append(value: &serde_json::Value) {
    let dir = get_data_dir().join("logs");
    let _ = fs::create_dir_all(&dir);
    let path = dir.join("performance.jsonl");
    if fs::metadata(&path).map(|m| m.len() >= MAX_LOG_BYTES).unwrap_or(false) {
        let _ = fs::rename(&path, dir.join("performance.1.jsonl"));
    }
    if let Ok(mut f) = OpenOptions::new().create(true).append(true).open(&path) {
        let _ = writeln!(f, "{}", value);
    }
}

/// Reads this process's CPU and memory and the system's CPU, memory and swap.
struct Probe {
    sys: System,
    pid: Option<sysinfo::Pid>,
}

#[derive(Clone, Copy, Default)]
struct Reading {
    /// Percent of one core (400% = four cores busy).
    app_cpu_pct: f32,
    app_rss_mb: u64,
    system_cpu_pct: f32,
    available_mem_mb: u64,
    swap_used_mb: u64,
}

impl Probe {
    fn new() -> Self {
        let mut probe = Probe { sys: System::new(), pid: sysinfo::get_current_pid().ok() };
        // CPU usage is measured between refreshes, so prime it once.
        probe.read();
        probe
    }

    fn read(&mut self) -> Reading {
        self.sys.refresh_cpu_usage();
        self.sys.refresh_memory();
        let mut reading = Reading {
            system_cpu_pct: self.sys.global_cpu_usage(),
            available_mem_mb: available_mem_mb(&self.sys),
            swap_used_mb: self.sys.used_swap() / (1024 * 1024),
            ..Default::default()
        };
        if let Some(pid) = self.pid {
            self.sys.refresh_processes_specifics(
                ProcessesToUpdate::Some(&[pid]),
                false,
                ProcessRefreshKind::nothing().with_cpu().with_memory(),
            );
            if let Some(p) = self.sys.process(pid) {
                reading.app_cpu_pct = p.cpu_usage();
                reading.app_rss_mb = p.memory() / (1024 * 1024);
            }
        }
        reading
    }
}

#[derive(Clone, Copy, Default, Serialize)]
struct Usage {
    samples: u32,
    app_cpu_avg_pct: f32,
    app_cpu_peak_pct: f32,
    app_rss_start_mb: u64,
    app_rss_peak_mb: u64,
    app_rss_end_mb: u64,
    system_cpu_avg_pct: f32,
    system_cpu_peak_pct: f32,
    available_mem_min_mb: u64,
    swap_used_peak_mb: u64,
}

/// Samples resource usage on its own thread from `start` until `stop`.
struct Sampler {
    stop: Arc<AtomicBool>,
    handle: JoinHandle<Usage>,
}

impl Sampler {
    fn start() -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        let stop_flag = Arc::clone(&stop);
        let handle = thread::spawn(move || {
            let mut probe = Probe::new();
            let first = probe.read();
            let mut u = Usage {
                app_rss_start_mb: first.app_rss_mb,
                available_mem_min_mb: first.available_mem_mb,
                ..Default::default()
            };
            let (mut app_cpu_sum, mut sys_cpu_sum) = (0f32, 0f32);
            loop {
                thread::sleep(SAMPLE_INTERVAL);
                let r = probe.read();
                u.samples += 1;
                app_cpu_sum += r.app_cpu_pct;
                sys_cpu_sum += r.system_cpu_pct;
                u.app_cpu_peak_pct = u.app_cpu_peak_pct.max(r.app_cpu_pct);
                u.system_cpu_peak_pct = u.system_cpu_peak_pct.max(r.system_cpu_pct);
                u.app_rss_peak_mb = u.app_rss_peak_mb.max(r.app_rss_mb);
                u.app_rss_end_mb = r.app_rss_mb;
                u.available_mem_min_mb = u.available_mem_min_mb.min(r.available_mem_mb);
                u.swap_used_peak_mb = u.swap_used_peak_mb.max(r.swap_used_mb);
                if stop_flag.load(Ordering::Relaxed) {
                    break;
                }
            }
            u.app_cpu_avg_pct = (app_cpu_sum / u.samples as f32).round();
            u.system_cpu_avg_pct = (sys_cpu_sum / u.samples as f32).round();
            u.app_cpu_peak_pct = u.app_cpu_peak_pct.round();
            u.system_cpu_peak_pct = u.system_cpu_peak_pct.round();
            u
        });
        Sampler { stop, handle }
    }

    /// Ends sampling after the current sample without waiting for it.
    fn request_stop(&self) {
        self.stop.store(true, Ordering::Relaxed);
    }

    fn stop(self) -> Usage {
        self.request_stop();
        self.handle.join().unwrap_or_default()
    }
}

/// Machine state that commonly explains a slow run: heat, power saving, overall load.
#[derive(Serialize)]
struct SystemContext {
    /// "nominal" | "fair" | "serious" | "critical" (macOS); null elsewhere.
    thermal_state: Option<&'static str>,
    low_power_mode: Option<bool>,
    load_avg_1m: f64,
    cpu_cores: usize,
    total_mem_mb: u64,
}

impl SystemContext {
    fn collect() -> Self {
        let mut sys = System::new();
        sys.refresh_memory();
        let (thermal_state, low_power_mode) = mac_power_state();
        SystemContext {
            thermal_state,
            low_power_mode,
            load_avg_1m: (System::load_average().one * 100.0).round() / 100.0,
            cpu_cores: thread::available_parallelism().map(|n| n.get()).unwrap_or(0),
            total_mem_mb: sys.total_memory() / (1024 * 1024),
        }
    }
}

/// Memory the system can still hand out, in MB. On macOS sysinfo's figure subtracts compressed
/// memory from free + inactive and reads 0 whenever the compressor is busy, so this uses the kernel's
/// memory-pressure level instead (the percentage Activity Monitor's pressure graph is based on).
#[cfg(target_os = "macos")]
pub(crate) fn available_mem_mb(sys: &System) -> u64 {
    extern "C" {
        fn sysctlbyname(name: *const u8, oldp: *mut std::ffi::c_void, oldlenp: *mut usize, newp: *mut std::ffi::c_void, newlen: usize) -> i32;
    }
    let mut level: i32 = 0;
    let mut len = std::mem::size_of::<i32>();
    let ok = unsafe {
        sysctlbyname(b"kern.memorystatus_level\0".as_ptr(), &mut level as *mut i32 as *mut _, &mut len, std::ptr::null_mut(), 0)
    } == 0;
    if ok && (0..=100).contains(&level) {
        sys.total_memory() / (1024 * 1024) * level as u64 / 100
    } else {
        sys.available_memory() / (1024 * 1024)
    }
}

#[cfg(not(target_os = "macos"))]
pub(crate) fn available_mem_mb(sys: &System) -> u64 {
    sys.available_memory() / (1024 * 1024)
}

#[cfg(target_os = "macos")]
fn mac_power_state() -> (Option<&'static str>, Option<bool>) {
    use std::ffi::c_void;
    #[link(name = "Foundation", kind = "framework")]
    extern "C" {
        fn objc_msgSend(receiver: *mut c_void, sel: *const c_void, ...) -> *mut c_void;
        fn objc_getClass(name: *const u8) -> *mut c_void;
        fn sel_registerName(name: *const u8) -> *const c_void;
    }
    unsafe {
        // Called through non-variadic signatures (see apply_pill_window_behavior).
        let get_obj: extern "C" fn(*mut c_void, *const c_void) -> *mut c_void =
            std::mem::transmute(objc_msgSend as *const ());
        let get_isize: extern "C" fn(*mut c_void, *const c_void) -> isize = std::mem::transmute(objc_msgSend as *const ());
        let get_bool: extern "C" fn(*mut c_void, *const c_void) -> bool = std::mem::transmute(objc_msgSend as *const ());

        let class = objc_getClass(b"NSProcessInfo\0".as_ptr());
        if class.is_null() {
            return (None, None);
        }
        let info = get_obj(class, sel_registerName(b"processInfo\0".as_ptr()));
        if info.is_null() {
            return (None, None);
        }
        let thermal = match get_isize(info, sel_registerName(b"thermalState\0".as_ptr())) {
            0 => "nominal",
            1 => "fair",
            2 => "serious",
            _ => "critical",
        };
        // isLowPowerModeEnabled is macOS 12+; the Intel build still runs on macOS 11.
        let responds: extern "C" fn(*mut c_void, *const c_void, *const c_void) -> bool =
            std::mem::transmute(objc_msgSend as *const ());
        let low_power_sel = sel_registerName(b"isLowPowerModeEnabled\0".as_ptr());
        let low_power = responds(info, sel_registerName(b"respondsToSelector:\0".as_ptr()), low_power_sel)
            .then(|| get_bool(info, low_power_sel));
        (Some(thermal), low_power)
    }
}

#[cfg(not(target_os = "macos"))]
fn mac_power_state() -> (Option<&'static str>, Option<bool>) {
    (None, None)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sampler_collects_usage() {
        let sampler = Sampler::start();
        // Keep a core busy so CPU readings are non-zero.
        let until = Instant::now() + Duration::from_millis(600);
        let mut x = 0u64;
        while Instant::now() < until {
            x = x.wrapping_mul(31).wrapping_add(7);
        }
        std::hint::black_box(x);
        let usage = sampler.stop();
        assert!(usage.samples >= 2, "samples: {}", usage.samples);
        assert!(usage.app_rss_peak_mb > 0);
        assert!(usage.app_cpu_peak_pct > 0.0);
        assert!(usage.available_mem_min_mb > 0);
    }

    #[test]
    fn early_outcome_files_unfinished_time_under_the_next_stage() {
        let mut run = PerfRun::start(100);
        run.lap("audio_prep");
        thread::sleep(Duration::from_millis(30));
        run.set_outcome("cancelled");
        thread::sleep(Duration::from_millis(200));
        let (stage, ms) = *run.stages.last().unwrap();
        assert_eq!(stage, "vad");
        assert!((30..200).contains(&ms), "vad: {} ms", ms);
        let total = run.ended.unwrap().duration_since(run.started).as_millis();
        assert!(total < 200, "total: {} ms", total);
        run.sampler.take().map(Sampler::stop);
        // Dropping would append this test run to the real performance log.
        std::mem::forget(run);
    }

    #[test]
    fn system_context_reads_power_state() {
        let ctx = SystemContext::collect();
        assert!(ctx.cpu_cores > 0);
        assert!(ctx.total_mem_mb > 0);
        #[cfg(target_os = "macos")]
        assert!(ctx.thermal_state.is_some());
        println!("{}", serde_json::to_string(&ctx).unwrap());
    }
}
