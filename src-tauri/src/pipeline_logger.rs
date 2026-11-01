use chrono::Local;
use serde::{Deserialize, Serialize};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChunkDiagnosticEvent {
    pub chunk_index: usize,
    pub trigger_reason: String,
    pub duration_sec: f32,
    pub has_overlap: bool,
    pub overlap_ms: u32,
    pub silence_detected_ms: u64,
    pub rms_energy: f32,
    pub timestamp: String,
    pub detail: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TranscriptionDiagnosticLog {
    pub id: u64,
    pub timestamp: String,
    pub audio_duration_sec: f32,
    pub audio_samples_count: usize,
    pub vad_trim_ms: u64,
    pub vad_original_sec: f32,
    pub vad_trimmed_sec: f32,
    pub vad_silence_removed_sec: f32,
    pub model_name: String,
    pub gpu_metal_active: bool,
    pub threads_count: i32,
    pub whisper_inference_ms: u64,
    pub whisper_speed_factor: f32,
    pub detected_lang: String,
    pub raw_text: String,
    pub translation_skipped: bool,
    pub translation_skip_reason: String,
    pub translation_ms: u64,
    /// Why translation failed (provider, model and error), when it did; the spoken text was used instead.
    #[serde(default)]
    pub translation_error: Option<String>,
    /// The service that translated, e.g. "Apple Translation (on-device) uk → en" or "LLM
    /// gemini-3.6-flash at …". None when translation was skipped.
    #[serde(default)]
    pub translation_provider: Option<String>,
    pub final_text: String,
    /// The text in the spoken language (after cleanup and masking), when `final_text` is its
    /// translation. Shown in Audio Lab's second try.
    #[serde(default)]
    pub spoken_text: Option<String>,
    pub clipboard_paste_ms: u64,
    pub history_save_ms: u64,
    pub total_pipeline_ms: u64,
    #[serde(default)]
    pub audio_filename: Option<String>,
    #[serde(default)]
    pub vad_audio_filename: Option<String>,
    #[serde(default)]
    pub whisper_raw_output: Option<String>,
    #[serde(default)]
    pub segments_count: usize,
    #[serde(default)]
    pub chunk_events: Vec<ChunkDiagnosticEvent>,
    /// True when chunking is off: the recording went to the model as one track.
    #[serde(default)]
    pub whole_track: bool,
    #[serde(default)]
    pub action_logs: Vec<String>,
    /// Row of this recording in the history database; none in private mode or when nothing was saved.
    #[serde(default)]
    pub history_id: Option<i64>,
    /// Why transcription failed, when it did. Audio Lab can retry it from the whole recording.
    #[serde(default)]
    pub transcription_error: Option<String>,
}

impl TranscriptionDiagnosticLog {
    pub fn format_ascii_report(&self) -> String {
        let metal_str = if self.gpu_metal_active {
            "Apple Silicon Metal GPU (Active)"
        } else {
            "CPU Only"
        };

        let trans_str = if self.translation_skipped {
            format!("0 ms (SKIPPED: {})", self.translation_skip_reason)
        } else {
            format!("{} ms ({})", self.translation_ms, self.translation_provider.as_deref().unwrap_or("translation service"))
        };

        format!(
            "\n┌──────────────────── BIFARIA TRANSCRIPTION REPORT ────────────────────┐\n\
             │ Run ID:       #{:<53} │\n\
             │ Timestamp:    {:<55} │\n\
             │ Total Time:   {:<55} │\n\
             ├───────────────────────────────────────────────────────────────────────┤\n\
             │ 1. Audio:     {:<55} │\n\
             │ 2. VAD Trim:  {:<55} │\n\
             │ 3. Whisper:   {:<55} │\n\
             │    - Model:   {:<55} │\n\
             │    - Accel:   {:<55} │\n\
             │    - Speed:   {:<55} │\n\
             │    - Lang:    {:<55} │\n\
             │    - Raw:     \"{}\"\n\
             │ 4. Translate: {:<55} │\n\
             │    - Final:   \"{}\"\n\
             │ 5. Paste:     {:<55} │\n\
             │ 6. DB Save:   {:<55} │\n\
             └───────────────────────────────────────────────────────────────────────┘\n",
            self.id,
            self.timestamp,
            format!("{} ms", self.total_pipeline_ms),
            format!("{:.2} s ({} samples @ 16 kHz)", self.audio_duration_sec, self.audio_samples_count),
            format!("{} ms (kept {:.2} s, cut {:.2} s silence)", self.vad_trim_ms, self.vad_trimmed_sec, self.vad_silence_removed_sec),
            format!("{} ms", self.whisper_inference_ms),
            self.model_name,
            metal_str,
            format!("{:.1}x real-time speed ({} threads)", self.whisper_speed_factor, self.threads_count),
            self.detected_lang,
            self.raw_text,
            trans_str,
            self.final_text,
            format!("{} ms (copied & Cmd+V dispatched)", self.clipboard_paste_ms),
            format!("{} ms (saved to local database)", self.history_save_ms),
        )
    }

    pub fn append_to_file(&self, data_dir: &Path) {
        append_log(
            data_dir,
            &format!(
                "[{}] id={} total_ms={} audio={:.2}s vad_ms={} vad_cut={:.2}s whisper_ms={} model={} speed={:.1}x lang={} trans_ms={} trans_via=\"{}\" text=\"{}\"\n",
                self.timestamp,
                self.id,
                self.total_pipeline_ms,
                self.audio_duration_sec,
                self.vad_trim_ms,
                self.vad_silence_removed_sec,
                self.whisper_inference_ms,
                self.model_name,
                self.whisper_speed_factor,
                self.detected_lang,
                self.translation_ms,
                self.translation_provider.as_deref().unwrap_or("-"),
                self.final_text.replace('\n', " ")
            ),
        );
    }
}

pub struct PipelineLogManager {
    logs: Arc<Mutex<Vec<TranscriptionDiagnosticLog>>>,
    counter: Arc<Mutex<u64>>,
}

impl PipelineLogManager {
    pub fn new() -> Self {
        // Logs live in memory only, so session recordings from an earlier launch belong to no log.
        let _ = fs::remove_dir_all(session_audio_dir(&crate::settings::get_data_dir()));
        Self {
            logs: Arc::new(Mutex::new(Vec::new())),
            counter: Arc::new(Mutex::new(1)),
        }
    }

    pub fn next_id(&self) -> u64 {
        let mut c = self.counter.lock().unwrap();
        let id = *c;
        *c += 1;
        id
    }

    pub fn add_log(&self, log: TranscriptionDiagnosticLog, data_dir: &Path) {
        // Print formatted ASCII report to terminal stdout
        print!("{}", log.format_ascii_report());

        // Append to persistent log file on disk
        log.append_to_file(data_dir);

        // Save in in-memory ring buffer (up to 50 recent runs)
        let mut list = self.logs.lock().unwrap();
        list.insert(0, log);
        if list.len() > 50 {
            for dropped in list.drain(50..) {
                remove_session_audio(data_dir, dropped.audio_filename.as_deref());
            }
        }
    }

    /// Changes the log entry with `id` and returns it as changed; `None` once it has left the log.
    pub fn update_log(
        &self,
        id: u64,
        change: impl FnOnce(&mut TranscriptionDiagnosticLog),
    ) -> Option<TranscriptionDiagnosticLog> {
        let mut list = self.logs.lock().unwrap();
        let log = list.iter_mut().find(|l| l.id == id)?;
        change(log);
        Some(log.clone())
    }

    pub fn get_logs(&self, limit: usize) -> Vec<TranscriptionDiagnosticLog> {
        let list = self.logs.lock().unwrap();
        list.iter().take(limit).cloned().collect()
    }

    pub fn clear_logs(&self) {
        let mut list = self.logs.lock().unwrap();
        list.clear();
        let _ = fs::remove_dir_all(session_audio_dir(&crate::settings::get_data_dir()));
    }
}

/// Folder with the whole recording of each logged session, for playback and Second Try.
const SESSION_AUDIO_DIR: &str = "session_audio";

fn session_audio_dir(data_dir: &Path) -> PathBuf {
    data_dir.join(SESSION_AUDIO_DIR)
}

/// Saves the entire recording of one session and returns its path relative to the data dir. The
/// file is written on a thread of its own (~150 ms for 3 minutes at 48 kHz), off the dictation's
/// path; it's only opened later, from the log or history.
pub fn save_session_audio(data_dir: &Path, audio: &crate::audio::CapturedAudio) -> Option<String> {
    let dir = session_audio_dir(data_dir);
    fs::create_dir_all(&dir).ok()?;
    let filename = format!("session_{}.wav", Local::now().format("%Y%m%d_%H%M%S_%3f"));
    let (samples, sample_rate) = audio.playback();
    let path = dir.join(&filename);
    std::thread::spawn(move || {
        if let Err(e) = crate::history::write_wav_file(&path, &samples, sample_rate) {
            log::warn!("Couldn't save the session audio {:?}: {}", path, e);
        }
    });
    Some(format!("{}/{}", SESSION_AUDIO_DIR, filename))
}

fn remove_session_audio(data_dir: &Path, relative: Option<&str>) {
    if let Some(rel) = relative.filter(|r| r.starts_with(SESSION_AUDIO_DIR)) {
        let _ = fs::remove_file(data_dir.join(rel));
    }
}

/// Diagnostics for a recording sent to the model as one whole track (chunking off).
pub fn whole_track_diagnostics(
    total_audio_sec: f32,
    vad_trimmed_sec: f32,
    silence_removed_sec: f32,
    raw_samples_count: usize,
) -> Vec<String> {
    vec![
        format!("[CAPTURE] Mic recording completed: {:.2}s ({} samples @ 16kHz)", total_audio_sec, raw_samples_count),
        format!("[VAD] Silence analysis: cut {:.2}s dead air, kept {:.2}s active voice", silence_removed_sec, vad_trimmed_sec),
        format!("[TRACK] Chunking off: the whole {:.2}s recording went to the model as one track", vad_trimmed_sec.max(0.0)),
    ]
}

pub fn current_timestamp() -> String {
    Local::now().format("%Y-%m-%d %H:%M:%S").to_string()
}

pub fn log_stage_event(data_dir: &Path, tag: &str, message: &str) {
    let line = format!("[{}] [{}] {}\n", current_timestamp(), tag, message);
    print!("{}", line);
    append_log(data_dir, &line);
}

const LOG_FILE: &str = "transcription_pipeline.log";
const OLD_LOG_FILE: &str = "transcription_pipeline.1.log";
const MAX_LOG_BYTES: u64 = 10 * 1024 * 1024;

/// "Don't save" storage mode: the pipeline log holds dictated text, so nothing goes to disk.
static PRIVATE: AtomicBool = AtomicBool::new(false);

pub fn set_private(private: bool) {
    PRIVATE.store(private, Ordering::SeqCst);
}

pub fn is_private() -> bool {
    PRIVATE.load(Ordering::SeqCst)
}

/// Appends to the pipeline log, keeping one older file once it reaches `MAX_LOG_BYTES`.
fn append_log(data_dir: &Path, text: &str) {
    if is_private() {
        return;
    }
    let logs_dir = data_dir.join("logs");
    let _ = fs::create_dir_all(&logs_dir);
    let log_file = logs_dir.join(LOG_FILE);
    if fs::metadata(&log_file).map(|m| m.len() >= MAX_LOG_BYTES).unwrap_or(false) {
        let _ = fs::rename(&log_file, logs_dir.join(OLD_LOG_FILE));
    }
    if let Ok(mut file) = OpenOptions::new().create(true).append(true).open(log_file) {
        let _ = file.write_all(text.as_bytes());
    }
}

/// Deletes the pipeline log files, which hold the text of every dictation.
pub fn delete_log_files(data_dir: &Path) {
    let logs_dir = data_dir.join("logs");
    let _ = fs::remove_file(logs_dir.join(LOG_FILE));
    let _ = fs::remove_file(logs_dir.join(OLD_LOG_FILE));
}

/// Diagnostics for a recording transcribed chunk by chunk (chunking on): the chunks `vad` planned.
pub fn chunk_diagnostics(
    total_audio_sec: f32,
    vad: &crate::vad::VadResult,
    raw_samples_count: usize,
) -> (Vec<ChunkDiagnosticEvent>, Vec<String>) {
    use crate::chunking::Ending;
    let mut chunks = Vec::new();
    let mut actions = vec![
        format!("[CAPTURE] Mic recording completed: {:.2}s ({} samples @ 16kHz)", total_audio_sec, raw_samples_count),
        format!("[VAD] Silence analysis: cut {:.2}s dead air, kept {:.2}s active voice", vad.silence_removed_sec, vad.trimmed_duration_sec),
    ];
    if vad.chunks.len() == 1 {
        actions.push(format!("[TRACK] {:.2}s is 40 s or less, so it went to the model in one piece", vad.trimmed_duration_sec));
    }
    let secs = |samples: usize| samples as f32 / 16000.0;
    for (i, chunk) in vad.chunks.iter().enumerate() {
        let audio = &vad.samples[chunk.range.clone()];
        let rms = (audio.iter().map(|x| x * x).sum::<f32>() / audio.len().max(1) as f32).sqrt();
        let (start, end) = (secs(chunk.range.start), secs(chunk.range.end));
        let (trigger_reason, silence_detected_ms, ends) = match chunk.ending {
            Ending::Pause { gap_ms } => ("Pause", gap_ms, format!("cut in the middle of a {} ms pause", gap_ms)),
            Ending::HardCut => ("Hard cut", 0, "cut without a pause: 40 s of speech with no usable pause".to_string()),
            Ending::End => ("End of recording", 0, "ends with the recording".to_string()),
        };
        let detail = format!("{:.2}s to {:.2}s, {}.", start, end, ends);
        actions.push(format!("[CHUNK #{}] {:.2}s ({})", i + 1, end - start, detail));
        chunks.push(ChunkDiagnosticEvent {
            chunk_index: i + 1,
            trigger_reason: trigger_reason.to_string(),
            duration_sec: end - start,
            has_overlap: false,
            overlap_ms: 0,
            silence_detected_ms,
            rms_energy: rms,
            timestamp: current_timestamp(),
            detail,
        });
    }
    (chunks, actions)
}
