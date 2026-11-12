use serde::{Deserialize, Serialize};
use std::fs;
use std::io::Write;
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;
use tauri::AppHandle;
use whisper_rs::{FullParams, SamplingStrategy, WhisperContext, WhisperContextParameters};

use crate::settings::get_data_dir;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelCatalogItem {
    pub id: String,
    pub name: String,
    pub size_desc: String,
    pub speed_desc: String,
    pub filename: String,
    pub downloaded: bool,
    pub file_size_bytes: u64,
}

pub const HF_BASE_URL: &str = "https://huggingface.co/ggerganov/whisper.cpp/resolve/main";

pub const AVAILABLE_MODELS: &[(&str, &str, &str, &str, &str)] = &[
    (
        "base",
        "Whisper Base",
        "142 MB",
        "Ultra Fast (~150-250 ms)",
        "ggml-base.bin",
    ),
    (
        "small",
        "Whisper Small (Q5_1)",
        "182 MB",
        "Balanced (~350-500 ms)",
        "ggml-small-q5_1.bin",
    ),
    (
        "medium",
        "Whisper Medium (Q5_0)",
        "514 MB",
        "High Accuracy (~1.2-2.0 s)",
        "ggml-medium-q5_0.bin",
    ),
    (
        "parakeet",
        "Parakeet TDT 0.6B v3",
        "670 MB",
        "Real-Time (~80-150 ms, 25 langs)",
        "parakeet-tdt-0.6b-v3",
    ),
];

pub fn get_models_dir() -> PathBuf {
    let dir = get_data_dir().join("models");
    if !dir.exists() {
        let _ = fs::create_dir_all(&dir);
    }
    dir
}

fn format_size(bytes: u64) -> String {
    if bytes >= 1024 * 1024 * 1024 {
        format!("{:.1} GB", bytes as f64 / (1024.0 * 1024.0 * 1024.0))
    } else if bytes >= 1024 * 1024 {
        format!("{:.0} MB", bytes as f64 / (1024.0 * 1024.0))
    } else if bytes >= 1024 {
        format!("{:.0} KB", bytes as f64 / 1024.0)
    } else {
        format!("{} B", bytes)
    }
}

pub fn get_model_catalog() -> Vec<ModelCatalogItem> {
    let mut items: Vec<ModelCatalogItem> = AVAILABLE_MODELS
        .iter()
        .map(|(id, name, size_desc, speed_desc, filename)| {
            let (downloaded, file_size_bytes) = if *id == "parakeet" {
                let ready = crate::parakeet::ParakeetTranscriber::model_ready();
                let sz = if ready {
                    crate::parakeet::ParakeetTranscriber::downloaded_size()
                } else {
                    0
                };
                (ready, sz)
            } else {
                let path = Transcriber::get_model_path(filename);
                let downloaded = path.exists() && path.metadata().map(|m| m.len() > 1_000_000).unwrap_or(false);
                let file_size_bytes = if downloaded {
                    path.metadata().map(|m| m.len()).unwrap_or(0)
                } else {
                    0
                };
                (downloaded, file_size_bytes)
            };

            // Parakeet's download is the Neural Engine build on Apple Silicon, smaller than the CPU one.
            let size_desc = if *id == "parakeet" {
                format_size(crate::parakeet::ParakeetTranscriber::download_size())
            } else {
                size_desc.to_string()
            };
            ModelCatalogItem {
                id: id.to_string(),
                name: name.to_string(),
                size_desc,
                speed_desc: speed_desc.to_string(),
                filename: filename.to_string(),
                downloaded,
                file_size_bytes,
            }
        })
        .collect();

    // Auto-scan models/ for custom .bin files not in the built-in catalog
    let known: std::collections::HashSet<String> = AVAILABLE_MODELS.iter().map(|(_, _, _, _, f)| f.to_string()).collect();
    let models_dir = get_data_dir().join("models");
    if let Ok(entries) = fs::read_dir(&models_dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_file() {
                continue;
            }
            // Skip temp downloading files and non-.bin
            let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase();
            if ext != "bin" {
                continue;
            }
            if path.extension().is_some() && path.with_extension("").extension().is_some() {
                // just to avoid double extension confusion - no-op
            }
            let fname = match path.file_name().and_then(|n| n.to_str()) {
                Some(n) => n.to_string(),
                None => continue,
            };
            if fname.ends_with(".downloading") {
                continue;
            }
            if known.contains(&fname) {
                continue;
            }
            // Must be at least 1 MB to be considered a valid model
            let meta = match entry.metadata() {
                Ok(m) => m,
                Err(_) => continue,
            };
            let size = meta.len();
            // Include even small files but mark not downloaded if too small? We'll include all .bin
            let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or(&fname);
            // Pretty name: stem -> Title Case + [Custom]
            let pretty = stem.replace(['_', '-'], " ");
            let name = format!("{} [Custom]", pretty);
            items.push(ModelCatalogItem {
                id: fname.clone(),
                name,
                size_desc: format_size(size),
                speed_desc: "Custom GGML".to_string(),
                filename: fname,
                downloaded: size > 0,
                file_size_bytes: size,
            });
        }
    }

    // Stable sort: keep built-ins first, customs alphabetically at end
    items.sort_by(|a, b| {
        let a_builtin = AVAILABLE_MODELS.iter().any(|(id, _, _, _, _)| *id == a.id || *id == b.id);
        let _ = a_builtin;
        // Keep original order for built-ins, alphabetical for customs
        let a_is_custom = a.name.contains("[Custom]");
        let b_is_custom = b.name.contains("[Custom]");
        match (a_is_custom, b_is_custom) {
            (false, true) => std::cmp::Ordering::Less,
            (true, false) => std::cmp::Ordering::Greater,
            (true, true) => a.filename.cmp(&b.filename),
            (false, false) => std::cmp::Ordering::Equal,
        }
    });

    items
}

pub fn map_language_hint(lang: &str) -> Option<&'static str> {
    let l = lang.trim().to_lowercase();
    match l.as_str() {
        "english" | "en" => Some("en"),
        "ukrainian" | "uk" | "ua" => Some("uk"),
        "spanish" | "es" => Some("es"),
        "french" | "fr" => Some("fr"),
        "german" | "de" => Some("de"),
        "italian" | "it" => Some("it"),
        "polish" | "pl" => Some("pl"),
        "portuguese" | "pt" => Some("pt"),
        "japanese" | "ja" => Some("ja"),
        "chinese" | "zh" => Some("zh"),
        "russian" | "ru" => Some("ru"),
        _ => None,
    }
}

/// Whether Whisper may run on the GPU: Settings → Hardware acceleration isn't "CPU only".
static USE_GPU: AtomicBool = AtomicBool::new(true);

pub fn set_device_backend(device_backend: &str) {
    USE_GPU.store(device_backend != "cpu", Ordering::Relaxed);
}

/// Whether the next Whisper model loads onto the GPU. Intel Macs stay on the CPU, as before.
fn gpu_wanted() -> bool {
    USE_GPU.load(Ordering::Relaxed) && !cfg!(all(target_os = "macos", target_arch = "x86_64"))
}

/// The GPUs Whisper can run on: Metal on a Mac, Vulkan on Windows and Linux. Empty without one.
pub fn gpu_devices() -> Vec<String> {
    use whisper_rs_sys as sys;
    // SAFETY: ggml's device registry is built once and lives for the whole process; each
    // description is a NUL-terminated string it owns.
    unsafe {
        (0..sys::ggml_backend_dev_count())
            .map(|i| sys::ggml_backend_dev_get(i))
            .filter(|&dev| {
                matches!(
                    sys::ggml_backend_dev_type(dev),
                    sys::ggml_backend_dev_type_GGML_BACKEND_DEVICE_TYPE_GPU | sys::ggml_backend_dev_type_GGML_BACKEND_DEVICE_TYPE_IGPU
                )
            })
            .map(|dev| std::ffi::CStr::from_ptr(sys::ggml_backend_dev_description(dev)).to_string_lossy().into_owned())
            .collect()
    }
}

pub struct Transcriber {
    context: Option<WhisperContext>,
    current_model: String,
    /// Whether the loaded model was put on the GPU; a change in the setting reloads it.
    current_gpu: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiagnosticSegment {
    pub start_ms: i64,
    pub end_ms: i64,
    pub text: String,
    pub no_speech_prob: f32,
}

#[derive(Debug, Clone)]
pub struct DetailedWhisperResult {
    pub text: String,
    pub detected_lang: String,
    pub inference_ms: u64,
    pub thread_count: i32,
    pub raw_output: String,
    pub segments: Vec<DiagnosticSegment>,
}

impl Transcriber {
    pub fn new() -> Self {
        Self {
            context: None,
            current_model: String::new(),
            current_gpu: false,
        }
    }

    pub fn has_context(&self) -> bool {
        self.context.is_some()
    }

    pub fn unload(&mut self) {
        if self.context.is_some() {
            log::info!("Unloading Whisper model from RAM ({})", self.current_model);
            self.context = None;
            self.current_model.clear();
        }
    }

    pub fn is_loaded(&self, model_path: &Path) -> bool {
        if let Some(s) = model_path.to_str() {
            self.context.is_some() && self.current_model == s && self.current_gpu == gpu_wanted()
        } else {
            false
        }
    }

    pub fn prewarm(&mut self, model_path: &Path) -> Result<(), String> {
        if !self.is_loaded(model_path) {
            log::info!("Prewarming Whisper model into RAM from {:?}", model_path);
            self.load_context(model_path)?;
        }
        Ok(())
    }

    pub fn get_model_path(model_name: &str) -> PathBuf {
        get_data_dir().join("models").join(model_name)
    }

    pub fn model_exists(model_name: &str) -> bool {
        let path = Self::get_model_path(model_name);
        path.exists() && path.metadata().map(|m| m.len() > 1_000_000).unwrap_or(false)
    }

    pub fn is_metal_supported() -> bool {
        cfg!(all(target_os = "macos", target_arch = "aarch64"))
    }

    pub async fn ensure_model(app_handle: &AppHandle, model_name: &str) -> Result<PathBuf, String> {
        if Self::model_exists(model_name) {
            return Ok(Self::get_model_path(model_name));
        }
        // Another caller may already be downloading it; wait, then re-check the disk.
        let _download = crate::model_download::lock().await;
        if Self::model_exists(model_name) {
            return Ok(Self::get_model_path(model_name));
        }

        crate::model_download::began(app_handle, model_name);
        let result = Self::download_model(app_handle, model_name).await;
        crate::model_download::finished(app_handle, model_name, &result);
        result
    }

    async fn download_model(app_handle: &AppHandle, model_name: &str) -> Result<PathBuf, String> {
        let model_path = Self::get_model_path(model_name);
        let parent = model_path.parent().ok_or("Invalid model directory")?;
        if !parent.exists() {
            let _ = fs::create_dir_all(parent);
        }

        let url = format!("{}/{}", HF_BASE_URL, model_name);
        let client = reqwest::Client::new();
        let resp = client
            .get(&url)
            .send()
            .await
            .map_err(|e| format!("Network error downloading model: {}", e))?;

        if !resp.status().is_success() {
            return Err(format!("Download failed with HTTP status: {}", resp.status()));
        }

        let total_size = resp.content_length().unwrap_or(1);
        let temp_path = model_path.with_extension("downloading");
        let mut file = fs::File::create(&temp_path)
            .map_err(|e| format!("Failed to create temporary file: {}", e))?;

        let mut downloaded: u64 = 0;
        let mut resp = resp;

        while let Some(chunk) = resp
            .chunk()
            .await
            .map_err(|e| format!("Chunk error during model download: {}", e))?
        {
            file.write_all(&chunk)
                .map_err(|e| format!("Write error: {}", e))?;
            downloaded += chunk.len() as u64;

            let percent = (downloaded as f64 / total_size as f64 * 100.0) as u32;
            crate::model_download::progress(app_handle, model_name, percent, downloaded, total_size);
        }

        file.flush().map_err(|e| e.to_string())?;
        // Close the file first: Windows refuses to rename a file that is still open.
        drop(file);
        fs::rename(&temp_path, &model_path)
            .map_err(|e| format!("Failed to finalize model file: {}", e))?;

        Ok(model_path)
    }

    fn load_context(&mut self, model_path: &Path) -> Result<(), String> {
        let path_str = model_path
            .to_str()
            .ok_or_else(|| "Invalid model path characters".to_string())?;

        let mut cparams = WhisperContextParameters::default();
        let gpu = gpu_wanted();
        cparams.use_gpu(gpu);

        let rss_before = crate::vitals::process_rss_mb();
        let started = Instant::now();
        let ctx = WhisperContext::new_with_params(path_str, cparams)
            .map_err(|e| format!("Failed to load Whisper model: {:?}", e))?;

        self.context = Some(ctx);
        self.current_model = path_str.to_string();
        self.current_gpu = gpu;
        let name = model_path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        crate::pipeline_logger::log_stage_event(
            &get_data_dir(),
            "MODEL_LOAD",
            &format!(
                "Loaded Whisper {} on the {} in {} ms; app memory {} → {} MB",
                name,
                if gpu { "GPU" } else { "CPU" },
                started.elapsed().as_millis(),
                rss_before,
                crate::vitals::process_rss_mb()
            ),
        );
        Ok(())
    }

    /// Transcribes with full diagnostic details (segments, probabilities, raw string). Each of
    /// `chunks` (ranges of `samples`) is decoded on its own and the texts are joined; the first
    /// chunk picks the language the rest are decoded in, unless `language_override` sets one.
    pub fn transcribe_detailed(
        &mut self,
        samples: &[f32],
        chunks: &[Range<usize>],
        model_path: &Path,
        translate_to_english: bool,
        language_override: Option<&str>,
    ) -> Result<DetailedWhisperResult, String> {
        if samples.is_empty() {
            return Ok(DetailedWhisperResult {
                text: String::new(),
                detected_lang: "unknown".to_string(),
                inference_ms: 0,
                thread_count: 4,
                raw_output: "Empty audio buffer (0 samples)".to_string(),
                segments: Vec::new(),
            });
        }

        if !self.is_loaded(model_path) {
            self.load_context(model_path)?;
        }

        let ctx = self.context.as_ref().ok_or("Whisper context not loaded")?;
        let mut state = ctx
            .create_state()
            .map_err(|e| format!("Failed to create Whisper state: {:?}", e))?;

        let hw = crate::cpu_features::detect_hardware_profile();
        let thread_count = hw.recommended_threads;

        let mut lang_to_use = language_override.and_then(|s| {
            let t = s.trim().to_lowercase();
            if t.is_empty() || t == "auto" || t == "detect" { None } else { Some(t) }
        });

        let t0 = Instant::now();
        let mut texts = Vec::new();
        let mut segments = Vec::new();
        let mut raw_debug_lines = Vec::new();
        let mut detected_lang = "auto";

        for (i, chunk) in chunks.iter().enumerate() {
            let mut params = FullParams::new(SamplingStrategy::Greedy { best_of: 1 });
            params.set_n_threads(thread_count);
            params.set_no_timestamps(false);
            params.set_single_segment(false);

            // Auto-detect language properly:
            // Set language to "auto" and detect_language to false so Whisper runs LID
            // AND then decodes speech into text. Setting detect_language(true) in whisper.cpp
            // aborts decoding immediately after language detection.
            params.set_language(Some(lang_to_use.as_deref().unwrap_or("auto")));
            params.set_detect_language(false);

            params.set_translate(translate_to_english);
            params.set_print_progress(false);
            params.set_print_special(false); // Do not inject special tokens into segments
            params.set_print_realtime(false);
            params.set_print_timestamps(false);

            state
                .full(params, &samples[chunk.clone()])
                .map_err(|e| format!("Whisper transcription failed: {:?}", e))?;

            // Segment times are within the chunk; shifted to times in the whole recording.
            let offset_ms = (chunk.start * 1000 / 16000) as i64;
            let mut chunk_text = String::new();
            for segment in state.as_iter() {
                let start_ms = segment.start_timestamp() * 10 + offset_ms;
                let end_ms = segment.end_timestamp() * 10 + offset_ms;
                let prob = segment.no_speech_probability();
                let seg_text = segment.to_str().unwrap_or("").to_string();

                raw_debug_lines.push(format!(
                    "[{:.2}s - {:.2}s] (no_speech_prob: {:.1}%): {:?}",
                    start_ms as f32 / 1000.0,
                    end_ms as f32 / 1000.0,
                    prob * 100.0,
                    seg_text
                ));

                let clean_seg = strip_special_tokens(&seg_text);
                chunk_text.push_str(&clean_seg);

                segments.push(DiagnosticSegment {
                    start_ms,
                    end_ms,
                    text: clean_seg,
                    no_speech_prob: prob,
                });
            }
            texts.push(chunk_text);

            if i == 0 {
                let detected_lang_id = state.full_lang_id_from_state();
                if detected_lang_id >= 0 {
                    detected_lang = whisper_rs::get_lang_str(detected_lang_id).unwrap_or("auto");
                }
                // Later chunks keep this language, so a long recording can't switch midway.
                if lang_to_use.is_none() && detected_lang != "auto" {
                    lang_to_use = Some(detected_lang.to_string());
                }
            }
        }
        let inference_ms = t0.elapsed().as_millis() as u64;

        let raw_output = if segments.is_empty() {
            format!("0 segments returned by model (detected lang: {})", detected_lang)
        } else {
            raw_debug_lines.join("\n")
        };

        Ok(DetailedWhisperResult {
            text: crate::chunking::join_transcripts(&texts),
            detected_lang: detected_lang.to_string(),
            inference_ms,
            thread_count,
            raw_output,
            segments,
        })
    }

    /// Transcribes 16 kHz float32 mono audio directly in RAM, chunk by chunk.
    /// Returns (transcribed_text, detected_language, inference_duration_ms, thread_count).
    pub fn transcribe(
        &mut self,
        samples: &[f32],
        chunks: &[Range<usize>],
        model_path: &Path,
        translate_to_english: bool,
    ) -> Result<(String, String, u64, i32), String> {
        let res = self.transcribe_detailed(samples, chunks, model_path, translate_to_english, None)?;
        Ok((res.text, res.detected_lang, res.inference_ms, res.thread_count))
    }
}

pub fn strip_special_tokens(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut in_bracket = false;
    let mut bracket_content = String::new();

    for c in s.chars() {
        if c == '[' {
            in_bracket = true;
            bracket_content.clear();
        } else if c == ']' && in_bracket {
            in_bracket = false;
            if !bracket_content.starts_with('_') {
                out.push('[');
                out.push_str(&bracket_content);
                out.push(']');
            }
            bracket_content.clear();
        } else if in_bracket {
            bracket_content.push(c);
        } else {
            out.push(c);
        }
    }
    if in_bracket {
        out.push('[');
        out.push_str(&bracket_content);
    }
    out
}


#[cfg(test)]
mod tests {
    /// Lists this computer's GPUs: `cargo test --lib gpu_devices -- --ignored --nocapture`. Ignored
    /// because CI runners have no GPU.
    #[test]
    #[ignore]
    fn gpu_devices_lists_this_computers_gpu() {
        let gpus = super::gpu_devices();
        println!("GPUs: {:?}", gpus);
        assert!(!gpus.is_empty());
    }
}
