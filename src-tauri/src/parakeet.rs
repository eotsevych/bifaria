use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::time::Instant;

use base64::Engine as _;
use parakeet_rs::Transcriber;
use serde_json::{json, Value};
use tauri::AppHandle;

use crate::settings::get_data_dir;

/// One build of the model: where it lives, its files and their sizes, and where they come from.
struct ModelFiles {
    dir: &'static str,
    files: &'static [(&'static str, u64)],
    url: &'static str,
}

/// Parakeet TDT 0.6B v3 INT8 for ONNX Runtime on the CPU (~670 MB).
const ONNX: ModelFiles = ModelFiles {
    dir: "parakeet-tdt-0.6b-v3",
    files: &[
        ("encoder-model.int8.onnx", 652_183_999),
        ("decoder_joint-model.int8.onnx", 18_202_004),
        ("nemo128.onnx", 139_764),
        ("config.json", 97),
        ("vocab.txt", 93_939),
    ],
    url: "https://huggingface.co/istupakov/parakeet-tdt-0.6b-v3-onnx/resolve/main",
};

/// The same model converted for Core ML, which runs it on the Neural Engine (~480 MB): the files
/// FluidAudio 0.18 loads (src-tauri/apple-asr), 6-bit encoder.
const NEURAL_ENGINE: ModelFiles = ModelFiles {
    dir: "parakeet-tdt-0.6b-v3-coreml",
    files: &[
        ("Decoder.mlmodelc/analytics/coremldata.bin", 243),
        ("Decoder.mlmodelc/coremldata.bin", 554),
        ("Decoder.mlmodelc/metadata.json", 3_427),
        ("Decoder.mlmodelc/model.mil", 13_110),
        ("Decoder.mlmodelc/weights/weight.bin", 23_604_992),
        ("Encoder.mlmodelc/analytics/coremldata.bin", 243),
        ("Encoder.mlmodelc/coremldata.bin", 485),
        ("Encoder.mlmodelc/metadata.json", 2_921),
        ("Encoder.mlmodelc/model.mil", 959_769),
        ("Encoder.mlmodelc/weights/weight.bin", 445_187_200),
        ("JointDecisionv3.mlmodelc/analytics/coremldata.bin", 243),
        ("JointDecisionv3.mlmodelc/coremldata.bin", 521),
        ("JointDecisionv3.mlmodelc/metadata.json", 3_453),
        ("JointDecisionv3.mlmodelc/model.mil", 11_775),
        ("JointDecisionv3.mlmodelc/weights/weight.bin", 12_642_764),
        ("Preprocessor.mlmodelc/analytics/coremldata.bin", 243),
        ("Preprocessor.mlmodelc/coremldata.bin", 486),
        ("Preprocessor.mlmodelc/metadata.json", 2_841),
        ("Preprocessor.mlmodelc/model.mil", 28_181),
        ("Preprocessor.mlmodelc/weights/weight.bin", 491_072),
        ("parakeet_vocab.json", 151_122),
    ],
    url: "https://huggingface.co/FluidInference/parakeet-tdt-0.6b-v3-coreml/resolve/main",
};

/// Parakeet Ultra for Core ML: moondream's post-training of v3, int8-linear encoder. ~632 MB, pinned to revision 95eaa59.
const ULTRA_ANE: ModelFiles = ModelFiles {
    dir: "parakeet-ultra-coreml",
    files: &[
        ("Decoder.mlmodelc/analytics/coremldata.bin", 243),
        ("Decoder.mlmodelc/coremldata.bin", 560),
        ("Decoder.mlmodelc/model.mil", 13_110),
        ("Decoder.mlmodelc/weights/weight.bin", 23_604_992),
        ("Encoder.mlmodelc/analytics/coremldata.bin", 243),
        ("Encoder.mlmodelc/coremldata.bin", 514),
        ("Encoder.mlmodelc/model.mil", 1_002_653),
        ("Encoder.mlmodelc/weights/weight.bin", 594_211_328),
        ("JointDecisionv3.mlmodelc/analytics/coremldata.bin", 243),
        ("JointDecisionv3.mlmodelc/coremldata.bin", 592),
        ("JointDecisionv3.mlmodelc/model.mil", 11_777),
        ("JointDecisionv3.mlmodelc/weights/weight.bin", 12_642_764),
        ("Preprocessor.mlmodelc/analytics/coremldata.bin", 243),
        ("Preprocessor.mlmodelc/coremldata.bin", 486),
        ("Preprocessor.mlmodelc/metadata.json", 2_841),
        ("Preprocessor.mlmodelc/model.mil", 28_181),
        ("Preprocessor.mlmodelc/weights/weight.bin", 491_072),
        ("parakeet_vocab.json", 151_122),
    ],
    url: "https://huggingface.co/FluidInference/parakeet-ultra-coreml/resolve/95eaa59a39d4394f047a4dc5cce480388a60d1b6",
};

/// Parakeet Redux for Core ML: moondream's ternary re-training of v3, 2-bit encoder. ~220 MB, pinned to revision 8c5ef97.
const REDUX_ANE: ModelFiles = ModelFiles {
    dir: "parakeet-redux-coreml",
    files: &[
        ("Decoder.mlmodelc/analytics/coremldata.bin", 243),
        ("Decoder.mlmodelc/coremldata.bin", 560),
        ("Decoder.mlmodelc/model.mil", 13_110),
        ("Decoder.mlmodelc/weights/weight.bin", 23_604_992),
        ("Encoder.mlmodelc/analytics/coremldata.bin", 243),
        ("Encoder.mlmodelc/coremldata.bin", 506),
        ("Encoder.mlmodelc/model.mil", 965_906),
        ("Encoder.mlmodelc/weights/weight.bin", 182_389_312),
        ("JointDecisionv3.mlmodelc/analytics/coremldata.bin", 243),
        ("JointDecisionv3.mlmodelc/coremldata.bin", 592),
        ("JointDecisionv3.mlmodelc/model.mil", 11_777),
        ("JointDecisionv3.mlmodelc/weights/weight.bin", 12_642_764),
        ("Preprocessor.mlmodelc/analytics/coremldata.bin", 243),
        ("Preprocessor.mlmodelc/coremldata.bin", 486),
        ("Preprocessor.mlmodelc/metadata.json", 2_841),
        ("Preprocessor.mlmodelc/model.mil", 28_181),
        ("Preprocessor.mlmodelc/weights/weight.bin", 491_072),
        ("parakeet_vocab.json", 151_122),
    ],
    url: "https://huggingface.co/FluidInference/parakeet-redux-coreml/resolve/8c5ef97a29cd120dc76b354b3f22b7fec3b486f9",
};

/// Phonon-2 for Core ML, English: Fermion Research's re-training of v3; only the default encoder. ~378 MB, pinned to revision 1702fc2.
const PHONON2_ANE: ModelFiles = ModelFiles {
    dir: "phonon-2-coreml",
    files: &[
        ("Decoder.mlmodelc/analytics/coremldata.bin", 243),
        ("Decoder.mlmodelc/coremldata.bin", 554),
        ("Decoder.mlmodelc/model.mil", 13_110),
        ("Decoder.mlmodelc/weights/weight.bin", 23_604_992),
        ("Encoder.mlmodelc/analytics/coremldata.bin", 243),
        ("Encoder.mlmodelc/coremldata.bin", 500),
        ("Encoder.mlmodelc/model.mil", 2_734_921),
        ("Encoder.mlmodelc/weights/weight.bin", 338_391_488),
        ("JointDecisionv3.mlmodelc/analytics/coremldata.bin", 243),
        ("JointDecisionv3.mlmodelc/coremldata.bin", 586),
        ("JointDecisionv3.mlmodelc/model.mil", 11_777),
        ("JointDecisionv3.mlmodelc/weights/weight.bin", 12_642_764),
        ("Preprocessor.mlmodelc/analytics/coremldata.bin", 243),
        ("Preprocessor.mlmodelc/coremldata.bin", 486),
        ("Preprocessor.mlmodelc/metadata.json", 2_841),
        ("Preprocessor.mlmodelc/model.mil", 28_181),
        ("Preprocessor.mlmodelc/weights/weight.bin", 491_072),
        ("parakeet_vocab.json", 151_122),
    ],
    url: "https://huggingface.co/FluidInference/phonon-2-coreml/resolve/1702fc22e76f203d5ca464cd5d3ef130a45ce2c0",
};

/// The helper that runs the Neural Engine build, bundled next to the Bifaria executable.
const ANE_HELPER: &str = "bifaria-asr";

/// A Parakeet model Bifaria offers. v3 has a CPU (ONNX) and a Neural Engine build; its re-trained
/// siblings ship only for Core ML, so they're offered where the Neural Engine helper runs.
pub struct Variant {
    /// Its `model_name` in settings and the model list.
    pub name: &'static str,
    pub title: &'static str,
    pub summary: &'static str,
    cpu: Option<&'static ModelFiles>,
    ane: Option<&'static ModelFiles>,
    /// FluidAudio's model version the helper loads it as.
    ane_version: &'static str,
    /// Oldest macOS its Core ML build runs on (Phonon-2's encoder format needs macOS 15).
    min_macos: u32,
}

const V3: Variant = Variant {
    name: crate::model_download::PARAKEET_MODEL,
    title: "Parakeet TDT 0.6B v3",
    summary: "Real-Time (~80-150 ms, 25 langs)",
    cpu: Some(&ONNX),
    ane: Some(&NEURAL_ENGINE),
    ane_version: "v3",
    min_macos: 14,
};
const ULTRA: Variant = Variant {
    name: "parakeet-ultra",
    title: "Parakeet Ultra",
    summary: "Most accurate Parakeet, same speed (25 langs)",
    cpu: None,
    ane: Some(&ULTRA_ANE),
    ane_version: "ultra",
    min_macos: 14,
};
const REDUX: Variant = Variant {
    name: "parakeet-redux",
    title: "Parakeet Redux",
    summary: "Smallest; better than v3 in other languages, a bit worse in English",
    cpu: None,
    ane: Some(&REDUX_ANE),
    ane_version: "redux",
    min_macos: 14,
};
const PHONON2: Variant = Variant {
    name: "phonon-2",
    title: "Phonon-2",
    summary: "English only; the fastest",
    cpu: None,
    ane: Some(&PHONON2_ANE),
    ane_version: "phonon2",
    min_macos: 15,
};

/// Every Parakeet model, v3 first.
pub static VARIANTS: [&Variant; 4] = [&V3, &ULTRA, &REDUX, &PHONON2];

/// Whether `model_name` is one of the Parakeet models (any variant).
pub fn is_parakeet(model_name: &str) -> bool {
    VARIANTS.iter().any(|v| v.name == model_name)
}

/// The Parakeet model called `model_name`, or v3.
pub fn variant(model_name: &str) -> &'static Variant {
    VARIANTS.iter().copied().find(|v| v.name == model_name).unwrap_or(&V3)
}

/// The variant whose Core ML build lives in `dir`.
fn neural_engine_variant(dir: &Path) -> Option<&'static Variant> {
    VARIANTS.iter().copied().find(|v| v.ane.is_some_and(|a| a.path() == dir))
}

impl Variant {
    /// Its Core ML build can run here: the helper is available and macOS is new enough.
    fn on_neural_engine_here(&self) -> bool {
        self.ane.is_some()
            && ane_helper().is_some()
            && crate::apple_translate::macos_version().is_some_and(|(major, _)| major >= self.min_macos)
    }

    /// Whether this computer can run it: v3 everywhere, the others on the Neural Engine.
    pub fn available(&self) -> bool {
        self.cpu.is_some() || self.on_neural_engine_here()
    }

    /// The build to use: the Core ML one where it runs, unless only the CPU one is downloaded so
    /// far (it's then fetched in the background, see `download_neural_engine_model`).
    fn current(&self) -> &'static ModelFiles {
        match (self.ane, self.cpu) {
            (Some(ane), Some(cpu)) if self.on_neural_engine_here() && (ane.ready() || !cpu.ready()) => ane,
            (_, Some(cpu)) => cpu,
            (Some(ane), None) => ane,
            (None, None) => unreachable!("every variant has a build"),
        }
    }
}

impl ModelFiles {
    fn path(&self) -> PathBuf {
        get_data_dir().join("models").join(self.dir)
    }

    /// Every file is on disk (a download writes each under a temporary name until it's complete).
    fn ready(&self) -> bool {
        let dir = self.path();
        self.files.iter().all(|(name, _)| dir.join(name).exists())
    }
}

/// The Neural Engine helper, when this Mac can use it: Apple Silicon, macOS 14+ (FluidAudio's
/// minimum), and a real helper bundled (a build without Swift ships a stand-in script).
fn ane_helper() -> Option<PathBuf> {
    if !cfg!(all(target_os = "macos", target_arch = "aarch64")) {
        return None;
    }
    if crate::apple_translate::macos_version().is_none_or(|(major, _)| major < 14) {
        return None;
    }
    let path = std::env::current_exe().ok()?.parent()?.join(ANE_HELPER);
    let head = fs::read(&path).ok()?;
    (!head.starts_with(b"#!")).then_some(path)
}

/// The helper process, kept running so the model stays on the Neural Engine between dictations.
struct AneHelper {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
}

impl AneHelper {
    /// Starts the helper and loads the model from `dir` as FluidAudio's `version` (compiled for the
    /// Neural Engine on the first load, which can take tens of seconds; Core ML caches it after that).
    fn start(path: &Path, dir: &Path, version: &str) -> Result<Self, String> {
        let mut child = Command::new(path)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|e| format!("couldn't start {ANE_HELPER}: {e}"))?;
        let stdin = child.stdin.take().ok_or("the helper has no stdin")?;
        let stdout = BufReader::new(child.stdout.take().ok_or("the helper has no stdout")?);
        let mut helper = Self { child, stdin, stdout };
        helper.ask(json!({ "op": "load", "dir": dir, "version": version }))?;
        Ok(helper)
    }

    /// Sends one request and reads its answer.
    /// shortcut: no timeout on the answer; add one if the helper is ever seen hanging.
    fn ask(&mut self, request: Value) -> Result<Value, String> {
        writeln!(self.stdin, "{request}").and_then(|_| self.stdin.flush()).map_err(|e| format!("couldn't talk to {ANE_HELPER}: {e}"))?;
        let mut line = String::new();
        if self.stdout.read_line(&mut line).map_err(|e| e.to_string())? == 0 {
            return Err(format!("{ANE_HELPER} quit"));
        }
        let answer: Value = serde_json::from_str(&line).map_err(|_| format!("unexpected answer from {ANE_HELPER}: {line}"))?;
        if answer["ok"].as_bool() == Some(true) {
            return Ok(answer);
        }
        Err(answer["message"].as_str().unwrap_or("unknown error").to_string())
    }

    fn transcribe(&mut self, samples: &[f32]) -> Result<String, String> {
        let bytes: Vec<u8> = samples.iter().flat_map(|s| s.to_le_bytes()).collect();
        let audio = base64::engine::general_purpose::STANDARD.encode(bytes);
        let answer = self.ask(json!({ "op": "transcribe", "audio": audio }))?;
        Ok(answer["text"].as_str().unwrap_or_default().trim().to_string())
    }
}

impl Drop for AneHelper {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Wraps the parakeet-rs crate (CPU) and the Neural Engine helper (Apple Silicon).
/// Caches the loaded model in memory.
pub struct ParakeetTranscriber {
    model: Option<parakeet_rs::ParakeetTDT>,
    ane: Option<AneHelper>,
    current_dir: String,
    /// Threads the loaded model's session runs on (0 on the Neural Engine).
    threads: usize,
}

impl ParakeetTranscriber {
    pub fn new() -> Self {
        Self {
            model: None,
            ane: None,
            current_dir: String::new(),
            threads: 0,
        }
    }

    pub fn has_model(&self) -> bool {
        self.model.is_some() || self.ane.is_some()
    }

    pub fn unload(&mut self) {
        if self.has_model() {
            log::info!("Unloading Parakeet model from RAM");
            self.model = None;
            self.ane = None;
            self.current_dir.clear();
        }
    }

    pub fn is_loaded(&self, model_dir: &Path) -> bool {
        if let Some(s) = model_dir.to_str() {
            self.has_model() && self.current_dir == s
        } else {
            false
        }
    }

    pub fn prewarm(&mut self, model_dir: &Path) -> Result<(), String> {
        if !self.is_loaded(model_dir) {
            log::info!("Prewarming Parakeet model into RAM from {:?}", model_dir);
            self.load_model(model_dir)?;
        }
        Ok(())
    }

    /// Where the files of `model_name`'s build in use are stored (see `Variant::current`).
    pub fn get_model_dir(model_name: &str) -> PathBuf {
        let dir = variant(model_name).current().path();
        if !dir.exists() {
            let _ = fs::create_dir_all(&dir);
        }
        dir
    }

    /// True when `model_name`'s build in use has all its files on disk.
    pub fn model_ready(model_name: &str) -> bool {
        variant(model_name).current().ready()
    }

    /// True when `model_name` runs on the Neural Engine build (it may still be downloading).
    pub fn runs_on_neural_engine(model_name: &str) -> bool {
        let v = variant(model_name);
        v.ane.is_some_and(|ane| std::ptr::eq(v.current(), ane))
    }

    /// Where Parakeet runs on this computer, for Settings: "the Neural Engine", or the CPU and why.
    pub fn engine_label() -> String {
        if ane_helper().is_none() {
            return format!("the CPU ({} threads)", crate::cpu_features::parakeet_threads());
        }
        if NEURAL_ENGINE.ready() || !ONNX.ready() {
            "the Neural Engine".to_string()
        } else {
            "the CPU until its Neural Engine model finishes downloading".to_string()
        }
    }

    /// Download size of `model_name`'s build in use, in bytes.
    pub fn download_size(model_name: &str) -> u64 {
        variant(model_name).current().files.iter().map(|(_, s)| s).sum()
    }

    /// Total size of `model_name`'s downloaded files in bytes.
    pub fn downloaded_size(model_name: &str) -> u64 {
        let model = variant(model_name).current();
        let dir = model.path();
        model
            .files
            .iter()
            .map(|(name, _)| {
                dir.join(name)
                    .metadata()
                    .map(|m| m.len())
                    .unwrap_or(0)
            })
            .sum()
    }

    /// Downloads `model_name`'s files from Hugging Face. Skips files already on disk.
    pub async fn ensure_model(app_handle: &AppHandle, model_name: &str) -> Result<PathBuf, String> {
        let v = variant(model_name);
        if !v.available() {
            return Err(format!("{} runs only on the Neural Engine of a Mac with Apple silicon (macOS {}+)", v.title, v.min_macos));
        }
        if Self::model_ready(model_name) {
            return Ok(Self::get_model_dir(model_name));
        }
        // Another caller may already be downloading it; wait, then re-check the disk.
        let _download = crate::model_download::lock().await;
        if Self::model_ready(model_name) {
            return Ok(Self::get_model_dir(model_name));
        }

        crate::model_download::began(app_handle, v.name);
        let result = Self::download_model(app_handle, v.current(), v.name).await;
        crate::model_download::finished(app_handle, v.name, &result);
        result
    }

    /// Where the CPU model is in use only because the Neural Engine one isn't downloaded yet,
    /// downloads it (once, ~480 MB) so the next dictations run on the Neural Engine. True when it
    /// was downloaded now. The CPU model is kept as the fallback.
    pub async fn download_neural_engine_model(app_handle: &AppHandle, model_name: &str) -> bool {
        let v = variant(model_name);
        // Only a model with both builds starts on the CPU one (v3); the others are Core ML only.
        let Some(ane) = v.ane.filter(|_| v.cpu.is_some() && v.on_neural_engine_here()) else {
            return false;
        };
        if ane.ready() {
            return false;
        }
        let _download = crate::model_download::lock().await;
        if ane.ready() {
            return false;
        }
        log::info!("Downloading {} for the Neural Engine", v.title);
        crate::model_download::began(app_handle, v.name);
        let result = Self::download_model(app_handle, ane, v.name).await;
        crate::model_download::finished(app_handle, v.name, &result);
        if let Err(e) = &result {
            log::warn!("Parakeet for the Neural Engine didn't download, staying on the CPU: {e}");
        }
        result.is_ok()
    }

    async fn download_model(app_handle: &AppHandle, model: &ModelFiles, model_name: &str) -> Result<PathBuf, String> {
        let model_dir = model.path();
        let client = reqwest::Client::new();
        let total_size: u64 = model.files.iter().map(|(_, s)| s).sum();
        let mut cumulative: u64 = 0;

        for (filename, _expected) in model.files {
            let file_path = model_dir.join(filename);

            // Skip files already downloaded
            if file_path.exists() && file_path.metadata().map(|m| m.len() > 0).unwrap_or(false) {
                cumulative += file_path.metadata().map(|m| m.len()).unwrap_or(0);
                continue;
            }
            if let Some(parent) = file_path.parent() {
                fs::create_dir_all(parent).map_err(|e| format!("Cannot create {}: {}", parent.display(), e))?;
            }

            let url = format!("{}/{}", model.url, filename);

            let resp = client
                .get(&url)
                .send()
                .await
                .map_err(|e| format!("Download failed for {}: {}", filename, e))?;

            if !resp.status().is_success() {
                return Err(format!("HTTP {} for {}", resp.status(), filename));
            }

            let temp_path = file_path.with_extension("downloading");
            let mut file = fs::File::create(&temp_path)
                .map_err(|e| format!("Cannot create {}: {}", filename, e))?;

            let mut resp = resp;
            while let Some(chunk) = resp
                .chunk()
                .await
                .map_err(|e| format!("Chunk error for {}: {}", filename, e))?
            {
                file.write_all(&chunk)
                    .map_err(|e| format!("Write error: {}", e))?;
                cumulative += chunk.len() as u64;

                let percent = (cumulative as f64 / total_size as f64 * 100.0) as u32;
                crate::model_download::progress(
                    app_handle,
                    model_name,
                    percent,
                    cumulative,
                    total_size,
                );
            }

            file.flush().map_err(|e| e.to_string())?;
            // Close the file first: Windows refuses to rename a file that is still open.
            drop(file);
            fs::rename(&temp_path, &file_path)
                .map_err(|e| format!("Rename failed for {}: {}", filename, e))?;
        }

        Ok(model_dir)
    }

    /// Load the model in `model_dir` into memory: the Neural Engine build in the helper, else ONNX.
    fn load_model(&mut self, model_dir: &Path) -> Result<(), String> {
        let dir_str = model_dir
            .to_str()
            .ok_or_else(|| "Bad model path".to_string())?;

        let rss_before = crate::vitals::process_rss_mb();
        let started = Instant::now();
        if let Some(v) = neural_engine_variant(model_dir) {
            let helper = ane_helper().ok_or("the Neural Engine helper isn't available")?;
            // One model at a time: the CPU session goes before the helper loads.
            self.model = None;
            self.ane = None;
            self.ane = Some(AneHelper::start(&helper, model_dir, v.ane_version)?);
            self.current_dir = dir_str.to_string();
            self.threads = 0;
            crate::pipeline_logger::log_stage_event(
                &get_data_dir(),
                "MODEL_LOAD",
                &format!("Loaded {} on the Neural Engine in {} ms (in {ANE_HELPER})", v.title, started.elapsed().as_millis()),
            );
            return Ok(());
        }
        self.ane = None;
        // The crate defaults to 4 threads whatever the machine has.
        let threads = crate::cpu_features::parakeet_threads();
        let config = parakeet_rs::ExecutionConfig::default().with_intra_threads(threads);
        let parakeet = parakeet_rs::ParakeetTDT::from_pretrained(dir_str, Some(config))
            .map_err(|e| format!("Failed to load Parakeet: {:?}", e))?;

        self.model = Some(parakeet);
        self.current_dir = dir_str.to_string();
        self.threads = threads;
        crate::pipeline_logger::log_stage_event(
            &get_data_dir(),
            "MODEL_LOAD",
            &format!(
                "Loaded Parakeet TDT in {} ms on {} threads; app memory {} → {} MB",
                started.elapsed().as_millis(),
                threads,
                rss_before,
                crate::vitals::process_rss_mb()
            ),
        );
        Ok(())
    }

    /// Transcribe 16 kHz f32 mono samples, each of `chunks` (ranges of `samples`) on its own.
    /// Returns (text, detected_lang, inference_ms, thread_count).
    pub fn transcribe(
        &mut self,
        samples: &[f32],
        chunks: &[Range<usize>],
        model_dir: &Path,
    ) -> Result<(String, String, u64, i32), String> {
        if samples.is_empty() {
            return Ok((String::new(), "auto".to_string(), 0, 0));
        }

        let mut model_dir = model_dir.to_path_buf();
        if neural_engine_variant(&model_dir).is_some() {
            match self.transcribe_on_neural_engine(samples, &model_dir) {
                Ok(done) => return Ok(done),
                // A helper that failed is started afresh next time; this recording goes to the CPU.
                Err(e) if ONNX.ready() => {
                    log::warn!("Parakeet on the Neural Engine failed ({e}); transcribing with Parakeet v3 on the CPU");
                    self.ane = None;
                    model_dir = ONNX.path();
                }
                Err(e) => {
                    self.ane = None;
                    return Err(format!("Parakeet on the Neural Engine failed: {e}"));
                }
            }
        }

        let dir_str = model_dir.to_str().unwrap_or_default();
        if self.model.is_none() || self.current_dir != dir_str {
            self.load_model(&model_dir)?;
        }

        let parakeet = self
            .model
            .as_mut()
            .ok_or("Parakeet model not loaded")?;

        let t0 = Instant::now();
        let mut texts = Vec::new();
        for chunk in chunks {
            let audio = &samples[chunk.clone()];
            for range in model_segments(audio, 16000) {
                let result = parakeet
                    .transcribe_samples(audio[range].to_vec(), 16000, 1, None)
                    .map_err(|e| format!("Parakeet transcription error: {:?}", e))?;
                texts.push(result.text);
            }
        }
        let inference_ms = t0.elapsed().as_millis() as u64;

        Ok((crate::chunking::join_transcripts(&texts), "auto".to_string(), inference_ms, self.threads as i32))
    }

    /// The whole recording in one call: FluidAudio windows long audio itself, with overlap. Our own
    /// cuts lost words at each one and cost ~0.7 s on 3 minutes, so `chunks` aren't used here.
    fn transcribe_on_neural_engine(&mut self, samples: &[f32], model_dir: &Path) -> Result<(String, String, u64, i32), String> {
        if self.ane.is_none() || self.current_dir != model_dir.to_str().unwrap_or_default() {
            self.load_model(model_dir)?;
        }
        let helper = self.ane.as_mut().ok_or("Parakeet model not loaded")?;
        let t0 = Instant::now();
        let text = helper.transcribe(samples)?;
        Ok((text, "auto".to_string(), t0.elapsed().as_millis() as u64, 0))
    }
}

/// Longest audio sent to Parakeet in one pass. The model's position table ends near 4 m 50 s
/// (3651 encoder steps of 80 ms) and attention memory grows with the square of the length.
const MAX_SEGMENT_SEC: usize = 60;
/// A long recording is cut at the quietest 20 ms within this many seconds before the limit.
const CUT_SEARCH_SEC: usize = 10;

/// Splits speech into model-sized pieces, cutting in pauses so no word is split.
fn model_segments(samples: &[f32], sample_rate: usize) -> Vec<std::ops::Range<usize>> {
    cut_at_pauses(samples, sample_rate, MAX_SEGMENT_SEC)
}

/// Splits `samples` into pieces of at most `max_sec` (more than CUT_SEARCH_SEC), each cut at the
/// quietest 20 ms shortly before the limit.
pub(crate) fn cut_at_pauses(samples: &[f32], sample_rate: usize, max_sec: usize) -> Vec<std::ops::Range<usize>> {
    let max_len = max_sec * sample_rate;
    let search = CUT_SEARCH_SEC * sample_rate;
    let frame = sample_rate / 50;
    let mut segments = Vec::new();
    let mut start = 0;
    while samples.len() - start > max_len {
        let window_end = start + max_len;
        let window_start = window_end - search;
        let quietest = (window_start..window_end - frame)
            .step_by(frame)
            .min_by(|&a, &b| frame_energy(&samples[a..a + frame]).total_cmp(&frame_energy(&samples[b..b + frame])))
            .map(|f| f + frame / 2)
            .unwrap_or(window_end);
        segments.push(start..quietest);
        start = quietest;
    }
    if start < samples.len() {
        segments.push(start..samples.len());
    }
    segments
}

fn frame_energy(frame: &[f32]) -> f32 {
    frame.iter().map(|x| x * x).sum()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_parakeet_variants_by_name_and_folder() {
        assert!(is_parakeet(crate::model_download::PARAKEET_MODEL));
        assert!(is_parakeet("parakeet-ultra") && is_parakeet("parakeet-redux") && is_parakeet("phonon-2"));
        assert!(!is_parakeet("ggml-base.bin"));
        assert_eq!(variant("parakeet-ultra").title, "Parakeet Ultra");
        // An unknown name falls back to v3.
        assert_eq!(variant("something-else").name, crate::model_download::PARAKEET_MODEL);
        // Each Core ML folder belongs to its own variant, so the helper loads the right version.
        for v in VARIANTS {
            let ane = v.ane.expect("every variant has a Core ML build");
            assert_eq!(neural_engine_variant(&ane.path()).map(|f| f.name), Some(v.name));
        }
        assert!(neural_engine_variant(&ONNX.path()).is_none());
        // v3 runs anywhere (its CPU build); the others only where the Neural Engine helper runs.
        assert!(V3.available());
    }

    /// The helper protocol, against a stand-in that loads anything and answers "hi" to audio.
    #[cfg(unix)]
    #[test]
    fn talks_to_the_neural_engine_helper() {
        use std::os::unix::fs::PermissionsExt;
        let dir = std::env::temp_dir().join(format!("bifaria-asr-test-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let fake = dir.join("bifaria-asr");
        fs::write(
            &fake,
            "#!/bin/sh\nwhile read -r line; do case \"$line\" in\n*'\"op\":\"load\"'*'\"version\":\"ultra\"'*) echo '{\"ok\":true,\"ms\":1}';;\n*'\"transcribe\"'*) echo '{\"ok\":true,\"text\":\" hi \",\"ms\":1}';;\n*) echo '{\"ok\":false,\"error\":\"bad_request\",\"message\":\"no\"}';;\nesac; done\n",
        )
        .unwrap();
        fs::set_permissions(&fake, fs::Permissions::from_mode(0o755)).unwrap();

        // The stand-in accepts only a load that names the model version.
        let mut helper = AneHelper::start(&fake, &dir, "ultra").unwrap();
        assert_eq!(helper.transcribe(&[0.1, -0.2]).unwrap(), "hi");
        assert_eq!(helper.ask(json!({ "op": "other" })).unwrap_err(), "no");
        drop(helper);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn short_audio_is_one_segment() {
        let samples = vec![0.1f32; 16000 * 30];
        assert_eq!(model_segments(&samples, 16000), vec![0..samples.len()]);
    }

    #[test]
    fn long_audio_is_cut_in_pauses_and_fully_covered() {
        // 12 minutes of "speech" with a 100 ms pause every 7 seconds.
        let rate = 16000;
        let samples: Vec<f32> = (0..rate * 720)
            .map(|i| if (i / (rate / 10)) % 70 == 69 { 0.0 } else { 0.2 * ((i as f32) * 0.05).sin() })
            .collect();
        let segments = model_segments(&samples, rate);

        assert!(segments.len() >= 12, "{} segments", segments.len());
        assert_eq!(segments.first().unwrap().start, 0);
        assert_eq!(segments.last().unwrap().end, samples.len());
        for pair in segments.windows(2) {
            assert_eq!(pair[0].end, pair[1].start, "segments must be contiguous");
        }
        for s in &segments {
            assert!(s.len() <= MAX_SEGMENT_SEC * rate);
            if s.end != samples.len() {
                assert_eq!(samples[s.end], 0.0, "cut at {} is not in a pause", s.end);
            }
        }
    }
}
