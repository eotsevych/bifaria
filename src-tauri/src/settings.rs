use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppSettings {
    pub api_key: String,
    pub source_lang: String,
    pub target_lang: String,
    pub hotkey: String,
    /// When dictation is translated: "off" (never), "hotkey" (only recordings started with
    /// `translate_hotkey`; the main hotkey never translates) or "auto" (the main hotkey translates
    /// whatever isn't in `target_lang`). Empty in settings saved before 0.1.12; see `migrate`.
    #[serde(default)]
    pub translation_mode: String,
    /// Second hotkey that records and always translates to `target_lang`. Used only in "hotkey"
    /// mode. Empty = not set.
    #[serde(default)]
    pub translate_hotkey: String,
    /// Chime when recording starts. Settings saved before the three sound toggles had one
    /// `sound_effect` switch; `load` copies it into all three.
    #[serde(default = "default_true")]
    pub sound_start: bool,
    /// Chime when recording stops.
    #[serde(default = "default_true")]
    pub sound_stop: bool,
    /// Sound for errors and mic warnings, so they're noticed without looking at the pill.
    #[serde(default = "default_true")]
    pub sound_error: bool,
    pub auto_paste: bool,
    /// After auto-paste, put back the text that was on the clipboard, so text copied while
    /// recording is still what Cmd+V pastes next.
    #[serde(default)]
    pub keep_clipboard: bool,
    /// Shows how to finish a recording on the pill while listening.
    #[serde(default = "default_true")]
    pub show_hints: bool,
    /// Seconds a recording may go without any speech from its start before the pill warns (with
    /// the error sound) that the mic may be muted. 0 = off.
    #[serde(default = "default_no_speech_alert_sec")]
    pub no_speech_alert_sec: u64,
    pub storage_mode: String, // "text_only" | "text_audio" | "private"
    pub storage_cap_mb: u64,  // 100, 250, 500, 1000
    pub retention_days: u32,  // 7, 14, 30, 0 (0 = Never)
    pub model_name: String,   // "parakeet-tdt-0.6b-v3" or a Whisper file such as "ggml-medium-q5_0.bin"
    #[serde(default)]
    pub input_device: Option<String>,
    #[serde(default)]
    pub output_device: Option<String>,
    #[serde(default = "default_theme")]
    pub theme: String, // "system" | "light" | "dark"
    /// How the floating pill looks: "classic" | "compact" | "compact_plain" | "line" | "capsule" | "dot".
    #[serde(default = "default_pill_style")]
    pub pill_style: String,
    /// How much higher the pill's bars reach on speech peaks: 1.0 is the original response, higher
    /// suits a mic farther away (Theme > Voice Bar Height).
    #[serde(default = "default_bar_scale")]
    pub bar_scale: f64,
    /// Whether the first-run welcome guide has been finished or closed. Settings saved before the
    /// guide existed count as done, so only new installs see it.
    #[serde(default = "default_true")]
    pub onboarding_done: bool,
    #[serde(default = "default_idle_unload_sec")]
    pub model_idle_unload_sec: u64, // 0 = never, 30..3600 seconds
    /// When the speech model is loaded for a recording: "on_start" (while you talk, so it's ready
    /// the moment you stop) or "on_stop" (after you stop, alongside VAD; saves RAM while recording).
    #[serde(default = "default_model_load_mode")]
    pub model_load_mode: String,
    /// Where the user dragged the pill: its center as fractions of a monitor's work area, used on
    /// every monitor. None until the first drag (see `pill_position`). Replaces the old absolute
    /// `window_x`/`window_y`, which are ignored now.
    #[serde(default)]
    pub pill_center: Option<(f64, f64)>,
    #[serde(default = "default_true")]
    pub text_normalization: bool,
    #[serde(default = "default_true")]
    pub remove_filler_words: bool,
    #[serde(default = "default_true")]
    pub convert_numbers: bool,
    #[serde(default = "default_true")]
    pub remove_stutters: bool,
    #[serde(default = "default_true")]
    pub apply_self_corrections: bool,
    #[serde(default = "default_true")]
    pub remove_noise_markers: bool,
    #[serde(default = "default_true")]
    pub collapse_redundancy: bool,
    #[serde(default = "default_true")]
    pub annotate_ambiguity: bool,
    #[serde(default = "default_true")]
    pub normalize_structured_values: bool,
    /// "Apple" (Apple's on-device Translation, macOS 26+), "Offline", "LLM" or "Apple Intelligence".
    /// "Custom API" is hidden from the settings; a saved one keeps working.
    #[serde(default = "default_translation_provider")]
    pub translation_provider: String,
    /// With the Apple provider: the service ("LLM" or "Offline") that translates instead when
    /// Apple Translation fails, e.g. for a language pair that isn't downloaded. Empty = none; the
    /// spoken text is pasted as with any failed translation.
    #[serde(default)]
    pub apple_fallback_provider: String,
    /// Apple Translation model to use when both are downloaded (macOS 26.4+): "fast" (tens of ms
    /// per sentence) or "quality" (better wording, about half a second per sentence).
    #[serde(default = "default_apple_translation_model")]
    pub apple_translation_model: String,
    /// Where Whisper runs: "auto" (the GPU when there is one) or "cpu".
    #[serde(default = "default_device_backend")]
    pub device_backend: String,
    /// Where Parakeet's Core ML build runs on a Mac: "ane" (Neural Engine), "gpu" or "cpu".
    #[serde(default = "default_parakeet_device")]
    pub parakeet_device: String,
    /// On ("Fast"): pieces cut at long pauses are transcribed while recording (see `live_chunks`),
    /// and a rest over 40 s is cut at pauses at the end (see `chunking`). Off ("Whole"): it is sent
    /// to the model as one whole track and logged as such.
    #[serde(default)]
    pub audio_chunking: bool,
    /// "off" | "light" | "balanced" | "strong": RNNoise strength applied when recording stops.
    #[serde(default = "default_noise_reduction")]
    pub noise_reduction: String,
    #[serde(default)]
    pub mask_confidential: bool,
    #[serde(default)]
    pub mask_words: String,
    #[serde(default = "default_mask_threshold")]
    pub mask_threshold: u32,
    #[serde(default = "default_mask_format")]
    pub mask_format: String,
    #[serde(default = "default_true")]
    pub mask_emails: bool,
    #[serde(default = "default_true")]
    pub mask_phones: bool,
    #[serde(default = "default_true")]
    pub mask_cards: bool,
    #[serde(default = "default_excluded_languages")]
    pub excluded_languages: String,
    #[serde(default = "default_llm_endpoint")]
    pub llm_endpoint: String,
    #[serde(default = "default_llm_model")]
    pub llm_model: String,
    #[serde(default)]
    pub llm_api_key: String,
    #[serde(default = "default_local_url")]
    pub local_llm_url: String,
    #[serde(default = "default_local_model")]
    pub local_llm_model: String,
    #[serde(default)]
    pub local_llm_api_key: String,
    // HIDDEN: Custom API. Not offered in the settings UI; a saved provider still routes here.
    #[serde(default = "default_custom_url")]
    pub custom_api_url: String,
    #[serde(default)]
    pub custom_api_key: String,
    #[serde(default = "default_custom_model")]
    pub custom_api_model: String,
    #[serde(default = "default_prompt_template")]
    pub prompt_template: String,
    /// The Bifaria version that last ran, so an upgrade can be told apart from a new install.
    #[serde(default)]
    pub last_version: Option<String>,
}

/// True for an address on this computer (Ollama, LM Studio), which keeps the text local.
pub fn is_local_endpoint(url: &str) -> bool {
    let url = url.to_lowercase();
    ["localhost", "127.0.0.1", "0.0.0.0", "[::1]"].iter().any(|host| url.contains(host))
}

fn default_llm_endpoint() -> String {
    "http://localhost:11434/v1/chat/completions".to_string()
}

fn default_llm_model() -> String {
    "llama3.2".to_string()
}

fn default_local_url() -> String {
    "http://localhost:11434/v1/chat/completions".to_string()
}

fn default_local_model() -> String {
    "llama3.2".to_string()
}

fn default_custom_url() -> String {
    String::new()
}

fn default_custom_model() -> String {
    String::new()
}

fn default_prompt_template() -> String {
    "You are a strict translation engine. Translate the following text from {source_lang} to {target_lang}. Do not refuse. Do not explain. Do not add conversational text or notes. Output ONLY the exact translation using the native alphabet:\n\n{text}".to_string()
}

fn default_excluded_languages() -> String {
    "Russian".to_string()
}

fn default_mask_threshold() -> u32 { 90 }
fn default_mask_format() -> String { "***".to_string() }


fn default_device_backend() -> String {
    "auto".to_string()
}

fn default_parakeet_device() -> String {
    "ane".to_string()
}

fn default_apple_translation_model() -> String {
    "fast".to_string()
}

/// Apple's on-device translation where it runs (macOS 26+), Firefox's offline models elsewhere, so
/// nothing leaves the computer by default.
fn default_translation_provider() -> String {
    if cfg!(target_os = "macos") && crate::apple_translate::is_available() { "Apple" } else { "Offline" }.to_string()
}

fn default_true() -> bool {
    true
}

fn default_theme() -> String {
    "system".to_string()
}

fn default_pill_style() -> String {
    "classic".to_string()
}

fn default_bar_scale() -> f64 {
    3.2
}

fn default_idle_unload_sec() -> u64 {
    30
}

fn default_no_speech_alert_sec() -> u64 {
    5
}

fn default_model_load_mode() -> String {
    "on_start".to_string()
}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            api_key: String::new(),
            source_lang: "Auto".to_string(),
            target_lang: "English".to_string(),
            #[cfg(target_os = "macos")]
            hotkey: "RightOption".to_string(),
            #[cfg(not(target_os = "macos"))]
            hotkey: "Control+Shift+Space".to_string(),
            translation_mode: "hotkey".to_string(),
            translate_hotkey: String::new(),
            sound_start: true,
            sound_stop: true,
            sound_error: true,
            auto_paste: true,
            keep_clipboard: false,
            show_hints: true,
            no_speech_alert_sec: default_no_speech_alert_sec(),
            storage_mode: "text_only".to_string(),
            storage_cap_mb: 500,
            retention_days: 30,
            model_name: crate::model_download::PARAKEET_MODEL.to_string(),
            input_device: None,
            output_device: None,
            theme: "system".to_string(),
            pill_style: default_pill_style(),
            bar_scale: default_bar_scale(),
            onboarding_done: false,
            model_idle_unload_sec: default_idle_unload_sec(),
            model_load_mode: default_model_load_mode(),
            pill_center: None,
            // The "Natural speech" text style (see TextTab's PRESETS).
            text_normalization: true,
            remove_filler_words: true,
            convert_numbers: false,
            remove_stutters: true,
            apply_self_corrections: false,
            remove_noise_markers: true,
            collapse_redundancy: false,
            annotate_ambiguity: false,
            normalize_structured_values: true,
            translation_provider: default_translation_provider(),
            apple_fallback_provider: String::new(),
            apple_translation_model: default_apple_translation_model(),
            device_backend: "auto".to_string(),
            parakeet_device: default_parakeet_device(),
            audio_chunking: false,
            noise_reduction: default_noise_reduction(),
            mask_confidential: false,
            mask_words: String::new(),
            mask_threshold: 90,
            mask_format: "***".to_string(),
            mask_emails: true,
            mask_phones: true,
            mask_cards: true,
            excluded_languages: "Russian".to_string(),
            llm_endpoint: default_llm_endpoint(),
            llm_model: default_llm_model(),
            llm_api_key: String::new(),
            local_llm_url: default_local_url(),
            local_llm_model: default_local_model(),
            local_llm_api_key: String::new(),
            custom_api_url: default_custom_url(),
            custom_api_key: String::new(),
            custom_api_model: default_custom_model(),
            prompt_template: default_prompt_template(),
            last_version: None,
        }
    }
}

pub fn get_config_dir() -> PathBuf {
    if let Some(base) = dirs::config_dir() {
        base.join("bifaria")
    } else {
        PathBuf::from(".bifaria")
    }
}

fn default_noise_reduction() -> String {
    "off".to_string()
}

/// The "Don't save" storage mode: nothing about a dictation is written to disk.
pub fn is_private(storage_mode: &str) -> bool {
    storage_mode == "private" || storage_mode == "private_mode"
}

pub fn get_data_dir() -> PathBuf {
    if let Some(base) = dirs::data_dir() {
        base.join("bifaria")
    } else {
        PathBuf::from(".bifaria-data")
    }
}

/// Bifaria started as RevFly. On its first run it copies RevFly's settings, history, recordings and
/// models, so nothing has to be set up or downloaded again. RevFly's own folders are left as they are.
pub fn import_from_revfly() {
    for (base, new) in [(dirs::config_dir(), get_config_dir()), (dirs::data_dir(), get_data_dir())] {
        let Some(old) = base.map(|b| b.join("revfly")) else { continue };
        if let Err(e) = import_dir(&old, &new) {
            eprintln!("Couldn't import RevFly's files from {}: {e}", old.display());
        }
    }
}

/// Copies `old` to `new` unless `new` already exists. It copies into a temporary folder first, so an
/// interrupted copy is tried again on the next launch instead of leaving half the files.
fn import_dir(old: &Path, new: &Path) -> std::io::Result<bool> {
    if new.exists() || !old.is_dir() {
        return Ok(false);
    }
    let tmp = new.with_extension("importing");
    let _ = fs::remove_dir_all(&tmp);
    copy_dir(old, &tmp).inspect_err(|_| {
        let _ = fs::remove_dir_all(&tmp);
    })?;
    fs::rename(&tmp, new)?;
    Ok(true)
}

/// Recursive copy. On macOS `fs::copy` clones files on APFS, so large models take no extra space.
fn copy_dir(from: &Path, to: &Path) -> std::io::Result<()> {
    fs::create_dir_all(to)?;
    for entry in fs::read_dir(from)? {
        let entry = entry?;
        let kind = entry.file_type()?;
        let target = to.join(entry.file_name());
        if kind.is_dir() {
            copy_dir(&entry.path(), &target)?;
        } else if kind.is_file() {
            fs::copy(entry.path(), &target)?;
        }
    }
    Ok(())
}

/// Settings saved before 1.0.5 had one `sound_effect` switch for every sound; it becomes the
/// value of all three sound toggles. Returns whether anything changed.
fn split_sound_toggle(json: &mut serde_json::Value) -> bool {
    let Some(obj) = json.as_object_mut() else {
        return false;
    };
    let Some(old) = obj.remove("sound_effect") else {
        return false;
    };
    let on = old.as_bool().unwrap_or(true);
    for key in ["sound_start", "sound_stop", "sound_error"] {
        obj.entry(key).or_insert(serde_json::Value::Bool(on));
    }
    true
}

impl AppSettings {
    pub fn translation_off(&self) -> bool {
        self.translation_mode == "off"
    }

    pub fn auto_translates(&self) -> bool {
        self.translation_mode == "auto"
    }

    /// The translate hotkey to register: empty unless the mode uses it.
    pub fn active_translate_hotkey(&self) -> &str {
        if self.translation_mode == "hotkey" {
            self.translate_hotkey.trim()
        } else {
            ""
        }
    }

    /// Fills in `translation_mode` for settings saved before it existed, keeping what the user
    /// had: "No Translation" as the provider → off; a translate hotkey → hotkey; otherwise the
    /// main hotkey translated automatically → auto. The provider no longer turns translation off,
    /// so a "No Translation" provider is reset to the default one. Returns whether anything changed.
    pub fn migrate(&mut self) -> bool {
        let provider = self.translation_provider.to_lowercase();
        let provider_off = provider.contains("no translation") || provider == "none";
        if !matches!(self.translation_mode.as_str(), "off" | "hotkey" | "auto") {
            self.translation_mode = if provider_off {
                "off"
            } else if !self.translate_hotkey.trim().is_empty() {
                "hotkey"
            } else {
                "auto"
            }
            .to_string();
        } else if !provider_off {
            return false;
        }
        if provider_off {
            self.translation_provider = default_translation_provider();
        }
        true
    }

    /// Whether to show What's New: the first launch after an update. A new install sets
    /// `last_version` when the welcome guide closes, so it never sees it.
    pub fn should_show_whats_new(&self) -> bool {
        self.onboarding_done && self.last_version.as_deref() != Some(env!("CARGO_PKG_VERSION"))
    }

    pub fn load() -> Self {
        Self::load_from(&get_config_dir().join("settings.json"))
    }

    fn load_from(settings_path: &Path) -> Self {
        if let Ok(content) = fs::read_to_string(settings_path) {
            let parsed = serde_json::from_str::<serde_json::Value>(&content).ok().and_then(|mut json| {
                let split_sounds = split_sound_toggle(&mut json);
                serde_json::from_value::<AppSettings>(json).ok().map(|s| (s, split_sounds))
            });
            if let Some((mut settings, split_sounds)) = parsed {
                if settings.migrate() | split_sounds {
                    let _ = settings.save_to(settings_path);
                }
                return settings;
            }
            // Kept aside instead of overwritten by the defaults below, so keys and hotkeys can be recovered.
            log::error!("settings.json could not be read; moved to settings.json.bad and started with defaults");
            let _ = fs::rename(settings_path, settings_path.with_extension("json.bad"));
        }

        let default_settings = Self::default();
        let _ = default_settings.save_to(settings_path);
        default_settings
    }

    pub fn save(&self) -> Result<(), String> {
        self.save_to(&get_config_dir().join("settings.json"))
    }

    /// Writes a temporary file and renames it over the old one, so a crash mid-write never leaves
    /// a half-written settings file.
    fn save_to(&self, settings_path: &Path) -> Result<(), String> {
        if let Some(dir) = settings_path.parent() {
            let _ = fs::create_dir_all(dir);
        }
        let json = serde_json::to_string_pretty(self).map_err(|e| e.to_string())?;
        let tmp = settings_path.with_extension("json.tmp");
        fs::write(&tmp, json).map_err(|e| e.to_string())?;
        fs::rename(&tmp, settings_path).map_err(|e| e.to_string())?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::AppSettings;

    #[test]
    fn revfly_files_are_copied_once_and_kept() {
        let root = std::env::temp_dir().join(format!("bifaria_import_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let (old, new) = (root.join("revfly"), root.join("bifaria"));
        std::fs::create_dir_all(old.join("models")).unwrap();
        std::fs::write(old.join("settings.json"), "{}").unwrap();
        std::fs::write(old.join("models/m.bin"), "weights").unwrap();

        assert!(super::import_dir(&old, &new).unwrap());
        assert_eq!(std::fs::read_to_string(new.join("models/m.bin")).unwrap(), "weights");
        assert!(old.join("settings.json").exists(), "RevFly's files stay");
        assert!(!new.with_extension("importing").exists());

        std::fs::write(new.join("settings.json"), "{\"changed\":true}").unwrap();
        assert!(!super::import_dir(&old, &new).unwrap(), "an existing Bifaria folder is never overwritten");
        assert!(std::fs::read_to_string(new.join("settings.json")).unwrap().contains("changed"));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn defaults_send_nothing_to_a_cloud_service() {
        let s = AppSettings::default();
        assert!(matches!(s.translation_provider.as_str(), "Apple" | "Offline"));
        assert!(s.llm_endpoint.starts_with("http://localhost"), "the LLM default is local");
        assert!(s.custom_api_url.is_empty(), "no remote Custom API address by default");
    }

    fn legacy(provider: &str, translate_hotkey: &str) -> AppSettings {
        let mut s = AppSettings::default();
        s.translation_mode = String::new();
        s.translation_provider = provider.to_string();
        s.translate_hotkey = translate_hotkey.to_string();
        s
    }

    #[test]
    fn migrate_keeps_what_the_user_had() {
        let mut off = legacy("No Translation", "F5");
        assert!(off.migrate());
        assert_eq!(off.translation_mode, "off");
        assert_eq!(off.translation_provider, super::default_translation_provider());
        assert_eq!(off.active_translate_hotkey(), "");

        let mut hotkey = legacy("LLM", "F5");
        assert!(hotkey.migrate());
        assert_eq!(hotkey.translation_mode, "hotkey");
        assert_eq!(hotkey.active_translate_hotkey(), "F5");

        let mut auto = legacy("Custom API", "");
        assert!(auto.migrate());
        assert_eq!(auto.translation_mode, "auto");
        assert_eq!(auto.translation_provider, "Custom API");

        assert!(!auto.migrate(), "already migrated");
    }

    #[test]
    fn the_old_sound_switch_sets_all_three_sounds() {
        for on in [true, false] {
            let mut saved = serde_json::to_value(AppSettings::default()).unwrap();
            let obj = saved.as_object_mut().unwrap();
            for key in ["sound_start", "sound_stop", "sound_error"] {
                obj.remove(key);
            }
            obj.insert("sound_effect".into(), serde_json::Value::Bool(on));
            assert!(super::split_sound_toggle(&mut saved));
            let s: AppSettings = serde_json::from_value(saved.clone()).unwrap();
            assert_eq!((s.sound_start, s.sound_stop, s.sound_error), (on, on, on));
            assert!(!super::split_sound_toggle(&mut saved), "already split");
        }
    }

    #[test]
    fn new_installs_translate_only_with_the_translate_hotkey() {
        let s = AppSettings::default();
        assert_eq!(s.translation_mode, "hotkey");
        assert!(!s.auto_translates());
    }

    #[test]
    fn only_new_installs_see_the_welcome_guide() {
        assert!(!AppSettings::default().onboarding_done);

        let mut saved = serde_json::to_value(AppSettings::default()).unwrap();
        saved.as_object_mut().unwrap().remove("onboarding_done");
        let upgraded: AppSettings = serde_json::from_value(saved).unwrap();
        assert!(upgraded.onboarding_done, "settings from before the guide count as done");
    }

    #[test]
    fn test_should_show_whats_new() {
        let mut s = AppSettings::default();
        // New install, welcome guide still open.
        assert!(!s.should_show_whats_new());
        // Updated from an older version.
        s.onboarding_done = true;
        s.last_version = Some("1.0.7".to_string());
        assert!(s.should_show_whats_new());
        // Seen.
        s.last_version = Some(env!("CARGO_PKG_VERSION").to_string());
        assert!(!s.should_show_whats_new());
    }

    #[test]
    fn unreadable_settings_are_kept_aside_and_saves_are_whole() {
        let dir = std::env::temp_dir().join(format!("bifaria_settings_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("settings.json");
        std::fs::write(&path, "{\"model_name\": 5}").unwrap();

        let loaded = AppSettings::load_from(&path);
        assert_eq!(std::fs::read_to_string(dir.join("settings.json.bad")).unwrap(), "{\"model_name\": 5}");
        assert_eq!(loaded.model_name, AppSettings::default().model_name);

        let mut changed = loaded;
        changed.target_lang = "German".to_string();
        changed.save_to(&path).unwrap();
        assert!(!dir.join("settings.json.tmp").exists());
        assert_eq!(AppSettings::load_from(&path).target_lang, "German");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
