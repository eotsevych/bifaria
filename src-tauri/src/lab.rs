use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter};

use crate::app_controller::AssistantPhase;
use crate::parakeet::ParakeetTranscriber;
use crate::settings::get_data_dir;
use crate::transcribe::{DiagnosticSegment, Transcriber};
use crate::vad::trim_silence;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LabAudioItem {
    pub id: String,
    pub name: String,
    pub filename: String,
    pub duration_sec: f32,
    pub size_bytes: u64,
    pub is_vad_trimmed: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LabExperimentRequest {
    pub audio_filename: String,
    pub model_name: String,
    pub use_vad: bool,
    pub language: Option<String>,
    #[serde(default = "lab_default_true")]
    pub text_normalization: bool,
    #[serde(default = "lab_default_true")]
    pub remove_filler_words: bool,
    #[serde(default = "lab_default_true")]
    pub convert_numbers: bool,
    #[serde(default = "lab_default_true")]
    pub remove_stutters: bool,
    #[serde(default = "lab_default_true")]
    pub apply_self_corrections: bool,
    #[serde(default = "lab_default_true")]
    pub remove_noise_markers: bool,
    #[serde(default = "lab_default_true")]
    pub collapse_redundancy: bool,
    #[serde(default = "lab_default_true")]
    pub annotate_ambiguity: bool,
    #[serde(default = "lab_default_true")]
    pub normalize_structured_values: bool,
    /// "off" | "light" | "balanced" | "strong", applied to the file before the models.
    #[serde(default)]
    pub noise_reduction: Option<String>,
    /// Transcribe long audio in chunks cut at pauses, as dictation does with chunking on.
    #[serde(default)]
    pub chunking: bool,
}

fn lab_default_true() -> bool { true }

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LabExperimentResult {
    pub clean_text: String,
    pub raw_text: String,
    pub raw_output: String,
    pub detected_lang: String,
    pub segments: Vec<DiagnosticSegment>,
    pub original_duration_sec: f32,
    pub processed_duration_sec: f32,
    pub vad_cut_sec: f32,
    pub vad_time_ms: u64,
    pub inference_time_ms: u64,
    pub speed_factor: f32,
    pub hardware_engine: String,
    pub original_audio_filename: String,
    pub vad_audio_filename: Option<String>,
    pub post_applied: Vec<String>,
    pub uncertain_spans: Vec<String>,
    pub requires_clarification: bool,
}

/// Reads WAV file samples into 16kHz float32.
pub fn read_wav_file(path: &Path) -> Result<(Vec<f32>, u32), String> {
    let mut reader = hound::WavReader::open(path)
        .map_err(|e| format!("Failed to open WAV: {}", e))?;
    let spec = reader.spec();
    let sample_rate = spec.sample_rate;

    let samples: Vec<f32> = match spec.sample_format {
        hound::SampleFormat::Float => reader.samples::<f32>().filter_map(|s| s.ok()).collect(),
        hound::SampleFormat::Int => {
            let max_val = (1i64 << (spec.bits_per_sample.saturating_sub(1))) as f32;
            reader
                .samples::<i32>()
                .filter_map(|s| s.ok())
                .map(|s| s as f32 / max_val)
                .collect()
        }
    };

    Ok((samples, sample_rate))
}

/// Decodes an audio file (WAV, MP3, M4A/MP4, FLAC, Ogg Vorbis, AIFF) into mono float32 samples
/// at the file's own rate; channels are averaged.
pub fn decode_audio_file(path: &Path) -> Result<(Vec<f32>, u32), String> {
    use symphonia::core::audio::SampleBuffer;
    use symphonia::core::codecs::{DecoderOptions, CODEC_TYPE_NULL};
    use symphonia::core::errors::Error;
    use symphonia::core::formats::FormatOptions;
    use symphonia::core::io::MediaSourceStream;
    use symphonia::core::meta::MetadataOptions;
    use symphonia::core::probe::Hint;

    let file = fs::File::open(path).map_err(|e| format!("Couldn't open {:?}: {}", path, e))?;
    let mut hint = Hint::new();
    if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
        hint.with_extension(ext);
    }
    let probed = symphonia::default::get_probe()
        .format(&hint, MediaSourceStream::new(Box::new(file), Default::default()), &FormatOptions::default(), &MetadataOptions::default())
        .map_err(|e| format!("Unsupported audio file (use WAV, MP3, M4A, FLAC, Ogg Vorbis or AIFF): {}", e))?;
    let mut format = probed.format;
    // A video file also has a video track; take the first one that is audio.
    let track = format
        .tracks()
        .iter()
        .find(|t| t.codec_params.codec != CODEC_TYPE_NULL && t.codec_params.sample_rate.is_some())
        .ok_or("The file has no audio track")?;
    let (track_id, rate) = (track.id, track.codec_params.sample_rate.unwrap_or(16000));
    let mut decoder = symphonia::default::get_codecs()
        .make(&track.codec_params, &DecoderOptions::default())
        .map_err(|e| format!("Can't decode this file's audio (Opus isn't supported): {}", e))?;

    let mut samples = Vec::new();
    loop {
        let packet = match format.next_packet() {
            Ok(packet) => packet,
            Err(Error::IoError(e)) if e.kind() == std::io::ErrorKind::UnexpectedEof => break,
            Err(Error::ResetRequired) => break,
            Err(e) => return Err(format!("Couldn't read the audio file: {}", e)),
        };
        if packet.track_id() != track_id {
            continue;
        }
        let decoded = match decoder.decode(&packet) {
            Ok(decoded) => decoded,
            // A damaged packet is skipped, as players do.
            Err(Error::DecodeError(e)) => {
                log::warn!("Skipped an undecodable packet in {:?}: {}", path, e);
                continue;
            }
            Err(e) => return Err(format!("Couldn't decode the audio file: {}", e)),
        };
        let spec = *decoded.spec();
        let channels = spec.channels.count().max(1);
        let mut buf = SampleBuffer::<f32>::new(decoded.capacity() as u64, spec);
        buf.copy_interleaved_ref(decoded);
        samples.extend(buf.samples().chunks(channels).map(|frame| frame.iter().sum::<f32>() / channels as f32));
    }
    Ok((samples, rate))
}

/// Length of an audio file in seconds from its header, without decoding it; `None` when the
/// header doesn't say.
fn audio_duration_sec(path: &Path) -> Option<f32> {
    use symphonia::core::io::MediaSourceStream;
    let file = fs::File::open(path).ok()?;
    let mut hint = symphonia::core::probe::Hint::new();
    if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
        hint.with_extension(ext);
    }
    let probed = symphonia::default::get_probe()
        .format(&hint, MediaSourceStream::new(Box::new(file), Default::default()), &Default::default(), &Default::default())
        .ok()?;
    let params = &probed.format.default_track()?.codec_params;
    Some(params.n_frames? as f32 / params.sample_rate? as f32)
}

/// Lists all audio files available in the app for testing.
pub fn list_available_audio_files() -> Vec<LabAudioItem> {
    let data_dir = get_data_dir();
    let mut list = Vec::new();

    // 1. Check latest_recording.wav (Original Mic Audio)
    let orig_path = data_dir.join("latest_recording.wav");
    if orig_path.exists() {
        if let Ok((samples, rate)) = read_wav_file(&orig_path) {
            let size = orig_path.metadata().map(|m| m.len()).unwrap_or(0);
            list.push(LabAudioItem {
                id: "latest_original".to_string(),
                name: "Latest Mic Recording (Uncut Original)".to_string(),
                filename: "latest_recording.wav".to_string(),
                duration_sec: samples.len() as f32 / rate as f32,
                size_bytes: size,
                is_vad_trimmed: false,
            });
        }
    }

    // 2. Check latest_vad_trimmed.wav (VAD Trimmed Audio)
    let vad_path = data_dir.join("latest_vad_trimmed.wav");
    if vad_path.exists() {
        if let Ok((samples, rate)) = read_wav_file(&vad_path) {
            let size = vad_path.metadata().map(|m| m.len()).unwrap_or(0);
            list.push(LabAudioItem {
                id: "latest_vad".to_string(),
                name: "Latest VAD Trimmed Audio".to_string(),
                filename: "latest_vad_trimmed.wav".to_string(),
                duration_sec: samples.len() as f32 / rate as f32,
                size_bytes: size,
                is_vad_trimmed: true,
            });
        }
    }

    // 3. The file last opened in the Audio Lab (`lab_custom.<ext>`)
    if let Ok(entries) = fs::read_dir(&data_dir) {
        for p in entries.flatten().map(|e| e.path()) {
            if p.file_stem().and_then(|s| s.to_str()) != Some("lab_custom") {
                continue;
            }
            if let Some(fname) = p.file_name().and_then(|s| s.to_str()) {
                list.push(LabAudioItem {
                    id: fname.to_string(),
                    name: format!("Opened File ({})", fname),
                    filename: fname.to_string(),
                    duration_sec: audio_duration_sec(&p).unwrap_or(0.0),
                    size_bytes: p.metadata().map(|m| m.len()).unwrap_or(0),
                    is_vad_trimmed: false,
                });
            }
        }
    }

    // 4. Whole recordings of this launch's logged sessions (Second Try targets these)
    let session_dir = data_dir.join("session_audio");
    if let Ok(entries) = fs::read_dir(&session_dir) {
        let mut entries_vec: Vec<PathBuf> = entries
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.extension().and_then(|s| s.to_str()) == Some("wav"))
            .collect();
        entries_vec.sort_by(|a, b| b.cmp(a)); // Newest first

        for p in entries_vec.into_iter().take(50) {
            if let Some(fname) = p.file_name().and_then(|s| s.to_str()) {
                if let Ok((samples, rate)) = read_wav_file(&p) {
                    let size = p.metadata().map(|m| m.len()).unwrap_or(0);
                    let rel = format!("session_audio/{}", fname);
                    list.push(LabAudioItem {
                        id: rel.clone(),
                        name: format!("Session Recording ({})", fname),
                        filename: rel,
                        duration_sec: samples.len() as f32 / rate as f32,
                        size_bytes: size,
                        is_vad_trimmed: false,
                    });
                }
            }
        }
    }

    // 5. Check audio/ directory for saved historical recordings
    let audio_dir = data_dir.join("audio");
    if let Ok(entries) = fs::read_dir(&audio_dir) {
        let mut entries_vec: Vec<PathBuf> = entries
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.extension().and_then(|s| s.to_str()) == Some("wav"))
            .collect();
        entries_vec.sort_by(|a, b| b.cmp(a)); // Newest first

        for p in entries_vec.into_iter().take(10) {
            if let Some(fname) = p.file_name().and_then(|s| s.to_str()) {
                if let Ok((samples, rate)) = read_wav_file(&p) {
                    let size = p.metadata().map(|m| m.len()).unwrap_or(0);
                    list.push(LabAudioItem {
                        id: fname.to_string(),
                        name: format!("History Recording ({})", fname),
                        filename: fname.to_string(),
                        duration_sec: samples.len() as f32 / rate as f32,
                        size_bytes: size,
                        is_vad_trimmed: false,
                    });
                }
            }
        }
    }

    list
}

/// Logs a lab run (load + inference, then the model is kept or freed) and updates the tray's model status.
fn log_lab_run(app_handle: &AppHandle, model: &str, audio_sec: f32, total_ms: u64, inference_ms: u64, kept: bool) {
    crate::tray::update_tray_model_status(app_handle, kept);
    crate::pipeline_logger::log_stage_event(
        &get_data_dir(),
        "LAB_RUN",
        &format!(
            "{} on {:.2} s of audio: load {} ms + inference {} ms ({:.1}x), then {}; app memory {} MB",
            model,
            audio_sec,
            total_ms.saturating_sub(inference_ms),
            inference_ms,
            if inference_ms > 0 { audio_sec * 1000.0 / inference_ms as f32 } else { 0.0 },
            if kept { "kept loaded for dictation" } else { "freed from RAM" },
            crate::vitals::process_rss_mb()
        ),
    );
}

/// Tells Audio Lab what a run is doing: `stage`, and while transcribing the share done.
fn report(app_handle: &AppHandle, stage: &str, fraction: Option<f32>) {
    let _ = app_handle.emit("lab-progress", serde_json::json!({ "stage": stage, "fraction": fraction }));
}

/// Longest piece Audio Lab gives a model at once. It holds the model only for a piece, so a
/// dictation waits at most this long (see `yield_to_dictation`).
const LAB_PIECE_SEC: usize = 60;
/// Each cut can lose a word on the Neural Engine, which reads long audio with overlap itself, so
/// its pieces are longer; it is fast enough that one still takes only seconds.
const LAB_PIECE_SEC_NEURAL_ENGINE: usize = 600;

static TRANSCRIBING: AtomicBool = AtomicBool::new(false);

/// Whether Audio Lab is transcribing a file, for the pill's note during a dictation.
pub fn is_transcribing() -> bool {
    TRANSCRIBING.load(Ordering::Relaxed)
}

/// Marks Audio Lab as transcribing until dropped, whichever way the run ends.
struct Transcribing;

impl Transcribing {
    fn start() -> Self {
        TRANSCRIBING.store(true, Ordering::Relaxed);
        Transcribing
    }
}

impl Drop for Transcribing {
    fn drop(&mut self) {
        TRANSCRIBING.store(false, Ordering::Relaxed);
    }
}

/// Free memory left over after loading a second speech model, for the system, other apps and the
/// model's working memory while it runs (~0.6 GB for Parakeet on a 60 s piece).
const TWO_MODELS_MARGIN_MB: u64 = 1024;

/// Memory a speech model takes once loaded, in MB: about its size on disk (Parakeet's 670 MB took
/// ~670 MB in a user's log). An unknown one counts as 1 GB.
fn model_size_mb(model_name: &str) -> u64 {
    crate::transcribe::get_model_catalog()
        .into_iter()
        .find(|m| m.filename == model_name)
        .map_or(1024, |m| m.file_size_bytes / (1024 * 1024))
}

/// Whether `loading` fits in memory next to the speech model already loaded.
pub fn room_for_two_models(loading: &str) -> bool {
    crate::vitals::available_memory_mb() >= model_size_mb(loading) + TWO_MODELS_MARGIN_MB
}

/// Whether a dictation loading `loading` may keep Audio Lab's other model loaded, so Audio Lab
/// doesn't reload it afterwards. Otherwise, as always, one speech model at a time.
pub fn keep_both_models(loading: &str) -> bool {
    is_transcribing() && room_for_two_models(loading)
}

/// A dictation is being recorded, transcribed or translated.
fn dictation_active(phase: &Mutex<AssistantPhase>) -> bool {
    matches!(
        *phase.lock().unwrap_or_else(|e| e.into_inner()),
        AssistantPhase::Listening | AssistantPhase::Transcribing | AssistantPhase::Translating
    )
}

/// The user's own dictation goes first: waits, between pieces, until it is done.
fn yield_to_dictation(app_handle: &AppHandle, phase: &Mutex<AssistantPhase>, done: f32) {
    if !dictation_active(phase) {
        return;
    }
    report(app_handle, "Paused for your dictation", Some(done));
    let paused = Instant::now();
    while dictation_active(phase) {
        std::thread::sleep(std::time::Duration::from_millis(200));
    }
    crate::pipeline_logger::log_stage_event(
        &get_data_dir(),
        "LAB_PAUSE",
        &format!("Audio Lab paused {} ms for a dictation", paused.elapsed().as_millis()),
    );
    report(app_handle, "Transcribing", Some(done));
}

/// Each of `chunks` cut at pauses into pieces of at most `max_sec`, as ranges of the whole.
fn lab_pieces(samples: &[f32], chunks: &[std::ops::Range<usize>], max_sec: usize) -> Vec<std::ops::Range<usize>> {
    chunks
        .iter()
        .flat_map(|c| {
            crate::parakeet::cut_at_pauses(&samples[c.clone()], 16000, max_sec)
                .into_iter()
                .map(move |r| c.start + r.start..c.start + r.end)
        })
        .collect()
}

/// Executes a test experiment on the given audio file.
pub async fn run_experiment(
    app_handle: &AppHandle,
    transcriber: Arc<Mutex<Transcriber>>,
    parakeet_transcriber: Arc<Mutex<ParakeetTranscriber>>,
    dictation_phase: Arc<Mutex<AssistantPhase>>,
    keep_loaded: bool,
    req: LabExperimentRequest,
) -> Result<LabExperimentResult, String> {
    let data_dir = get_data_dir();

    // Resolve audio file path
    let audio_path = if req.audio_filename == "latest_recording.wav" || req.audio_filename == "latest" {
        data_dir.join("latest_recording.wav")
    } else if req.audio_filename == "latest_vad_trimmed.wav" {
        data_dir.join("latest_vad_trimmed.wav")
    } else {
        let p = data_dir.join("audio").join(&req.audio_filename);
        if p.exists() {
            p
        } else {
            data_dir.join(&req.audio_filename)
        }
    };

    if !audio_path.exists() {
        return Err(format!("Audio file not found: {:?}", audio_path));
    }

    report(app_handle, "Reading the file", None);
    let (file_samples, sample_rate) = decode_audio_file(&audio_path)?;
    let file_samples = match req.noise_reduction.as_deref().and_then(crate::denoise::strength_wet) {
        Some(wet) => {
            report(app_handle, "Removing noise", None);
            let clean = crate::denoise::denoise(&file_samples, sample_rate, wet);
            drop(file_samples);
            // Saved so the denoised version can be listened to.
            let _ = crate::history::write_wav_file(&data_dir.join("lab_denoised.wav"), &clean, sample_rate);
            clean
        }
        None => file_samples,
    };
    // History recordings are saved at the microphone's own rate; the models need 16 kHz.
    let raw_samples = crate::audio::resample_to_16k(&file_samples, sample_rate);
    // A long file at 48 kHz is ~1 GB; only the 16 kHz copy is used from here on.
    drop(file_samples);
    if raw_samples.is_empty() {
        return Err("Audio file is empty".to_string());
    }

    let original_duration_sec = raw_samples.len() as f32 / 16000.0;

    // Optional VAD stage
    let (processed_samples, chunks, vad_time_ms, processed_duration_sec, vad_cut_sec, vad_audio_filename) = if req.use_vad {
        report(app_handle, "Finding speech", None);
        let vad_t0 = Instant::now();
        let vad_res = trim_silence(&raw_samples, 16000);
        drop(raw_samples);
        let elapsed = vad_t0.elapsed().as_millis() as u64;

        // Save trimmed audio for playback
        let trimmed_path = data_dir.join("lab_vad_trimmed.wav");
        let _ = crate::history::write_wav_file(&trimmed_path, &vad_res.samples, 16000);

        let chunks = vad_res.ranges(req.chunking);
        (
            vad_res.samples,
            chunks,
            elapsed,
            vad_res.trimmed_duration_sec,
            vad_res.silence_removed_sec,
            Some("lab_vad_trimmed.wav".to_string()),
        )
    } else {
        let chunks = if req.chunking {
            crate::vad::plan_chunks(&raw_samples).into_iter().map(|c| c.range).collect()
        } else {
            vec![0..raw_samples.len()]
        };
        (
            raw_samples,
            chunks,
            0,
            original_duration_sec,
            0.0,
            None,
        )
    };

    let is_parakeet = req.model_name == crate::model_download::PARAKEET_MODEL;

    if is_parakeet {
        // One model at a time, unless there's room to keep the other (the dictation's) loaded too.
        if !room_for_two_models(&req.model_name) {
            transcriber.lock().unwrap().unload();
        }
        report(app_handle, "Loading the model", None);
        let model_dir = ParakeetTranscriber::ensure_model(app_handle).await?;
        let t0 = Instant::now();
        let max_sec = if ParakeetTranscriber::runs_on_neural_engine() { LAB_PIECE_SEC_NEURAL_ENGINE } else { LAB_PIECE_SEC };
        let pieces = lab_pieces(&processed_samples, &chunks, max_sec);
        let (app, pk_arc) = (app_handle.clone(), Arc::clone(&parakeet_transcriber));
        let res = tauri::async_runtime::spawn_blocking(move || {
            let _transcribing = Transcribing::start();
            let total = pieces.iter().map(|p| p.len()).sum::<usize>().max(1);
            let (mut texts, mut inference_ms, mut done) = (Vec::new(), 0, 0);
            report(&app, "Transcribing", Some(0.0));
            let res = (|| {
                for piece in &pieces {
                    yield_to_dictation(&app, &dictation_phase, done as f32 / total as f32);
                    let audio = &processed_samples[piece.clone()];
                    let (text, _, ms, _) = pk_arc.lock().unwrap().transcribe(audio, &[0..audio.len()], &model_dir)?;
                    texts.push(text);
                    inference_ms += ms;
                    done += piece.len();
                    report(&app, "Transcribing", Some(done as f32 / total as f32));
                }
                Ok::<_, String>((crate::chunking::join_transcripts(&texts), "auto".to_string(), inference_ms))
            })();
            // Freed now unless it's the dictation model, which the idle cleanup frees later.
            if !keep_loaded {
                pk_arc.lock().unwrap().unload();
            }
            res
        })
        .await
        .map_err(|e| e.to_string())?;
        let total_ms = t0.elapsed().as_millis() as u64;
        let res: (String, String, u64) = res?;
        // Inference alone; the rest of the run was loading the model.
        let inference_ms = if res.2 > 0 { res.2 } else { total_ms };
        log_lab_run(app_handle, &req.model_name, processed_duration_sec, total_ms, inference_ms, keep_loaded);

        let speed_factor = if inference_ms > 0 {
            (processed_duration_sec * 1000.0) / (inference_ms as f32)
        } else {
            0.0
        };

        let raw_output = if res.0.is_empty() {
            "Parakeet TDT returned 0 tokens (empty output)".to_string()
        } else {
            format!("Parakeet TDT Output: \"{}\"", res.0)
        };

        let raw_text = res.0.clone();
        let post = crate::post_processor::post_process(&raw_text, crate::post_processor::PostProcessorConfig {
            normalization: crate::text_normalizer::NormalizationOptions {
                enabled: req.text_normalization,
                remove_fillers: req.remove_filler_words,
                convert_numbers: req.convert_numbers,
                remove_stutters: req.remove_stutters,
            },
            apply_self_corrections: req.apply_self_corrections,
            remove_noise_markers: req.remove_noise_markers,
            collapse_redundancy: req.collapse_redundancy,
            annotate_ambiguity: req.annotate_ambiguity,
            normalize_structured_values: req.normalize_structured_values,
        });
        let final_clean = if !post.clean_text.is_empty() { post.clean_text.clone() } else { raw_text.clone() };
        let post_applied = {
            let mut v = Vec::new();
            if req.text_normalization { v.push("normalization".to_string()); }
            if req.apply_self_corrections { v.push("self_corrections".to_string()); }
            if req.remove_noise_markers { v.push("noise_markers".to_string()); }
            if req.collapse_redundancy { v.push("redundancy".to_string()); }
            if req.annotate_ambiguity { v.push("ambiguity".to_string()); }
            if req.normalize_structured_values { v.push("structured".to_string()); }
            v
        };

        Ok(LabExperimentResult {
            clean_text: final_clean,
            raw_text: raw_text.clone(),
            raw_output,
            detected_lang: res.1,
            segments: vec![DiagnosticSegment {
                start_ms: 0,
                end_ms: (processed_duration_sec * 1000.0) as i64,
                text: raw_text,
                no_speech_prob: 0.0,
            }],
            original_duration_sec,
            processed_duration_sec,
            vad_cut_sec,
            vad_time_ms,
            inference_time_ms: inference_ms,
            speed_factor,
            hardware_engine: "ONNX Runtime (CPU FastConformer)".to_string(),
            original_audio_filename: req.audio_filename.clone(),
            vad_audio_filename,
            post_applied,
            uncertain_spans: post.uncertain_spans.clone(),
            requires_clarification: post.requires_clarification,
        })
    } else {
        // One model at a time, unless there's room to keep the other (the dictation's) loaded too.
        if !room_for_two_models(&req.model_name) {
            parakeet_transcriber.lock().unwrap().unload();
        }
        report(app_handle, "Loading the model", None);
        let model_path = Transcriber::ensure_model(app_handle, &req.model_name).await?;
        let t0 = Instant::now();
        let pieces = lab_pieces(&processed_samples, &chunks, LAB_PIECE_SEC);
        let (app, tr_arc, language) = (app_handle.clone(), Arc::clone(&transcriber), req.language.clone());
        let res = tauri::async_runtime::spawn_blocking(move || {
            let _transcribing = Transcribing::start();
            let total = pieces.iter().map(|p| p.len()).sum::<usize>().max(1);
            let mut all = crate::transcribe::DetailedWhisperResult {
                text: String::new(),
                detected_lang: "auto".to_string(),
                inference_ms: 0,
                thread_count: 0,
                raw_output: String::new(),
                segments: Vec::new(),
            };
            let (mut texts, mut raw, mut lang, mut done) = (Vec::new(), Vec::new(), language, 0);
            report(&app, "Transcribing", Some(0.0));
            let res = (|| {
                for (i, piece) in pieces.iter().enumerate() {
                    yield_to_dictation(&app, &dictation_phase, done as f32 / total as f32);
                    let audio = &processed_samples[piece.clone()];
                    let r = tr_arc.lock().unwrap().transcribe_detailed(audio, &[0..audio.len()], &model_path, false, lang.as_deref())?;
                    // Segment times are within the piece; shifted to times in the whole file.
                    let offset_ms = (piece.start * 1000 / 16000) as i64;
                    all.segments.extend(r.segments.into_iter().map(|mut s| {
                        s.start_ms += offset_ms;
                        s.end_ms += offset_ms;
                        s
                    }));
                    if i == 0 {
                        all.detected_lang = r.detected_lang.clone();
                        // Later pieces keep this language, so a long file can't switch midway.
                        if lang.as_deref().map_or(true, |l| l.trim().is_empty() || l == "auto") && r.detected_lang != "auto" {
                            lang = Some(r.detected_lang.clone());
                        }
                    }
                    texts.push(r.text);
                    raw.push(r.raw_output);
                    all.inference_ms += r.inference_ms;
                    all.thread_count = r.thread_count;
                    done += piece.len();
                    report(&app, "Transcribing", Some(done as f32 / total as f32));
                }
                all.text = crate::chunking::join_transcripts(&texts);
                all.raw_output = raw.join("\n");
                Ok::<_, String>(all)
            })();
            // Freed now unless it's the dictation model, which the idle cleanup frees later.
            if !keep_loaded {
                tr_arc.lock().unwrap().unload();
            }
            res
        })
        .await
        .map_err(|e| e.to_string())?;
        let total_ms = t0.elapsed().as_millis() as u64;
        let res = res?;
        log_lab_run(app_handle, &req.model_name, processed_duration_sec, total_ms, res.inference_ms, keep_loaded);

        let speed_factor = if res.inference_ms > 0 {
            (processed_duration_sec * 1000.0) / (res.inference_ms as f32)
        } else {
            0.0
        };

        let hardware_engine = match crate::transcribe::gpu_devices().first() {
            Some(gpu) if crate::settings::AppSettings::load().device_backend != "cpu" => format!("GPU: {}", gpu),
            _ => "CPU".to_string(),
        };

        let raw_text = res.text.clone();
        let post = crate::post_processor::post_process(&raw_text, crate::post_processor::PostProcessorConfig {
            normalization: crate::text_normalizer::NormalizationOptions {
                enabled: req.text_normalization,
                remove_fillers: req.remove_filler_words,
                convert_numbers: req.convert_numbers,
                remove_stutters: req.remove_stutters,
            },
            apply_self_corrections: req.apply_self_corrections,
            remove_noise_markers: req.remove_noise_markers,
            collapse_redundancy: req.collapse_redundancy,
            annotate_ambiguity: req.annotate_ambiguity,
            normalize_structured_values: req.normalize_structured_values,
        });
        let final_clean = if !post.clean_text.is_empty() { post.clean_text.clone() } else { raw_text.clone() };
        let post_applied = {
            let mut v = Vec::new();
            if req.text_normalization { v.push("normalization".to_string()); }
            if req.apply_self_corrections { v.push("self_corrections".to_string()); }
            if req.remove_noise_markers { v.push("noise_markers".to_string()); }
            if req.collapse_redundancy { v.push("redundancy".to_string()); }
            if req.annotate_ambiguity { v.push("ambiguity".to_string()); }
            if req.normalize_structured_values { v.push("structured".to_string()); }
            v
        };

        Ok(LabExperimentResult {
            clean_text: final_clean,
            raw_text: raw_text.clone(),
            raw_output: res.raw_output.clone(),
            detected_lang: res.detected_lang.clone(),
            segments: res.segments.clone(),
            original_duration_sec,
            processed_duration_sec,
            vad_cut_sec,
            vad_time_ms,
            inference_time_ms: res.inference_ms,
            speed_factor,
            hardware_engine,
            original_audio_filename: req.audio_filename.clone(),
            vad_audio_filename,
            post_applied,
            uncertain_spans: post.uncertain_spans.clone(),
            requires_clarification: post.requires_clarification,
        })
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn lab_pieces_cover_each_chunk_in_order_within_the_limit() {
        // A 10 s chunk and a 140 s chunk, cut into pieces of at most 60 s.
        let samples: Vec<f32> = (0..150 * 16000).map(|i| (i as f32 * 0.01).sin() * 0.5).collect();
        let chunks = vec![0..10 * 16000, 10 * 16000..samples.len()];
        let pieces = super::lab_pieces(&samples, &chunks, 60);
        assert_eq!(pieces.first().unwrap().start, 0);
        assert_eq!(pieces.last().unwrap().end, samples.len());
        for pair in pieces.windows(2) {
            assert_eq!(pair[0].end, pair[1].start, "no gaps or overlaps");
        }
        assert!(pieces.iter().all(|p| p.len() <= 60 * 16000));
        // The chunk boundary stays a piece boundary; 140 s needs three pieces.
        assert_eq!(pieces[0], 0..10 * 16000);
        assert_eq!(pieces.len(), 4);
    }

    #[test]
    fn decodes_a_stereo_wav_to_mono_at_its_own_rate() {
        let path = std::env::temp_dir().join(format!("bifaria_lab_stereo_{}.wav", std::process::id()));
        let spec = hound::WavSpec { channels: 2, sample_rate: 44100, bits_per_sample: 16, sample_format: hound::SampleFormat::Int };
        let mut w = hound::WavWriter::create(&path, spec).unwrap();
        for _ in 0..4410 {
            w.write_sample(i16::MAX / 2).unwrap(); // left
            w.write_sample(0i16).unwrap(); // right
        }
        w.finalize().unwrap();
        let (samples, rate) = super::decode_audio_file(&path).unwrap();
        let _ = std::fs::remove_file(&path);
        assert_eq!(rate, 44100);
        assert_eq!(samples.len(), 4410, "one sample per frame, not per channel");
        assert!(samples.iter().all(|s| (s - 0.25).abs() < 0.01), "channels averaged: {}", samples[0]);
    }

    #[test]
    fn a_file_that_isnt_audio_is_refused_with_the_formats_that_work() {
        let path = std::env::temp_dir().join(format!("bifaria_lab_not_audio_{}.mp3", std::process::id()));
        std::fs::write(&path, b"definitely not audio").unwrap();
        let err = super::decode_audio_file(&path).unwrap_err();
        let _ = std::fs::remove_file(&path);
        assert!(err.contains("MP3"), "{err}");
    }

    /// Decodes a real file: `BIFARIA_AUDIO=path cargo test --lib real_audio -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn real_audio_file_decodes() {
        let path = std::env::var("BIFARIA_AUDIO").expect("set BIFARIA_AUDIO to an audio file");
        let (samples, rate) = super::decode_audio_file(std::path::Path::new(&path)).unwrap();
        println!("{} Hz, {:.2} s (header: {:?} s)", rate, samples.len() as f32 / rate as f32, super::audio_duration_sec(std::path::Path::new(&path)));
        assert!(!samples.is_empty());
    }

    /// A real file through decoding, chunking and Parakeet, chunked and whole:
    /// `BIFARIA_AUDIO=path cargo test --release --lib chunked_file -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn chunked_file_transcribes_like_the_whole_file() {
        let path = std::env::var("BIFARIA_AUDIO").expect("set BIFARIA_AUDIO to an audio file");
        let (samples, rate) = super::decode_audio_file(std::path::Path::new(&path)).unwrap();
        let samples = crate::audio::resample_to_16k(&samples, rate);
        let chunks: Vec<_> = crate::vad::plan_chunks(&samples).into_iter().map(|c| c.range).collect();
        let dir = crate::parakeet::ParakeetTranscriber::get_model_dir();
        let mut pk = crate::parakeet::ParakeetTranscriber::new();
        let (chunked, _, chunked_ms, _) = pk.transcribe(&samples, &chunks, &dir).unwrap();
        let (whole, _, whole_ms, _) = pk.transcribe(&samples, &[0..samples.len()], &dir).unwrap();
        println!("{} chunks in {} ms; whole in {} ms", chunks.len(), chunked_ms, whole_ms);
        println!("CHUNKED ({} words): {}\n", chunked.split_whitespace().count(), chunked);
        println!("WHOLE ({} words): {}", whole.split_whitespace().count(), whole);
        assert!(chunks.len() > 1);
    }
}
