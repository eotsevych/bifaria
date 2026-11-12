use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tauri::{AppHandle, Emitter, Manager};

use crate::audio::AudioRecorder;
use crate::history::{HistoryManager, HistoryRecord};
use crate::paste::copy_and_paste;
use crate::pipeline_logger::{current_timestamp, log_stage_event, PipelineLogManager, TranscriptionDiagnosticLog};
use crate::settings::{get_data_dir, AppSettings};
use crate::transcribe::Transcriber;
use crate::translate::{normalize_lang, should_skip};
use crate::vad::trim_silence;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AssistantPhase {
    Idle,
    Listening,
    Transcribing,
    Translating,
    Done,
    Error,
}

impl AssistantPhase {
    pub fn as_str(&self) -> &'static str {
        match self {
            AssistantPhase::Idle => "idle",
            AssistantPhase::Listening => "listening",
            AssistantPhase::Transcribing => "transcribing",
            AssistantPhase::Translating => "translating",
            AssistantPhase::Done => "done",
            AssistantPhase::Error => "error",
        }
    }
}

fn hide_main_window(app_handle: &AppHandle) {
    let app = app_handle.clone();
    let _ = app_handle.run_on_main_thread(move || {
        // Gives the window the panel's frame back on macOS, so a drag there is read below.
        crate::fullscreen_pill::release();
        if let Some(win) = app.get_webview_window("main") {
            if crate::pill_position::take_dragged() {
                remember_pill_spot(&app, &win);
            }
            let _ = win.hide();
        }
    });
}

/// Saves where the user dragged the pill, so it shows there from now on, on every monitor.
fn remember_pill_spot(app: &AppHandle, win: &tauri::WebviewWindow) {
    let (Some(center), Some(state)) = (crate::pill_position::current_center(win), app.try_state::<crate::AppState>())
    else {
        return;
    };
    let mut settings = state.controller.get_settings();
    settings.pill_center = Some(center);
    if let Err(e) = state.controller.save_settings(settings) {
        log::warn!("Could not save the pill's position: {}", e);
    }
}

fn get_active_monitor(win: &tauri::WebviewWindow) -> Option<tauri::Monitor> {
    let monitors = win.available_monitors().ok()?;
    if monitors.is_empty() {
        return None;
    }
    if monitors.len() == 1 {
        return monitors.into_iter().next();
    }

    #[cfg(target_os = "macos")]
    unsafe {
        use std::ffi::{c_char, c_void, CStr};
        #[link(name = "AppKit", kind = "framework")]
        extern "C" {
            fn objc_msgSend(receiver: *mut c_void, sel: *const c_void, ...) -> *mut c_void;
            fn sel_registerName(str: *const u8) -> *const c_void;
            fn objc_getClass(str: *const u8) -> *mut c_void;
        }

        #[repr(C)]
        #[derive(Copy, Clone, Debug)]
        struct NSPoint {
            x: f64,
            y: f64,
        }
        #[repr(C)]
        #[derive(Copy, Clone, Debug)]
        struct NSSize {
            width: f64,
            height: f64,
        }
        #[repr(C)]
        #[derive(Copy, Clone, Debug)]
        struct NSRect {
            origin: NSPoint,
            size: NSSize,
        }

        let nsevent_cls = objc_getClass(b"NSEvent\0".as_ptr());
        let nsscreen_cls = objc_getClass(b"NSScreen\0".as_ptr());

        if !nsevent_cls.is_null() && !nsscreen_cls.is_null() {
            let sel_mouse = sel_registerName(b"mouseLocation\0".as_ptr());
            let mouse_fn: extern "C" fn(*mut c_void, *const c_void) -> NSPoint =
                std::mem::transmute(objc_msgSend as *const ());
            let mouse = mouse_fn(nsevent_cls, sel_mouse);

            let sel_screens = sel_registerName(b"screens\0".as_ptr());
            let sel_count = sel_registerName(b"count\0".as_ptr());
            let sel_object_at_index = sel_registerName(b"objectAtIndex:\0".as_ptr());
            let sel_frame = sel_registerName(b"frame\0".as_ptr());
            let sel_name = sel_registerName(b"localizedName\0".as_ptr());
            let sel_utf8 = sel_registerName(b"UTF8String\0".as_ptr());

            let screen_array = objc_msgSend(nsscreen_cls, sel_screens);
            if !screen_array.is_null() {
                let count = objc_msgSend(screen_array, sel_count) as usize;
                let frame_fn: extern "C" fn(*mut c_void, *const c_void) -> NSRect =
                    std::mem::transmute(objc_msgSend as *const ());
                // Through a non-variadic signature: on Apple Silicon a variadic argument goes on the
                // stack, objc_msgSend reads it from a register, and the index came out as garbage
                // (an out-of-range crash whenever a second display was connected).
                let object_at_index: extern "C" fn(*mut c_void, *const c_void, usize) -> *mut c_void =
                    std::mem::transmute(objc_msgSend as *const ());

                for i in 0..count {
                    let screen = object_at_index(screen_array, sel_object_at_index, i);
                    if !screen.is_null() {
                        let frame = frame_fn(screen, sel_frame);
                        let in_rect = mouse.x >= frame.origin.x
                            && mouse.x <= (frame.origin.x + frame.size.width)
                            && mouse.y >= frame.origin.y
                            && mouse.y <= (frame.origin.y + frame.size.height);

                        if in_rect {
                            let ns_name = objc_msgSend(screen, sel_name);
                            if !ns_name.is_null() {
                                let c_str_ptr = objc_msgSend(ns_name, sel_utf8) as *const c_char;
                                if !c_str_ptr.is_null() {
                                    if let Ok(name_str) = CStr::from_ptr(c_str_ptr).to_str() {
                                        if let Some(m) = monitors.iter().find(|m| m.name().map(|n| n == name_str).unwrap_or(false)) {
                                            return Some(m.clone());
                                        }
                                    }
                                }
                            }
                            if i < monitors.len() {
                                return Some(monitors[i].clone());
                            }
                        }
                    }
                }
            }

            // Fallback: NSScreen.mainScreen
            let sel_main = sel_registerName(b"mainScreen\0".as_ptr());
            let main_screen = objc_msgSend(nsscreen_cls, sel_main);
            if !main_screen.is_null() {
                let ns_name = objc_msgSend(main_screen, sel_name);
                if !ns_name.is_null() {
                    let c_str_ptr = objc_msgSend(ns_name, sel_utf8) as *const c_char;
                    if !c_str_ptr.is_null() {
                        if let Ok(name_str) = CStr::from_ptr(c_str_ptr).to_str() {
                            if let Some(m) = monitors.iter().find(|m| m.name().map(|n| n == name_str).unwrap_or(false)) {
                                return Some(m.clone());
                            }
                        }
                    }
                }
            }
        }
    }

    if let Ok(cursor) = win.cursor_position() {
        for m in &monitors {
            let pos = m.position();
            let size = m.size();
            if cursor.x >= pos.x as f64
                && cursor.x <= (pos.x + size.width as i32) as f64
                && cursor.y >= pos.y as f64
                && cursor.y <= (pos.y + size.height as i32) as f64
            {
                return Some(m.clone());
            }
        }
    }

    win.current_monitor().ok().flatten().or_else(|| win.primary_monitor().ok().flatten())
}

fn show_main_window(app_handle: &AppHandle, settings: &AppSettings) {
    let app = app_handle.clone();
    let center = settings.pill_center;
    let _ = app_handle.run_on_main_thread(move || {
        if let Some(win) = app.get_webview_window("main") {
            // A previous long error may have enlarged the pill window.
            let _ = win.set_size(tauri::Size::Logical(tauri::LogicalSize { width: PILL_WINDOW.0, height: PILL_WINDOW.1 }));
            // At its spot on the monitor with the cursor (see pill_position).
            if let Some(monitor) = get_active_monitor(&win) {
                crate::pill_position::move_onto(&win, &monitor, PILL_WINDOW, center);
            }

            let _ = win.show();
            let _ = win.set_shadow(false);
            apply_pill_window_behavior(&win, true);
        }
    });
}

/// Makes the voice pill float above full-screen apps on every Space. The pill never takes keyboard
/// focus (so the paste lands in the app the user was typing in) because the window is created with
/// `"focusable": false` in tauri.conf.json. Don't toggle that at runtime: on Windows it rewrites the
/// window's extended style, which can leave the transparent WebView2 pill unpainted.
///
/// The window must stay the NSWindow subclass tao created: swapping its class (e.g. to NSPanel) breaks
/// WebKit's KVO observers and aborts the app on macOS 27 when the view hierarchy is rebuilt.
pub(crate) fn apply_pill_window_behavior(win: &tauri::WebviewWindow, order_front: bool) {
    #[cfg(not(target_os = "macos"))]
    let _ = (win, order_front);

    #[cfg(target_os = "macos")]
    unsafe {
        use std::ffi::c_void;
        #[link(name = "AppKit", kind = "framework")]
        extern "C" {
            fn objc_msgSend(receiver: *mut c_void, sel: *const c_void, ...) -> *mut c_void;
            fn sel_registerName(str: *const u8) -> *const c_void;
        }

        // objc_msgSend must be called through a non-variadic signature: on Apple Silicon variadic
        // arguments go on the stack, but the callee reads them from registers.
        let send_usize: extern "C" fn(*mut c_void, *const c_void, usize) =
            std::mem::transmute(objc_msgSend as *const ());
        let send_isize: extern "C" fn(*mut c_void, *const c_void, isize) =
            std::mem::transmute(objc_msgSend as *const ());

        if let Ok(ns_win) = win.ns_window() {
            let ptr = ns_win as *mut c_void;

            // NSWindowCollectionBehaviorCanJoinAllSpaces (1) | IgnoresCycle (64) | FullScreenAuxiliary (256)
            let behavior: usize = (1 << 0) | (1 << 6) | (1 << 8);
            send_usize(ptr, sel_registerName(b"setCollectionBehavior:\0".as_ptr()), behavior);

            // NSScreenSaverWindowLevel (1000) floats directly above full-screen and active apps
            send_isize(ptr, sel_registerName(b"setLevel:\0".as_ptr()), 1000);

            if order_front {
                // A native full-screen app's Space never composites this window no matter the
                // collection behavior above, so the pill is shown from an NSPanel instead (see
                // fullscreen_pill.rs).
                crate::fullscreen_pill::adopt(win);
            }
        } else {
            log::warn!("Pill window: ns_window() failed, could not apply full-screen collection behavior");
        }
    }
}

/// What Audio Lab's "Translate Again" got back from the translation service.
#[derive(Debug, Clone, serde::Serialize)]
pub struct LabTranslation {
    pub text: String,
    pub ms: u64,
    /// Service, model and endpoint, e.g. `LLM llama3.2 at http://localhost:11434/…`.
    pub service: String,
}

/// Longest a translation request may take before it counts as failed.
const TRANSLATION_TIMEOUT: Duration = Duration::from_secs(20);

/// The translation service a recording goes to, resolved from settings. Shared by the pipeline and
/// the "Retry translation" action so both send text the same way.
struct TranslationRoute {
    is_custom: bool,
    /// Apple's on-device Translation (see `apple_translate`); the LLM fields are then unused.
    is_apple: bool,
    /// Apple Intelligence's on-device LLM: takes the prompt template, but no endpoint, key or model.
    is_intelligence: bool,
    /// Firefox Translations' on-device models (see `offline_translate`); like Apple's, no fields.
    is_offline: bool,
    endpoint: String,
    key: String,
    model: String,
}

/// A finished translation and the service that produced it.
struct Translated {
    text: String,
    /// Service for logs, like `TranslationRoute::describe`; the fallback's when it answered.
    via: String,
    /// Short service name for the pill and the performance log: "LLM", "Custom API" or "Apple".
    label: &'static str,
    /// Languages as codes, e.g. "uk → en"; "auto → en" when the spoken language wasn't known.
    pair: Option<String>,
    /// The model that translated, for the performance log: the LLM's name, or Apple's "fast" or
    /// "quality".
    model: Option<String>,
}

/// The languages as codes ("uk → en"). The spoken one is the detected one while it's set to Auto.
fn language_pair(settings: &AppSettings, from_lang: &str) -> Option<String> {
    let target = crate::apple_translate::to_bcp47(&settings.target_lang)?;
    let source = crate::apple_translate::source_code(&settings.source_lang, from_lang).unwrap_or_else(|| "auto".to_string());
    Some(format!("{} → {}", source, target))
}

/// The translation settings that are still empty, e.g. `["API key"]`. It checks only that the
/// fields are filled in; whether the service is reachable is found out when it's called. Apple's
/// provider has no fields; see `apple_setup_problem`.
fn missing_translation_settings(settings: &AppSettings) -> Vec<&'static str> {
    let route = TranslationRoute::from_settings(settings);
    if route.is_apple || route.is_intelligence || route.is_offline {
        return Vec::new();
    }
    let (url, model) = if route.is_custom {
        (settings.custom_api_url.trim(), route.model.as_str())
    } else {
        (settings.llm_endpoint.trim(), settings.llm_model.trim())
    };
    let key = if route.is_custom { settings.custom_api_key.trim() } else { route.key.as_str() };
    let mut missing = Vec::new();
    if url.is_empty() {
        missing.push("endpoint URL");
    }
    if key.is_empty() && !crate::settings::is_local_endpoint(url) {
        missing.push("API key");
    }
    if model.is_empty() {
        missing.push("model name");
    }
    missing
}

/// With the Apple provider: what is known to keep this recording from being translated (an old
/// macOS, a language pair Apple doesn't support), unless a fallback service is
/// set up to translate instead. `None` for the other providers.
/// The same for the Offline provider (a language pair that isn't downloaded).
fn apple_setup_problem(settings: &AppSettings) -> Option<String> {
    let route = TranslationRoute::from_settings(settings);
    if !(route.is_apple || route.is_offline) || route.fallback(settings).is_some() {
        return None;
    }
    if route.is_offline {
        return crate::offline_translate::setup_problem(&settings.source_lang, &settings.target_lang);
    }
    crate::apple_translate::setup_problem(&settings.source_lang, &settings.target_lang)
}

/// "the API key", "the API key and model name", "the endpoint URL, API key and model name".
fn list_missing(missing: &[&str]) -> String {
    match missing {
        [] => String::new(),
        [one] => format!("the {one}"),
        [rest @ .., last] => format!("the {} and {}", rest.join(", "), last),
    }
}

impl TranslationRoute {
    fn from_settings(settings: &AppSettings) -> Self {
        let is_apple = crate::apple_translate::is_apple(&settings.translation_provider);
        let is_intelligence = crate::apple_translate::is_apple_intelligence(&settings.translation_provider);
        let is_offline = crate::offline_translate::is_offline(&settings.translation_provider);
        // HIDDEN: Custom API, not offered in the settings UI; a saved one still routes here.
        let is_custom = !is_apple && settings.translation_provider.to_lowercase().contains("custom");
        let endpoint = if !settings.llm_endpoint.trim().is_empty() {
            settings.llm_endpoint.trim().to_string()
        } else if !settings.local_llm_url.trim().is_empty() {
            settings.local_llm_url.trim().to_string()
        } else {
            String::new()
        };
        let key = if !settings.llm_api_key.trim().is_empty() {
            settings.llm_api_key.trim().to_string()
        } else if !settings.api_key.trim().is_empty() {
            settings.api_key.trim().to_string()
        } else {
            settings.local_llm_api_key.trim().to_string()
        };
        let model = if is_custom {
            settings.custom_api_model.trim().to_string()
        } else if !settings.llm_model.trim().is_empty() {
            settings.llm_model.trim().to_string()
        } else {
            settings.local_llm_model.trim().to_string()
        };
        Self { is_custom, is_apple, is_intelligence, is_offline, endpoint, key, model }
    }

    fn label(&self) -> &'static str {
        if self.is_apple {
            "Apple"
        } else if self.is_intelligence {
            "Apple Intelligence"
        } else if self.is_offline {
            "Offline"
        } else if self.is_custom {
            "Custom API"
        } else {
            "LLM"
        }
    }

    /// One-line description for logs, e.g. `LLM llama3.2 at http://localhost:11434/…`.
    fn describe(&self, settings: &AppSettings) -> String {
        if self.is_apple {
            return "Apple Translation (on-device)".to_string();
        }
        if self.is_intelligence {
            return "Apple Intelligence (on-device)".to_string();
        }
        if self.is_offline {
            return "Offline translation (on-device)".to_string();
        }
        let url = if self.is_custom { settings.custom_api_url.trim() } else { self.endpoint.as_str() };
        format!("{} {} at {}", self.label(), self.model, url)
    }

    /// Whether the service can be called at all: Apple's needs macOS 26 and its helper (or a
    /// fallback service), the others their fields filled in.
    fn is_ready(&self, settings: &AppSettings) -> bool {
        if self.is_apple {
            crate::apple_translate::is_available() || self.fallback(settings).is_some()
        } else if self.is_intelligence {
            crate::apple_translate::is_available()
        } else if self.is_offline {
            true
        } else {
            missing_translation_settings(settings).is_empty()
        }
    }

    /// The service that translates when Apple Translation (or the Offline provider) fails, if one
    /// is chosen and filled in.
    fn fallback(&self, settings: &AppSettings) -> Option<TranslationRoute> {
        if !(self.is_apple || self.is_offline) {
            return None;
        }
        let provider = settings.apple_fallback_provider.trim();
        if provider.is_empty() || crate::apple_translate::is_apple(provider) || crate::offline_translate::is_offline(provider) {
            return None;
        }
        let mut with_fallback = settings.clone();
        with_fallback.translation_provider = provider.to_string();
        missing_translation_settings(&with_fallback)
            .is_empty()
            .then(|| TranslationRoute::from_settings(&with_fallback))
    }

    /// Translates `text`, spoken in `from_lang` (the detected language; Apple's provider uses it
    /// when the spoken language is set to Auto, the LLMs get the configured one in their prompt).
    async fn translate(&self, settings: &AppSettings, text: &str, from_lang: &str) -> Result<Translated, String> {
        if !(self.is_apple || self.is_offline) {
            let text = self.translate_llm(settings, text).await?;
            let model = (!self.is_intelligence).then(|| self.model.clone());
            return Ok(Translated { text, via: self.describe(settings), label: self.label(), pair: language_pair(settings, from_lang), model });
        }
        let first = if self.is_offline {
            self.translate_offline(settings, text, from_lang).await
        } else {
            self.translate_apple(settings, text, from_lang).await
        };
        let reason = match first {
            Ok(done) => return Ok(done),
            Err(reason) => reason,
        };
        let Some(fallback) = self.fallback(settings) else {
            return Err(reason);
        };
        log_stage_event(
            &get_data_dir(),
            "TRANSLATE_FALLBACK",
            &format!("{}; translating with {} instead", reason, fallback.describe(settings)),
        );
        match fallback.translate_llm(settings, text).await {
            Ok(text) => Ok(Translated {
                text,
                via: format!("{} (after {} failed: {})", fallback.describe(settings), self.describe(settings), reason),
                label: fallback.label(),
                pair: language_pair(settings, from_lang),
                model: Some(fallback.model.clone()),
            }),
            Err(e) => Err(format!("{}; {} failed too: {}", reason, fallback.label(), e)),
        }
    }

    async fn translate_apple(&self, settings: &AppSettings, text: &str, from_lang: &str) -> Result<Translated, String> {
        let source = crate::apple_translate::source_code(&settings.source_lang, from_lang);
        let Some(target) = crate::apple_translate::to_bcp47(&settings.target_lang) else {
            return Err(format!("Apple Translation can't translate into {}", settings.target_lang));
        };
        let model = crate::apple_translate::model_name(&settings.apple_translation_model);
        match crate::apple_translate::translate(text, source.as_deref(), &target, model, TRANSLATION_TIMEOUT).await {
            Ok(done) => {
                let pair = format!("{} → {}", done.source, target);
                Ok(Translated {
                    text: done.text,
                    via: format!("{} {}", self.describe(settings), pair),
                    label: self.label(),
                    pair: Some(pair),
                    model: Some(settings.apple_translation_model.clone()),
                })
            }
            Err(e) => Err(e.describe(&crate::apple_translate::pair_name(source.as_deref(), &target))),
        }
    }

    async fn translate_offline(&self, settings: &AppSettings, text: &str, from_lang: &str) -> Result<Translated, String> {
        let Some(target) = crate::apple_translate::to_bcp47(&settings.target_lang) else {
            return Err(format!("Offline translation can't translate into {}", settings.target_lang));
        };
        // The models can't detect the language; the transcriber's detected one stands in for Auto.
        let Some(source) = crate::apple_translate::source_code(&settings.source_lang, from_lang) else {
            return Err("Offline translation needs to know the spoken language; set it in Settings → Transcription".to_string());
        };
        let text = crate::offline_translate::translate(text, &source, &target, TRANSLATION_TIMEOUT).await?;
        let pair = format!("{} → {}", source, target);
        Ok(Translated { text, via: format!("{} {}", self.describe(settings), pair), label: self.label(), pair: Some(pair), model: None })
    }

    async fn translate_llm(&self, settings: &AppSettings, text: &str) -> Result<String, String> {
        if self.is_intelligence {
            let prompt = crate::translate::render_prompt_template(&settings.prompt_template, text, &settings.source_lang, &settings.target_lang);
            return crate::apple_translate::generate(&prompt, TRANSLATION_TIMEOUT).await;
        }
        let request = async {
            // HIDDEN: Custom API branch (see `is_custom`).
            if self.is_custom {
                crate::translate::translate_with_openai_compatible(
                    text,
                    &settings.custom_api_url,
                    &settings.custom_api_key,
                    &settings.source_lang,
                    &settings.target_lang,
                    &self.model,
                    &settings.prompt_template,
                )
                .await
            } else {
                crate::translate::translate_with_openai_compatible(
                    text,
                    &self.endpoint,
                    &self.key,
                    &settings.source_lang,
                    &settings.target_lang,
                    &self.model,
                    &settings.prompt_template,
                )
                .await
            }
        };
        match tokio::time::timeout(TRANSLATION_TIMEOUT, request).await {
            Ok(result) => result,
            Err(_) => Err(format!("timed out after {} s", TRANSLATION_TIMEOUT.as_secs())),
        }
    }
}

/// A translation that failed, kept so the user can retry it from the pill or the tray menu.
#[derive(Clone)]
pub(crate) struct PendingRetry {
    /// Text that was sent for translation (already normalized and masked).
    text: String,
    from_lang: String,
    history_id: i64,
}

/// Entry point for the audio thread; routes a microphone problem to the running controller.
pub fn report_mic_problem(app_handle: &AppHandle, report: crate::audio::MicReport) {
    if let Some(state) = app_handle.try_state::<crate::AppState>() {
        state.controller.on_mic_problem(report);
    }
}

/// Entry point for the audio thread; no speech since the recording started, for `after`.
pub fn report_no_speech(app_handle: &AppHandle, after: Duration) {
    if let Some(state) = app_handle.try_state::<crate::AppState>() {
        state.controller.on_no_speech(after);
    }
}

/// Entry point for the audio thread; speech arrived after the no-speech warning.
pub fn report_speech_heard(app_handle: &AppHandle) {
    if let Some(state) = app_handle.try_state::<crate::AppState>() {
        state.controller.on_speech_heard();
    }
}

/// Entry point for the audio thread; audio is flowing again after a mid-recording dropout.
pub fn report_mic_recovered(app_handle: &AppHandle, switched_to: Option<String>) {
    if let Some(state) = app_handle.try_state::<crate::AppState>() {
        state.controller.on_mic_recovered(switched_to);
    }
}

/// While the welcome guide's practice run is open, the hotkeys drive its simulated recording
/// instead of a real one, so it works while the speech model is still downloading.
static PRACTICE_MODE: AtomicBool = AtomicBool::new(false);

pub fn set_practice_mode(on: bool) {
    PRACTICE_MODE.store(on, Ordering::SeqCst);
}

/// Which hotkey started the recording, which decides whether it gets translated.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RecordingMode {
    /// The main hotkey (or the app/tray): translates only in "auto" translation mode.
    Main,
    /// The translate hotkey: always translates to the target language (when it isn't already in it).
    Translate,
}

impl RecordingMode {
    /// True when this recording must not be translated.
    fn transcribe_only(self, settings: &AppSettings) -> bool {
        settings.translation_off() || (self == RecordingMode::Main && !settings.auto_translates())
    }

    /// The pill's `mode` while listening: "translate" (will be translated), "auto" (translated if
    /// not in the target language) or "transcribe" (never translated).
    fn pill_mode(self, settings: &AppSettings) -> &'static str {
        if self.transcribe_only(settings) {
            "transcribe"
        } else if self == RecordingMode::Translate {
            "translate"
        } else {
            "auto"
        }
    }
}

/// When and how the current recording started.
pub(crate) struct RecordingStart {
    at: Instant,
    trigger: &'static str,
    /// The microphone recording, and its sample rate.
    device: String,
    sample_rate: u32,
}

/// Captured audio shorter than the wall-clock recording by more than this means audio was dropped.
const CAPTURE_GAP_WARN_SEC: f32 = 0.5;

/// Logs how a recording ended and whether the audio matches it: wall-clock time between start and
/// stop against the audio actually captured, plus dead (digital silence) or invalid stretches.
fn log_recording_stop(recording_start: &Mutex<Option<RecordingStart>>, trigger: &str, stats: &crate::audio::CaptureStats) {
    let data_dir = get_data_dir();
    let Some(start) = recording_start.lock().unwrap().take() else {
        log_stage_event(&data_dir, "REC_STOP", &format!("Recording stopped by {}; captured {:.2} s (start time unknown)", trigger, stats.captured_sec));
        return;
    };
    let wall = start.at.elapsed().as_secs_f32();
    let missing = wall - stats.captured_sec;
    let dead = if stats.zero_sec >= 0.05 {
        format!(
            "; digital silence {:.2} s (longest {:.2} s starting at {:.2} s)",
            stats.zero_sec, stats.longest_zero_sec, stats.longest_zero_at_sec
        )
    } else {
        "; no digital silence".to_string()
    };
    let invalid = if stats.invalid_samples > 0 { format!("; {} invalid samples", stats.invalid_samples) } else { String::new() };
    log_stage_event(
        &data_dir,
        "REC_STOP",
        &format!(
            "Recording stopped by {} (started by {}): {:.2} s between start and stop, {:.2} s of audio captured ({:+.2} s){}{}",
            trigger, start.trigger, wall, stats.captured_sec, -missing, dead, invalid
        ),
    );
    if missing > CAPTURE_GAP_WARN_SEC {
        log_stage_event(&data_dir, "REC_GAP", &format!("{:.2} s of the recording never arrived from the microphone", missing));
        log::warn!("Recording lost {:.2} s of audio ({:.2} s wall, {:.2} s captured)", missing, wall, stats.captured_sec);
    }
    if stats.longest_zero_sec >= 1.0 || stats.invalid_samples > 0 {
        log::warn!("Recording had dead audio: {:?}", stats);
    }
}

/// Pill title and text for a microphone problem that started after real audio arrived.
fn mic_dropout_message(report: &crate::audio::MicReport) -> (&'static str, String) {
    use crate::audio::MicProblem;
    let device = &report.device;
    if report.reconnecting {
        return (
            "Reconnecting Mic…",
            format!("\"{device}\" stopped sending sound. Reopening it; what you said so far is kept."),
        );
    }
    match report.problem {
        MicProblem::Disconnected => ("Mic Disconnected", format!("\"{device}\" was disconnected. Press the hotkey to finish; what you said so far is kept.")),
        MicProblem::Stalled => ("Mic Stopped", format!("No audio from \"{device}\" for 2 s. Press the hotkey to finish; what you said so far is kept.")),
        _ => ("Mic Went Silent", format!("\"{device}\" sends only silence. Press the hotkey to finish; what you said so far is kept.")),
    }
}

/// Where the user fixes the microphone choice.
const MIC_SETTING: &str = "Settings → General → Microphone input";
/// How long an error that needs reading stays on screen.
const LONG_ERROR_HOLD: Duration = Duration::from_secs(9);
/// Recordings stop on their own after this long.
const MAX_RECORDING: Duration = Duration::from_secs(15 * 60);
/// How long the start chime waits for the mic's first audio before playing anyway.
const START_CHIME_MAX_WAIT: Duration = Duration::from_millis(1500);
/// How long the pill waits for a pick when the chosen microphone isn't connected.
const MIC_CHOICE_HOLD: Duration = Duration::from_secs(30);
/// Pill window size while it shows a long error, and its normal size.
const LONG_ERROR_WINDOW: (f64, f64) = (400.0, 150.0);
const PILL_WINDOW: (f64, f64) = (340.0, 100.0);
/// The tallest a long-message pill window may grow to fit its text.
const LONG_ERROR_MAX_HEIGHT: f64 = 360.0;

/// Grows (or shrinks) the long-message pill window to the height its content needs, so a
/// message with a long device name or several sentences isn't cut off.
pub(crate) fn fit_long_pill(app_handle: &AppHandle, content_height: f64) {
    if !content_height.is_finite() {
        return;
    }
    let height = content_height.clamp(LONG_ERROR_WINDOW.1, LONG_ERROR_MAX_HEIGHT);
    set_pill_window_size(app_handle, (LONG_ERROR_WINDOW.0, height));
}

/// Title and explanation shown in the pill for a microphone problem.
fn mic_problem_message(report: &crate::audio::MicReport) -> (&'static str, String) {
    use crate::audio::MicProblem;
    let device = &report.device;
    if let Some(missing) = &report.missing_device {
        let what = match report.problem {
            MicProblem::NoAudio | MicProblem::Silent => "sends no audio",
            MicProblem::Stalled | MicProblem::WentSilent | MicProblem::Disconnected => "stopped sending audio",
        };
        return (
            "Microphone Not Found",
            format!("\"{missing}\" isn't connected, and the default \"{device}\" {what}. Pick a working mic in {MIC_SETTING}."),
        );
    }
    match report.problem {
        MicProblem::NoAudio => (
            "Microphone Not Working",
            format!("No audio from \"{device}\" in the first 2 seconds. Check it's connected, or pick another mic in {MIC_SETTING}."),
        ),
        MicProblem::Silent => (
            "Microphone Is Silent",
            format!("\"{device}\" sends only silence. It may be muted, or mic access is blocked in privacy settings. Check {MIC_SETTING}."),
        ),
        MicProblem::Stalled | MicProblem::WentSilent => (
            "Microphone Stopped",
            format!("\"{device}\" stopped sending audio, maybe it was unplugged. Check {MIC_SETTING}."),
        ),
        MicProblem::Disconnected => (
            "Microphone Disconnected",
            format!("\"{device}\" was disconnected while recording. Reconnect it or pick another mic in {MIC_SETTING}."),
        ),
    }
}

fn set_pill_window_size(app_handle: &AppHandle, (width, height): (f64, f64)) {
    let app = app_handle.clone();
    let _ = app_handle.run_on_main_thread(move || {
        if let Some(win) = app.get_webview_window("main") {
            // Grows around its center and stays on screen, instead of off the right or bottom edge.
            crate::pill_position::reposition_for_size(&win, (width, height));
            let _ = win.set_size(tauri::Size::Logical(tauri::LogicalSize { width, height }));
            crate::fullscreen_pill::sync_frame(width, height);
        }
    });
}

/// Shows an error the user needs to read in a larger pill for `LONG_ERROR_HOLD`, then hides it,
/// unless a new recording (a new session) has started meanwhile.
fn show_long_error(app_handle: &AppHandle, session_id: &Arc<AtomicU64>, session: u64, title: &str, subtitle: &str) {
    show_long_error_payload(
        app_handle,
        session_id,
        session,
        serde_json::json!({ "state": "error", "title": title, "subtitle": subtitle, "long": true }),
    );
}

/// `show_long_error` with a full pill state payload (e.g. one that offers a Retry button).
fn show_long_error_payload(app_handle: &AppHandle, session_id: &Arc<AtomicU64>, session: u64, payload: serde_json::Value) {
    show_long_message(app_handle, session_id, session, payload, LONG_ERROR_HOLD);
}

/// Ends a recording that produced no text. Idle right away, so the hotkey records again while the
/// error is on screen; the error then hides itself unless a new recording has started.
fn fail_recording(app_handle: &AppHandle, phase: &Mutex<AssistantPhase>, session_id: &Arc<AtomicU64>, session: u64, title: &str, subtitle: &str) {
    *phase.lock().unwrap() = AssistantPhase::Idle;
    show_long_error(app_handle, session_id, session, title, subtitle);
}

/// Shows a long pill state for `hold`, then hides it unless a new session has started meanwhile.
fn show_long_message(app_handle: &AppHandle, session_id: &Arc<AtomicU64>, session: u64, payload: serde_json::Value, hold: Duration) {
    set_pill_window_size(app_handle, LONG_ERROR_WINDOW);
    let _ = app_handle.emit("assistant-state-changed", payload);
    let app = app_handle.clone();
    let session_id = Arc::clone(session_id);
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(hold).await;
        if session_id.load(Ordering::SeqCst) != session {
            return;
        }
        let _ = app.emit(
            "assistant-state-changed",
            serde_json::json!({ "state": "idle", "title": "Ready", "subtitle": null }),
        );
        hide_main_window(&app);
        set_pill_window_size(&app, PILL_WINDOW);
    });
}

fn mask_config_from(settings: &AppSettings) -> crate::masker::MaskConfig {
    crate::masker::MaskConfig {
        enabled: settings.mask_confidential,
        rules: crate::masker::parse_rules(&settings.mask_words, &settings.mask_format),
        threshold: settings.mask_threshold,
        default_mask: settings.mask_format.clone(),
        mask_emails: settings.mask_emails,
        mask_phones: settings.mask_phones,
        mask_cards: settings.mask_cards,
    }
}

fn capitalize(s: &str) -> String {
    let mut chars = s.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => String::new(),
    }
}

/// True when a recording carries no signal at all. A live microphone always has some noise floor, so
/// this means the OS blocked or muted the input (e.g. Windows microphone privacy settings).
/// The chunks a recording was transcribed in, or none when chunking is off and it went to the model whole.
fn chunk_diagnostics(
    chunking: bool,
    total_audio_sec: f32,
    vad: &crate::vad::VadResult,
    raw_samples_count: usize,
) -> (Vec<crate::pipeline_logger::ChunkDiagnosticEvent>, Vec<String>) {
    if chunking {
        crate::pipeline_logger::chunk_diagnostics(total_audio_sec, vad, raw_samples_count)
    } else {
        let actions = crate::pipeline_logger::whole_track_diagnostics(total_audio_sec, vad.trimmed_duration_sec, vad.silence_removed_sec, raw_samples_count);
        (Vec::new(), actions)
    }
}

/// Time allowed for one transcription: 30 s plus half the audio length, so long recordings fit.
fn inference_timeout(audio_sec: f32) -> Duration {
    Duration::from_secs_f32(30.0 + audio_sec.max(0.0) * 0.5)
}

/// Keeps a recording whose transcription failed: saves it to history and adds a Logs entry
/// pointing at the whole recording, so it can be played back and retried with Second Try.
#[allow(clippy::too_many_arguments)]
fn keep_failed_recording(
    history: &HistoryManager,
    logger: &PipelineLogManager,
    app_handle: &AppHandle,
    captured: &crate::audio::CapturedAudio,
    settings: &AppSettings,
    vad: &crate::vad::VadResult,
    duration_sec: f32,
    data_dir: &std::path::Path,
    session_audio: &Option<String>,
    pipeline_ms: u64,
    reason: &str,
) {
    let marker = format!("[{}]", reason);
    let history_id = history.add_entry(
        &marker,
        &marker,
        "unknown",
        &settings.target_lang,
        duration_sec,
        Some(captured),
        settings,
    );
    let history_id = history_id.ok().filter(|id| *id > 0);
    let (chunks, mut actions) = chunk_diagnostics(
        settings.audio_chunking,
        duration_sec,
        &vad,
        captured.speech.len(),
    );
    actions.push(format!("[FAILED] {}", reason));
    let log = TranscriptionDiagnosticLog {
        id: logger.next_id(),
        timestamp: current_timestamp(),
        audio_duration_sec: duration_sec,
        audio_samples_count: captured.speech.len(),
        vad_trim_ms: vad.duration_ms,
        vad_original_sec: vad.original_duration_sec,
        vad_trimmed_sec: vad.trimmed_duration_sec,
        vad_silence_removed_sec: vad.silence_removed_sec,
        model_name: settings.model_name.clone(),
        gpu_metal_active: Transcriber::is_metal_supported(),
        threads_count: 0,
        whisper_inference_ms: 0,
        whisper_speed_factor: 0.0,
        detected_lang: "unknown".to_string(),
        raw_text: marker,
        translation_skipped: true,
        translation_skip_reason: "Transcription failed".to_string(),
        translation_ms: 0,
        translation_error: None,
        translation_provider: None,
        final_text: String::new(),
        spoken_text: None,
        clipboard_paste_ms: 0,
        history_save_ms: 0,
        total_pipeline_ms: pipeline_ms,
        audio_filename: session_audio.clone(),
        vad_audio_filename: Some("latest_vad_trimmed.wav".to_string()),
        whisper_raw_output: Some(reason.to_string()),
        segments_count: 0,
        chunk_events: chunks,
        whole_track: !settings.audio_chunking,
        action_logs: actions,
        history_id,
        transcription_error: Some(reason.to_string()),
    };
    logger.add_log(log.clone(), data_dir);
    let _ = app_handle.emit("transcription-diagnostic-log", &log);
}

/// Whole recording of this session for the log's playback and Second Try; none in private mode.
fn session_audio_filename(data_dir: &std::path::Path, storage_mode: &str, audio: &crate::audio::CapturedAudio) -> Option<String> {
    if crate::settings::is_private(storage_mode) {
        return None;
    }
    crate::pipeline_logger::save_session_audio(data_dir, audio).or_else(|| Some("latest_recording.wav".to_string()))
}

fn is_digital_silence(samples: &[f32]) -> bool {
    samples.iter().all(|s| s.abs() < 1e-4)
}

/// Returns the model path for a recording that's waiting on it. If the model still has to download,
/// the pill shows the download progress and is put back to "Transcribing…" afterwards.
async fn ensure_model_for_recording(
    app_handle: &AppHandle,
    model_name: &str,
    transcribing_subtitle: &str,
) -> Result<std::path::PathBuf, String> {
    if crate::model_download::is_model_present(model_name) {
        return crate::model_download::ensure(app_handle, model_name).await;
    }
    let pill_wait = crate::model_download::PillWait::start(app_handle);
    let result = crate::model_download::ensure(app_handle, model_name).await;
    drop(pill_wait);
    let _ = app_handle.emit(
        "assistant-state-changed",
        serde_json::json!({
            "state": "transcribing",
            "title": "Transcribing…",
            "subtitle": transcribing_subtitle
        }),
    );
    result
}

pub struct AppController {
    app_handle: AppHandle,
    phase: Arc<Mutex<AssistantPhase>>,
    recorder: Arc<AudioRecorder>,
    transcriber: Arc<Mutex<Transcriber>>,
    parakeet_transcriber: Arc<Mutex<crate::parakeet::ParakeetTranscriber>>,
    history: Arc<HistoryManager>,
    settings: Arc<Mutex<AppSettings>>,
    pipeline_logger: Arc<PipelineLogManager>,
    last_activity: Arc<Mutex<Instant>>,
    session_id: Arc<AtomicU64>,
    /// The last failed translation, offered for retry until another recording finishes.
    pending_retry: Arc<Mutex<Option<PendingRetry>>>,
    /// The untranslated text of the last translation, while the pill offers to paste it instead.
    /// When and how the current recording started, for the start/stop log.
    recording_start: Arc<Mutex<Option<RecordingStart>>>,
    /// Set when a recording starts; read when it's processed.
    recording_mode: Arc<Mutex<RecordingMode>>,
    /// Held while a recording starts. The phase turns Listening before the microphone is open, so
    /// a stop or cancel arriving in between (e.g. a quick hold-to-talk release) waits on this;
    /// otherwise it reached the recorder before the start did and the microphone was left open.
    start_gate: Arc<Mutex<()>>,
    /// Pieces of the current recording transcribed while it goes on (see `live_chunks`).
    live_chunks: Arc<Mutex<Option<crate::live_chunks::LiveChunks>>>,
}

impl AppController {
    pub fn new(app_handle: AppHandle) -> Result<Self, String> {
        let settings = AppSettings::load();
        crate::transcribe::set_device_backend(&settings.device_backend);
        crate::pipeline_logger::set_private(crate::settings::is_private(&settings.storage_mode));
        let history = Arc::new(HistoryManager::new()?);

        let _ = history.enforce_retention_policy(settings.retention_days);

        let phase = Arc::new(Mutex::new(AssistantPhase::Idle));
        let recorder = Arc::new(AudioRecorder::new());
        let transcriber = Arc::new(Mutex::new(Transcriber::new()));
        let parakeet_transcriber = Arc::new(Mutex::new(crate::parakeet::ParakeetTranscriber::new()));
        let settings_arc = Arc::new(Mutex::new(settings));
        let pipeline_logger = Arc::new(PipelineLogManager::new());
        let last_activity = Arc::new(Mutex::new(Instant::now()));
        let session_id = Arc::new(AtomicU64::new(0));

        // Background task: Configurable idle model unloader (drops RAM to ~45 MB)
        {
            let transcriber_idle = Arc::clone(&transcriber);
            let parakeet_idle = Arc::clone(&parakeet_transcriber);
            let phase_idle = Arc::clone(&phase);
            let settings_idle = Arc::clone(&settings_arc);
            let last_act_idle = Arc::clone(&last_activity);
            let data_dir_idle = get_data_dir();
            let app_handle_idle = app_handle.clone();

            tauri::async_runtime::spawn(async move {
                loop {
                    tokio::time::sleep(Duration::from_secs(5)).await;
                    let idle_limit_sec = {
                        let s = settings_idle.lock().unwrap();
                        s.model_idle_unload_sec
                    };
                    if idle_limit_sec == 0 {
                        continue; // 0 = Never unload
                    }
                    let is_idle = {
                        let p = phase_idle.lock().unwrap();
                        *p == AssistantPhase::Idle
                    };
                    // Audio Lab takes the model piece by piece; freeing it in between would reload it each time.
                    if !is_idle || crate::lab::is_transcribing() {
                        continue;
                    }
                    let elapsed = {
                        let t = last_act_idle.lock().unwrap();
                        t.elapsed().as_secs()
                    };
                    if elapsed >= idle_limit_sec {
                        let rss_before = crate::vitals::process_rss_mb();
                        let mut tr_unloaded = false;
                        let mut pk_unloaded = false;
                        {
                            let mut tr = transcriber_idle.lock().unwrap();
                            if tr.has_context() {
                                tr.unload();
                                tr_unloaded = true;
                            }
                        }
                        {
                            let mut pk = parakeet_idle.lock().unwrap();
                            if pk.has_model() {
                                pk.unload();
                                pk_unloaded = true;
                            }
                        }
                        if tr_unloaded || pk_unloaded {
                            crate::tray::update_tray_model_status(&app_handle_idle, false);
                            log_stage_event(
                                &data_dir_idle,
                                "IDLE_CLEANUP",
                                &format!(
                                    "Freed speech model from RAM after {}s idle (limit: {}s); app memory {} → {} MB",
                                    elapsed,
                                    idle_limit_sec,
                                    rss_before,
                                    crate::vitals::process_rss_mb()
                                ),
                            );
                        }
                    }
                }
            });
        }

        Ok(Self {
            app_handle,
            phase,
            recorder,
            transcriber,
            parakeet_transcriber,
            history,
            settings: settings_arc,
            pipeline_logger,
            last_activity,
            session_id,
            pending_retry: Arc::new(Mutex::new(None)),
            recording_start: Arc::new(Mutex::new(None)),
            recording_mode: Arc::new(Mutex::new(RecordingMode::Main)),
            start_gate: Arc::new(Mutex::new(())),
            live_chunks: Arc::new(Mutex::new(None)),
        })
    }

    /// What transcribes the pieces cut while recording, when live chunking applies: chunking on and
    /// the model on disk. On the Neural Engine this is the opt-in Fast mode; off, it takes the whole
    /// recording at the end.
    fn live_transcriber(&self, settings: &AppSettings) -> Option<crate::live_chunks::Transcribe> {
        use crate::live_chunks::Piece;
        if !settings.audio_chunking || !crate::model_download::is_model_present(&settings.model_name) {
            return None;
        }
        if settings.model_name == crate::model_download::PARAKEET_MODEL {
            let parakeet = Arc::clone(&self.parakeet_transcriber);
            let dir = crate::parakeet::ParakeetTranscriber::get_model_dir();
            return Some(Box::new(move |piece: &[f32]| {
                let mut pk = parakeet.lock().unwrap_or_else(|e| e.into_inner());
                let (text, ..) = pk.transcribe(piece, &[0..piece.len()], &dir)?;
                Ok(Piece { text, lang: None })
            }));
        }
        let transcriber = Arc::clone(&self.transcriber);
        let path = Transcriber::get_model_path(&settings.model_name);
        let excluded = settings.excluded_languages.clone();
        let mut lang = crate::transcribe::map_language_hint(&settings.source_lang).map(str::to_string);
        Some(Box::new(move |piece: &[f32]| {
            let mut tr = transcriber.lock().unwrap_or_else(|e| e.into_inner());
            let res = tr.transcribe_detailed(piece, &[0..piece.len()], &path, false, lang.as_deref())?;
            // The first piece sets the language for the rest. One in an excluded language is
            // re-transcribed in another, so that recording goes the usual way, whole.
            if lang.is_none() && res.detected_lang != "auto" {
                if !excluded.trim().is_empty() && crate::translate::is_language_excluded(&res.detected_lang, &res.text, &excluded) {
                    return Err(format!("the first piece is in an excluded language ({})", res.detected_lang));
                }
                lang = Some(res.detected_lang.clone());
            }
            Ok(Piece { text: res.text, lang: lang.clone() })
        }))
    }

    pub fn get_phase(&self) -> String {
        self.phase.lock().unwrap().as_str().to_string()
    }

    pub fn is_active(&self) -> bool {
        let p = self.phase.lock().unwrap();
        *p != AssistantPhase::Idle
    }

    pub fn is_listening(&self) -> bool {
        let p = self.phase.lock().unwrap();
        *p == AssistantPhase::Listening
    }

    pub fn get_settings(&self) -> AppSettings {
        self.settings.lock().unwrap().clone()
    }

    /// Saves settings changed outside the Settings window (the pill, the tray) and sends them to
    /// the Settings window, so it shows them and doesn't save its older copy over them.
    pub fn save_settings(&self, new_settings: AppSettings) -> Result<(), String> {
        self.save_settings_from_window(new_settings.clone())?;
        let _ = self.app_handle.emit("settings-changed", &new_settings);
        Ok(())
    }

    /// Saves settings the Settings window sent; it already shows them.
    pub fn save_settings_from_window(&self, new_settings: AppSettings) -> Result<(), String> {
        let (old_model, devices_changed) = {
            let s = self.settings.lock().unwrap();
            (
                s.model_name.clone(),
                s.input_device != new_settings.input_device || s.output_device != new_settings.output_device,
            )
        };
        new_settings.save()?;
        {
            let mut s = self.settings.lock().unwrap();
            *s = new_settings.clone();
        }
        // A loaded Whisper model is moved to or off the GPU when it is next used.
        crate::transcribe::set_device_backend(&new_settings.device_backend);
        crate::pipeline_logger::set_private(crate::settings::is_private(&new_settings.storage_mode));
        crate::tray::sync_translation_menu(&new_settings);
        if devices_changed {
            // Listing devices can take seconds on Linux; this may run on the UI thread (Settings, tray, pill).
            let (app, settings) = (self.app_handle.clone(), new_settings.clone());
            std::thread::spawn(move || crate::tray::update_tray_audio_devices(&app, &settings));
        }

        // Single Model Policy: If active model changed, drop the other from RAM immediately
        if old_model != new_settings.model_name {
            if new_settings.model_name == crate::model_download::PARAKEET_MODEL {
                let mut tr = self.transcriber.lock().unwrap();
                tr.unload();
            } else {
                let mut pk = self.parakeet_transcriber.lock().unwrap();
                pk.unload();
            }
        }

        Ok(())
    }

    pub fn get_history(&self, limit: usize) -> Result<Vec<HistoryRecord>, String> {
        self.history.get_records(limit)
    }

    pub fn history_audio_bytes(&self) -> u64 {
        self.history.audio_bytes()
    }

    pub fn clear_history(&self) -> Result<(), String> {
        self.history.clear_history()?;
        self.pipeline_logger.clear_logs();
        crate::pipeline_logger::delete_log_files(&get_data_dir());
        Ok(())
    }

    pub fn get_diagnostic_logs(&self, limit: usize) -> Vec<TranscriptionDiagnosticLog> {
        self.pipeline_logger.get_logs(limit)
    }

    pub fn clear_diagnostic_logs(&self) {
        self.pipeline_logger.clear_logs();
    }

    pub fn is_model_loaded(&self) -> bool {
        // try_lock: a model loading holds its lock for seconds (17 s on the first Neural Engine
        // load), and stopping a recording asks this for the tray icon; waiting froze the stop.
        let tr_loaded = self.transcriber.try_lock().map(|t| t.has_context()).unwrap_or(false);
        let pk_loaded = self.parakeet_transcriber.try_lock().map(|p| p.has_model()).unwrap_or(false);
        tr_loaded || pk_loaded
    }

    pub fn eject_model(&self) {
        let mut did = false;
        if let Ok(mut t) = self.transcriber.lock() {
            if t.has_context() { t.unload(); did = true; }
        }
        if let Ok(mut pk) = self.parakeet_transcriber.lock() {
            if pk.has_model() { pk.unload(); did = true; }
        }
        if did {
            crate::tray::update_tray_model_status(&self.app_handle, false);
            crate::settings::get_data_dir(); // ensure data dir exists for log
            crate::pipeline_logger::log_stage_event(&crate::settings::get_data_dir(), "EJECT_MODEL", "User ejected LLM from RAM via Control Center");
        }
    }

    pub fn get_audio_data(&self, filename: Option<String>) -> Result<String, String> {
        use base64::Engine;
        let data_dir = get_data_dir();
        let path = if let Some(ref f) = filename {
            if f.is_empty() || f == "latest" || f == "latest_recording.wav" {
                data_dir.join("latest_recording.wav")
            } else {
                let p = self.history.get_audio_file_path(f);
                if p.exists() {
                    p
                } else {
                    data_dir.join(f)
                }
            }
        } else {
            data_dir.join("latest_recording.wav")
        };

        if !path.exists() {
            return Err(format!("Audio file not found: {:?}", path));
        }

        let bytes = std::fs::read(&path)
            .map_err(|e| format!("Failed to read audio file: {}", e))?;
        let b64 = base64::engine::general_purpose::STANDARD.encode(&bytes);
        Ok(b64)
    }

    pub fn open_audio_folder(&self) -> Result<(), String> {
        let dir = self.history.get_audio_dir();
        crate::custom_models::open_in_file_manager(&dir)
    }

    pub fn list_lab_audio(&self) -> Vec<crate::lab::LabAudioItem> {
        crate::lab::list_available_audio_files()
    }

    pub async fn run_lab_experiment(
        &self,
        req: crate::lab::LabExperimentRequest,
    ) -> Result<crate::lab::LabExperimentResult, String> {
        // The dictation model stays loaded after the run, so the next dictation doesn't reload it.
        let keep_loaded = self.get_settings().model_name == req.model_name;
        let res = crate::lab::run_experiment(
            &self.app_handle,
            Arc::clone(&self.transcriber),
            Arc::clone(&self.parakeet_transcriber),
            Arc::clone(&self.phase),
            keep_loaded,
            req,
        )
        .await;
        if keep_loaded {
            // The idle cleanup frees it as after a dictation, counting from now.
            *self.last_activity.lock().unwrap() = Instant::now();
        }
        res
    }

    /// Audio Lab's second try at a translation: sends `text` to the chosen service ("LLM",
    /// "Custom API" or "Apple") with the saved keys, endpoints and prompt, and returns the result without
    /// pasting it. Masking applies as it does for a dictation.
    pub async fn lab_translate(
        &self,
        text: String,
        provider: String,
        target_lang: String,
    ) -> Result<LabTranslation, String> {
        let text = text.trim().to_string();
        if text.is_empty() {
            return Err("There is no text to translate".to_string());
        }
        let mut settings = self.get_settings();
        settings.translation_provider = provider;
        if !target_lang.trim().is_empty() {
            settings.target_lang = target_lang.trim().to_string();
        }
        let mask_config = mask_config_from(&settings);
        let input = if mask_config.enabled { crate::masker::apply_masking(&text, &mask_config) } else { text };
        let route = TranslationRoute::from_settings(&settings);
        let service = route.describe(&settings);
        let data_dir = get_data_dir();
        log_stage_event(&data_dir, "LAB_TRANSLATE_START", &format!("Audio Lab: translating to {} with {}: \"{}\"", settings.target_lang, service, input));
        let started = Instant::now();
        // Audio Lab has no detected language; Apple's provider detects it from the text.
        let result = route.translate(&settings, &input, "auto").await;
        let ms = started.elapsed().as_millis() as u64;
        match result {
            Ok(translated) => {
                log_stage_event(&data_dir, "LAB_TRANSLATE_DONE", &format!("Translated in {} ms via {}: \"{}\"", ms, translated.via, translated.text));
                Ok(LabTranslation { text: translated.text.trim().to_string(), ms, service: translated.via })
            }
            Err(e) => {
                log_stage_event(&data_dir, "LAB_TRANSLATE_FAILED", &format!("{} after {} ms: {}", service, ms, e));
                Err(format!("{} failed: {}", route.label(), e))
            }
        }
    }

    /// Saves an Audio Lab result to the Logs entry `log_id` and its history row: a transcript
    /// for a recording whose transcription failed (`kind` "transcription"), or a translation of
    /// `source` (`kind` "translation"). A saved translation also ends the tray's retry for it.
    pub fn lab_save_result(&self, log_id: u64, kind: &str, text: String, source: Option<String>) -> Result<(), String> {
        let text = text.trim().to_string();
        if text.is_empty() {
            return Err("There is no text to save".to_string());
        }
        let translation = match kind {
            "translation" => true,
            "transcription" => false,
            other => return Err(format!("Unknown result kind: {}", other)),
        };
        let log = self
            .pipeline_logger
            .update_log(log_id, |log| {
                if translation {
                    let spoken = source
                        .map(|s| s.trim().to_string())
                        .filter(|s| !s.is_empty())
                        .or_else(|| log.spoken_text.clone())
                        .unwrap_or_else(|| log.final_text.clone());
                    log.spoken_text = Some(spoken);
                    log.translation_error = None;
                    log.action_logs.push("[AUDIO LAB] Translation saved from Audio Lab".to_string());
                } else {
                    log.raw_text = text.clone();
                    log.transcription_error = None;
                    log.action_logs.push("[AUDIO LAB] Transcript saved from Audio Lab".to_string());
                }
                log.final_text = text.clone();
            })
            .ok_or_else(|| "This recording is no longer in Logs".to_string())?;

        if let Some(history_id) = log.history_id {
            if translation {
                self.history.update_translation(history_id, &text)?;
                let mut pending = self.pending_retry.lock().unwrap();
                if pending.as_ref().is_some_and(|p| p.history_id == history_id) {
                    *pending = None;
                    crate::tray::set_retry_item(None);
                }
            } else {
                self.history.update_transcription(history_id, &text)?;
            }
        }
        log_stage_event(
            &get_data_dir(),
            "LAB_SAVE",
            &format!("Audio Lab: saved a {} to log #{}: \"{}\"", kind, log_id, text),
        );
        Ok(())
    }

    /// Saves a file opened in the Audio Lab as `lab_custom.<its extension>`, so the decoder can
    /// tell its format, and returns that name.
    // shortcut: the file crosses IPC as base64 in memory; a native file picker passing a path would scale past ~100 MB files.
    pub fn save_lab_custom_audio(&self, base64_wav: String, file_name: Option<String>) -> Result<String, String> {
        use base64::Engine;
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(&base64_wav)
            .map_err(|e| format!("Base64 decode error: {}", e))?;
        let ext = file_name
            .as_deref()
            .and_then(|n| std::path::Path::new(n).extension()?.to_str().map(str::to_ascii_lowercase))
            .filter(|e| !e.is_empty() && e.len() <= 5 && e.chars().all(|c| c.is_ascii_alphanumeric()))
            .unwrap_or_else(|| "wav".to_string());
        let data_dir = get_data_dir();
        // One custom file at a time: drop the last one, whatever its format.
        if let Ok(entries) = std::fs::read_dir(&data_dir) {
            for entry in entries.flatten() {
                if entry.path().file_stem().and_then(|s| s.to_str()) == Some("lab_custom") {
                    let _ = std::fs::remove_file(entry.path());
                }
            }
        }
        let name = format!("lab_custom.{}", ext);
        std::fs::write(data_dir.join(&name), bytes).map_err(|e| format!("Failed to write custom audio: {}", e))?;
        Ok(name)
    }

    pub fn cancel(&self, trigger: &'static str) {
        let _started = self.start_gate.lock().unwrap();
        if let Some(start) = self.recording_start.lock().unwrap().take() {
            log_stage_event(
                &get_data_dir(),
                "REC_CANCEL",
                &format!(
                    "Recording cancelled by {} after {:.2} s (started by {})",
                    trigger,
                    start.at.elapsed().as_secs_f32(),
                    start.trigger
                ),
            );
        }
        self.session_id.fetch_add(1, Ordering::SeqCst);
        {
            let mut phase = self.phase.lock().unwrap();
            *phase = AssistantPhase::Idle;
        }
        crate::tray::set_tray_recording(&self.app_handle, false, self.is_model_loaded());
        self.recorder.cancel_recording();
        self.live_chunks.lock().unwrap().take();

        let _ = self.app_handle.emit(
            "assistant-state-changed",
            serde_json::json!({
                "state": "idle",
                "title": "Ready",
                "subtitle": null
            }),
        );

        hide_main_window(&self.app_handle);
    }

    /// Called from the audio thread when the microphone health check fails during a recording.
    /// Stops the recording (it has no usable audio) and shows what is wrong, long enough to read.
    pub fn on_mic_problem(&self, report: crate::audio::MicReport) {
        if !report.problem.is_startup() {
            self.on_mic_dropout(report);
            return;
        }
        {
            let mut phase = self.phase.lock().unwrap();
            if *phase != AssistantPhase::Listening {
                return;
            }
            // Idle, not Error: pressing the hotkey again starts a fresh recording right away.
            *phase = AssistantPhase::Idle;
        }
        let session = self.session_id.fetch_add(1, Ordering::SeqCst) + 1;
        self.recorder.cancel_recording();
        self.live_chunks.lock().unwrap().take();
        crate::tray::set_tray_recording(&self.app_handle, false, self.is_model_loaded());

        let (title, subtitle) = mic_problem_message(&report);
        log_stage_event(&get_data_dir(), "MIC_CHECK", &format!("{}: {} ({:?})", title, subtitle, report));
        if let Some(start) = self.recording_start.lock().unwrap().take() {
            log_stage_event(
                &get_data_dir(),
                "REC_CANCEL",
                &format!("Recording cancelled by the mic check after {:.2} s (started by {})", start.at.elapsed().as_secs_f32(), start.trigger),
            );
        }
        // Offer the other connected mics right in the pill, so switching doesn't need Settings.
        let devices: Vec<String> = crate::audio::list_input_devices().into_iter().filter(|d| *d != report.device).collect();
        if devices.is_empty() {
            show_long_error(&self.app_handle, &self.session_id, session, title, &subtitle);
            return;
        }
        show_long_message(
            &self.app_handle,
            &self.session_id,
            session,
            serde_json::json!({ "state": "error", "title": title, "subtitle": subtitle, "long": true, "devices": devices }),
            MIC_CHOICE_HOLD,
        );
    }

    /// The microphone failed after real audio arrived. Keep recording, so what was said before is still
    /// transcribed, and warn in the pill until audio comes back or the user stops.
    fn on_mic_dropout(&self, report: crate::audio::MicReport) {
        if *self.phase.lock().unwrap() != AssistantPhase::Listening {
            return;
        }
        let at = self
            .recording_start
            .lock()
            .unwrap()
            .as_ref()
            .map(|s| s.at.elapsed().as_secs_f32())
            .unwrap_or_default();
        let (title, subtitle) = mic_dropout_message(&report);
        log_stage_event(
            &get_data_dir(),
            "MIC_DROPOUT",
            &format!("{} at {:.1} s into the recording: {} ({:?})", title, at, subtitle, report),
        );
        // "reconnecting" while the stream is being reopened, "down" once reopening didn't help.
        let mic = if report.reconnecting { "reconnecting" } else { "down" };
        set_pill_window_size(&self.app_handle, LONG_ERROR_WINDOW);
        let _ = self.app_handle.emit(
            "assistant-state-changed",
            serde_json::json!({ "state": "listening", "title": title, "subtitle": subtitle, "warning": true, "mic": mic, "long": true }),
        );
    }

    /// Nothing that sounds like speech since the recording started: most often a muted mic, which
    /// still sends a faint noise floor. Keep recording (the user may just be thinking) but warn,
    /// with the error sound, until speech arrives or the user stops.
    pub fn on_no_speech(&self, after: Duration) {
        if *self.phase.lock().unwrap() != AssistantPhase::Listening {
            return;
        }
        let secs = after.as_secs();
        log_stage_event(&get_data_dir(), "NO_SPEECH", &format!("No speech heard in the first {} s of the recording; warned the user", secs));
        set_pill_window_size(&self.app_handle, LONG_ERROR_WINDOW);
        let _ = self.app_handle.emit(
            "assistant-state-changed",
            serde_json::json!({
                "state": "listening",
                "title": "No Speech Heard",
                "subtitle": format!("Nothing heard for {secs} s. Is your mic muted? Keep talking, or press the hotkey to stop."),
                "warning": true,
                "mic": "down",
                "long": true,
            }),
        );
    }

    /// Speech arrived after the no-speech warning: back to the normal listening pill.
    pub fn on_speech_heard(&self) {
        if *self.phase.lock().unwrap() != AssistantPhase::Listening {
            return;
        }
        log_stage_event(&get_data_dir(), "SPEECH_HEARD", "Speech heard after the no-speech warning");
        set_pill_window_size(&self.app_handle, PILL_WINDOW);
        let mode = self.recording_mode.lock().unwrap().pill_mode(&self.get_settings());
        let _ = self.app_handle.emit(
            "assistant-state-changed",
            serde_json::json!({ "state": "listening", "title": "Listening…", "subtitle": null, "mode": mode }),
        );
    }

    /// Audio is flowing again after a mid-recording dropout: clear the pill's warning. `switched_to`
    /// names the device now in use when the chosen one was gone and the system default took over.
    pub fn on_mic_recovered(&self, switched_to: Option<String>) {
        if *self.phase.lock().unwrap() != AssistantPhase::Listening {
            return;
        }
        let at = self
            .recording_start
            .lock()
            .unwrap()
            .as_ref()
            .map(|s| s.at.elapsed().as_secs_f32())
            .unwrap_or_default();
        let via = switched_to.as_ref().map(|d| format!(" on \"{}\"", d)).unwrap_or_default();
        log_stage_event(&get_data_dir(), "MIC_RECOVERED", &format!("Microphone audio came back{} at {:.1} s into the recording", via, at));
        // Say which mic took over when it changed, in the larger pill so the name fits.
        let subtitle = switched_to.map(|d| format!("Switched to \"{}\"", d));
        let long = subtitle.is_some();
        set_pill_window_size(&self.app_handle, if long { LONG_ERROR_WINDOW } else { PILL_WINDOW });
        let mode = self.recording_mode.lock().unwrap().pill_mode(&self.get_settings());
        let _ = self.app_handle.emit(
            "assistant-state-changed",
            serde_json::json!({ "state": "listening", "title": "Listening…", "subtitle": subtitle, "long": long, "mode": mode }),
        );
    }

    pub fn has_pending_retry(&self) -> bool {
        self.pending_retry.lock().unwrap().is_some()
    }

    /// Translates the last failed text again, with the current settings (so a fixed API key or
    /// endpoint takes effect), pastes the result and updates its history entry.
    pub fn retry_translation(&self) -> Result<(), String> {
        let pending = self
            .pending_retry
            .lock()
            .unwrap()
            .clone()
            .ok_or_else(|| "There is no failed translation to retry".to_string())?;
        {
            let mut phase = self.phase.lock().unwrap();
            if *phase != AssistantPhase::Idle {
                return Err("Bifaria is busy; try again when it finishes".to_string());
            }
            *phase = AssistantPhase::Translating;
        }
        // A new session also stops the error pill's hide timer.
        let session = self.session_id.fetch_add(1, Ordering::SeqCst) + 1;
        let settings = self.get_settings();
        let route = TranslationRoute::from_settings(&settings);
        let from_name = crate::translate::format_lang_name(&pending.from_lang);
        let to_name = crate::translate::format_lang_name(&settings.target_lang);

        show_main_window(&self.app_handle, &settings);
        let _ = self.app_handle.emit(
            "assistant-state-changed",
            serde_json::json!({
                "state": "translating",
                "title": "Retrying translation…",
                "subtitle": format!("{} → {} ({})", from_name, to_name, route.label())
            }),
        );

        let app_handle = self.app_handle.clone();
        let phase_arc = Arc::clone(&self.phase);
        let session_id_arc = Arc::clone(&self.session_id);
        let pending_retry_arc = Arc::clone(&self.pending_retry);
        let history_arc = Arc::clone(&self.history);
        tauri::async_runtime::spawn(async move {
            let data_dir = get_data_dir();
            log_stage_event(
                &data_dir,
                "TRANSLATE_RETRY_START",
                &format!("Retrying {} → {} with {}: \"{}\"", from_name, to_name, route.describe(&settings), pending.text),
            );
            let started = Instant::now();
            let result = route.translate(&settings, &pending.text, &pending.from_lang).await;
            let ms = started.elapsed().as_millis() as u64;
            if session_id_arc.load(Ordering::SeqCst) != session {
                log_stage_event(&data_dir, "TRANSLATE_RETRY_CANCELLED", "A new recording started; retry result dropped");
                return;
            }

            match result {
                Ok(translated) => {
                    let mask_config = mask_config_from(&settings);
                    let final_text = if mask_config.enabled {
                        crate::masker::apply_masking(&translated.text, &mask_config)
                    } else {
                        translated.text
                    };
                    log_stage_event(&data_dir, "TRANSLATE_RETRY_DONE", &format!("Translated in {} ms via {}: \"{}\"", ms, translated.via, final_text));
                    *phase_arc.lock().unwrap() = AssistantPhase::Done;
                    let pasted = if settings.auto_paste {
                        copy_and_paste(&final_text, settings.keep_clipboard).map(|_| ())
                    } else {
                        crate::paste::copy_to_clipboard(&final_text)
                    };
                    if let Err(e) = pasted {
                        log_stage_event(&data_dir, "PASTE_FAILED", &e);
                    }
                    if let Err(e) = history_arc.update_translation(pending.history_id, &final_text) {
                        log::warn!("Could not update history with the retried translation: {}", e);
                    }
                    *pending_retry_arc.lock().unwrap() = None;
                    crate::tray::set_retry_item(None);
                    let _ = app_handle.emit(
                        "assistant-state-changed",
                        serde_json::json!({
                            "state": "done",
                            "title": "Copied to clipboard",
                            "subtitle": null,
                            "text": final_text
                        }),
                    );
                    tokio::time::sleep(Duration::from_millis(600)).await;
                    let mut p = phase_arc.lock().unwrap();
                    if *p == AssistantPhase::Done && session_id_arc.load(Ordering::SeqCst) == session {
                        *p = AssistantPhase::Idle;
                        let _ = app_handle.emit(
                            "assistant-state-changed",
                            serde_json::json!({ "state": "idle", "title": "Ready", "subtitle": null }),
                        );
                        hide_main_window(&app_handle);
                    }
                }
                Err(e) => {
                    let detail = format!("{} → {} via {} failed again after {} ms: {}", from_name, to_name, route.describe(&settings), ms, e);
                    log_stage_event(&data_dir, "TRANSLATE_RETRY_ERROR", &detail);
                    log::error!("Translation retry failed: {}", detail);
                    *phase_arc.lock().unwrap() = AssistantPhase::Idle;
                    show_long_error_payload(
                        &app_handle,
                        &session_id_arc,
                        session,
                        serde_json::json!({
                            "state": "error",
                            "title": "Translation Failed Again",
                            "subtitle": format!("{}. Check your translation settings, then retry.", capitalize(&e)),
                            "long": true,
                            "retry": true
                        }),
                    );
                }
            }
        });
        Ok(())
    }

    /// Starts or stops a recording. `trigger` says what asked for it (e.g. "hotkey press"); it goes
    /// into the start/stop log.
    pub fn toggle_recording(&self, trigger: &'static str) -> Result<(), String> {
        self.toggle_recording_as(trigger, RecordingMode::Main)
    }

    /// Like `toggle_recording`; `mode` applies only when this starts a recording (either hotkey
    /// finishes one, keeping the mode it started with).
    pub fn toggle_recording_as(&self, trigger: &'static str, mode: RecordingMode) -> Result<(), String> {
        let cur_phase = { self.phase.lock().unwrap().clone() };

        if PRACTICE_MODE.load(Ordering::SeqCst) && matches!(cur_phase, AssistantPhase::Idle) {
            let mode = match mode {
                RecordingMode::Main => "main",
                RecordingMode::Translate => "translate",
            };
            let _ = self.app_handle.emit("practice-hotkey", serde_json::json!({ "mode": mode }));
            return Ok(());
        }

        match cur_phase {
            AssistantPhase::Idle => {
                *self.recording_mode.lock().unwrap() = mode;
                let started = self.start_listening(trigger);
                if self.is_listening() {
                    crate::secure_input::guard_escape(&self.app_handle);
                }
                started
            }
            AssistantPhase::Listening => self.stop_listening_and_process(trigger),
            _ => {
                // If currently processing or in Done state, ignore toggle or cancel
                Ok(())
            }
        }
    }

    fn start_listening(&self, trigger: &'static str) -> Result<(), String> {
        let pressed = Instant::now();
        let _starting = self.start_gate.lock().unwrap();
        // A second press that read Idle while the first was still starting must not restart it.
        if *self.phase.lock().unwrap() != AssistantPhase::Idle {
            return Ok(());
        }
        self.session_id.fetch_add(1, Ordering::SeqCst);

        // A recording that will be translated needs the translation settings filled in. Say so
        // now, before anything is recorded, instead of after the user has finished talking.
        {
            let settings = self.get_settings();
            let mode = *self.recording_mode.lock().unwrap();
            let missing = missing_translation_settings(&settings);
            if !mode.transcribe_only(&settings) {
                if let Some(problem) = apple_setup_problem(&settings) {
                    log_stage_event(
                        &get_data_dir(),
                        "REC_BLOCKED",
                        &format!("Recording requested by {} ({:?} mode) not started: {}", trigger, mode, problem),
                    );
                    let subtitle = if mode == RecordingMode::Translate {
                        format!("{problem}. Nothing was recorded; the launch hotkey still dictates without translating.")
                    } else {
                        format!("{problem}. Nothing was recorded. To dictate without translating, set When to translate to Off.")
                    };
                    show_main_window(&self.app_handle, &settings);
                    let session = self.session_id.load(Ordering::SeqCst);
                    let title = if crate::offline_translate::is_offline(&settings.translation_provider) {
                        "Offline Translation Not Ready"
                    } else {
                        "Apple Translation Not Ready"
                    };
                    show_long_error(&self.app_handle, &self.session_id, session, title, &subtitle);
                    return Ok(());
                }
            }
            if !mode.transcribe_only(&settings) && !missing.is_empty() {
                let what = list_missing(&missing);
                log_stage_event(
                    &get_data_dir(),
                    "REC_BLOCKED",
                    &format!("Recording requested by {} ({:?} mode) not started: translation is missing {}", trigger, mode, what),
                );
                let subtitle = if mode == RecordingMode::Translate {
                    format!("Add {what} in Settings → Translations. Nothing was recorded; the launch hotkey still dictates without translating.")
                } else {
                    format!("Automatic translation needs {what} in Settings → Translations. Nothing was recorded. To dictate without translating, set When to translate to Off.")
                };
                show_main_window(&self.app_handle, &settings);
                let session = self.session_id.load(Ordering::SeqCst);
                show_long_error(&self.app_handle, &self.session_id, session, "Translation Not Set Up", &subtitle);
                return Ok(());
            }
            // Recording from the system default instead of the chosen mic often catches nothing (a
            // Bluetooth headset in call mode, a muted laptop mic), so ask which mic to use instead.
            if let Some(missing) = crate::audio::missing_input_device(settings.input_device.as_deref()) {
                self.offer_mic_choice(trigger, mode, &missing, &settings);
                return Ok(());
            }
        }
        {
            let mut phase = self.phase.lock().unwrap();
            *phase = AssistantPhase::Listening;
        }

        crate::tray::set_tray_recording(&self.app_handle, true, self.is_model_loaded());

        {
            let mut t = self.last_activity.lock().unwrap();
            *t = Instant::now();
        }

        let settings = self.get_settings();
        let mode = *self.recording_mode.lock().unwrap();

        // 1. Immediately emit state so the pill UI displays "Listening..." without delay
        let _ = self.app_handle.emit(
            "assistant-state-changed",
            serde_json::json!({
                "state": "listening",
                "title": "Listening…",
                // Audio Lab waits between pieces while a dictation runs (see lab::yield_to_dictation).
                "subtitle": crate::lab::is_transcribing().then_some("Audio Lab paused for this"),
                "mode": mode.pill_mode(&settings)
            }),
        );

        // 2. Immediately show the pill window on screen
        show_main_window(&self.app_handle, &settings);
        // Runs right after the show above on the main thread, so it records how long a busy
        // system kept the pill off screen.
        let _ = self.app_handle.run_on_main_thread(move || {
            log_stage_event(&get_data_dir(), "PILL_SHOWN", &format!("Pill shown {} ms after the key press", pressed.elapsed().as_millis()));
        });

        // 3. Open the microphone first, so speech right after the key press is captured. (Playing
        // the chime to completion first lost 1.5-2.5 s of speech on Bluetooth headsets.)
        let mic_open_started = Instant::now();
        crate::audio::set_no_speech_alert(settings.no_speech_alert_sec);
        let denoise = crate::denoise::strength_wet(&settings.noise_reduction).is_some();
        let stream = self.recorder.start_recording(self.app_handle.clone(), settings.input_device.clone(), denoise);
        let mic_open_ms = mic_open_started.elapsed().as_millis();
        *self.live_chunks.lock().unwrap() = stream
            .is_ok()
            .then(|| self.live_transcriber(&settings))
            .flatten()
            .map(|t| crate::live_chunks::LiveChunks::start(Arc::clone(&self.recorder), crate::denoise::strength_wet(&settings.noise_reduction), t));

        // 4. Then the chime, once the mic delivers its first audio. Playback and capture are never
        // opened at the same instant (some combo USB/Bluetooth headsets stall when they are), and
        // Bluetooth headsets have finished switching to their call profile, which swallowed a
        // chime played while it was under way.
        if stream.is_ok() && settings.sound_start {
            let recorder = Arc::clone(&self.recorder);
            let session_id = Arc::clone(&self.session_id);
            let session = session_id.load(Ordering::SeqCst);
            std::thread::spawn(move || {
                let waited = recorder.wait_for_first_audio(START_CHIME_MAX_WAIT);
                if session_id.load(Ordering::SeqCst) != session || !recorder.is_recording() {
                    return;
                }
                crate::sound::play_sound(crate::sound::AppSound::StartRecording);
                recorder.mark_start_chime(crate::sound::duration(crate::sound::AppSound::StartRecording));
                let detail = match waited {
                    Some(d) => format!("Start chime played when the first audio arrived, {} ms after the mic opened", d.as_millis()),
                    None => format!("Start chime played without audio from the mic after {} ms", START_CHIME_MAX_WAIT.as_millis()),
                };
                log_stage_event(&get_data_dir(), "START_CHIME", &detail);
            });
        }

        // 5. Apple Translation: start its helper and load the language model while the user talks;
        // the first translation after launch otherwise waits 2-3 s for it.
        if stream.is_ok()
            && !mode.transcribe_only(&settings)
            && crate::apple_translate::is_apple(&settings.translation_provider)
            && crate::apple_translate::is_available()
        {
            if let Some(target) = crate::apple_translate::to_bcp47(&settings.target_lang) {
                let source = crate::apple_translate::to_bcp47(&settings.source_lang);
                let model = crate::apple_translate::model_name(&settings.apple_translation_model);
                tauri::async_runtime::spawn(crate::apple_translate::warm_up(source, target, model));
            }
        }
        // Offline provider: load the pair's models (a few hundred ms from disk) while the user talks.
        if stream.is_ok()
            && !mode.transcribe_only(&settings)
            && crate::offline_translate::is_offline(&settings.translation_provider)
        {
            let target = crate::apple_translate::to_bcp47(&settings.target_lang);
            if let (Some(source), Some(target)) = (crate::apple_translate::to_bcp47(&settings.source_lang), target) {
                tauri::async_runtime::spawn(crate::offline_translate::warm_up(source, target));
            }
        }
        // Apple Intelligence: the same, for its language model (about 2.6 s → 0.9 s on the first one).
        if stream.is_ok()
            && !mode.transcribe_only(&settings)
            && crate::apple_translate::is_apple_intelligence(&settings.translation_provider)
            && crate::apple_translate::is_available()
        {
            tauri::async_runtime::spawn(crate::apple_translate::warm_up_intelligence());
        }

        if let Ok(info) = &stream {
            let fallback = info
                .missing_device
                .as_ref()
                .map(|m| format!(" (chosen \"{}\" not found, using default)", m))
                .unwrap_or_default();
            log_stage_event(
                &get_data_dir(),
                "REC_START",
                &format!(
                    "Recording started by {} ({:?} mode) on \"{}\" at {} Hz, {} ch{}; microphone ready {} ms after the key press (opening it took {} ms)",
                    trigger,
                    mode,
                    info.device,
                    info.sample_rate,
                    info.channels,
                    fallback,
                    pressed.elapsed().as_millis(),
                    mic_open_ms
                ),
            );
            *self.recording_start.lock().unwrap() = Some(RecordingStart {
                at: Instant::now(),
                trigger,
                device: info.device.clone(),
                sample_rate: info.sample_rate,
            });
        }
        if let Err(e) = stream {
            log_stage_event(&get_data_dir(), "REC_START_FAILED", &format!("Recording requested by {} failed: {}", trigger, e));
            log::error!("Failed to start recording: {}", e);
            {
                // Idle, not Error: pressing the hotkey again retries right away.
                let mut p = self.phase.lock().unwrap();
                *p = AssistantPhase::Idle;
            }
            crate::tray::set_tray_recording(&self.app_handle, false, self.is_model_loaded());
            let session = self.session_id.load(Ordering::SeqCst);
            let subtitle = format!("Couldn't open the microphone: {}. Check {}.", e.trim_end_matches('.'), MIC_SETTING);
            show_long_error(&self.app_handle, &self.session_id, session, "Recording Failed", &subtitle);
            return Err(e);
        }

        // A forgotten recording would grow by ~1 GB an hour and then time out in transcription.
        {
            let app = self.app_handle.clone();
            let session_id = Arc::clone(&self.session_id);
            let session = session_id.load(Ordering::SeqCst);
            tauri::async_runtime::spawn(async move {
                tokio::time::sleep(MAX_RECORDING).await;
                let Some(state) = app.try_state::<crate::AppState>() else { return };
                if session_id.load(Ordering::SeqCst) == session && state.controller.is_listening() {
                    log_stage_event(&get_data_dir(), "REC_LIMIT", &format!("Recording stopped at the {} min limit", MAX_RECORDING.as_secs() / 60));
                    let _ = state.controller.stop_listening_and_process("recording length limit");
                }
            });
        }

        // 5. Pre-warm active model asynchronously while user is speaking (0 wait on finish!), unless
        // the memory-saving mode defers it to when the recording stops.
        if settings.model_load_mode != "on_stop" {
            self.spawn_model_prewarm(&settings.model_name);
        }

        Ok(())
    }

    /// Loads the active speech model in the background (and unloads the other one). Transcription
    /// waits on the same lock, so starting this early hides the load behind whatever runs first.
    fn spawn_model_prewarm(&self, model_name: &str) {
        let app_handle_cl = self.app_handle.clone();
        let transcriber_cl = Arc::clone(&self.transcriber);
        let parakeet_cl = Arc::clone(&self.parakeet_transcriber);
        let model_name = model_name.to_string();

        tauri::async_runtime::spawn(async move {
            let is_parakeet = model_name == crate::model_download::PARAKEET_MODEL;
            if is_parakeet {
                // Single Model Policy: unload Whisper
                {
                    let mut tr = transcriber_cl.lock().unwrap();
                    tr.unload();
                }
                if let Ok(dir) = crate::parakeet::ParakeetTranscriber::ensure_model(&app_handle_cl).await {
                    {
                        let mut pk = parakeet_cl.lock().unwrap();
                        let _ = pk.prewarm(&dir);
                    }
                    crate::tray::update_tray_model_status(&app_handle_cl, true);
                    // On Apple Silicon, fetch the Neural Engine build once, then switch to it.
                    if crate::parakeet::ParakeetTranscriber::download_neural_engine_model(&app_handle_cl).await {
                        let dir = crate::parakeet::ParakeetTranscriber::get_model_dir();
                        let mut pk = parakeet_cl.lock().unwrap();
                        let _ = pk.prewarm(&dir);
                    }
                }
            } else {
                // Single Model Policy: unload Parakeet
                {
                    let mut pk = parakeet_cl.lock().unwrap();
                    pk.unload();
                }
                if let Ok(path) = crate::transcribe::Transcriber::ensure_model(&app_handle_cl, &model_name).await {
                    let mut tr = transcriber_cl.lock().unwrap();
                    let _ = tr.prewarm(&path);
                    crate::tray::update_tray_model_status(&app_handle_cl, true);
                }
            }
        });
    }

    fn stop_listening_and_process(&self, trigger: &'static str) -> Result<(), String> {
        let pipeline_start = Instant::now();
        {
            // Let a start in progress open the microphone first; if opening failed, there's
            // nothing to stop.
            let _started = self.start_gate.lock().unwrap();
            if !self.is_listening() {
                return Ok(());
            }
        }

        {
            let mut phase = self.phase.lock().unwrap();
            *phase = AssistantPhase::Transcribing;
        }
        crate::tray::set_tray_recording(&self.app_handle, false, self.is_model_loaded());
        // A mic warning may have enlarged the pill; processing states use the normal size.
        set_pill_window_size(&self.app_handle, PILL_WINDOW);

        {
            let mut t = self.last_activity.lock().unwrap();
            *t = Instant::now();
        }

        let settings = self.get_settings();
        // Memory-saving mode: load the model now, alongside leveling and VAD, instead of while
        // the user was talking. A no-op if it's still loaded from a recent recording.
        if settings.model_load_mode == "on_stop" {
            self.spawn_model_prewarm(&settings.model_name);
        }
        let target_norm = crate::translate::normalize_lang(&settings.target_lang);
        let transcribing_subtitle = if crate::lab::is_transcribing() {
            // It may first wait for Audio Lab to finish the piece it's on.
            "Audio Lab paused; may take a few seconds longer".to_string()
        } else if target_norm == "none" {
            "Voice to Text".to_string()
        } else {
            "Converting speech to text".to_string()
        };

        // Emit transcribing state immediately so UI updates instantaneously upon hotkey release
        let _ = self.app_handle.emit(
            "assistant-state-changed",
            serde_json::json!({
                "state": "transcribing",
                "title": "Transcribing…",
                "subtitle": transcribing_subtitle
            }),
        );

        // Stop earcon plays on key release; the recorder discards audio captured after this point,
        // so the chime is not transcribed.
        if settings.sound_stop {
            crate::sound::play_sound(crate::sound::AppSound::StopRecording);
        }

        let (recording_sec, input_device) = self
            .recording_start
            .lock()
            .unwrap()
            .as_ref()
            .map(|s| (s.at.elapsed().as_secs_f32(), Some((s.device.clone(), s.sample_rate))))
            .unwrap_or((0.0, None));
        let stop_started = Instant::now();
        let captured = self.recorder.stop_recording(crate::denoise::strength_wet(&settings.noise_reduction));
        let live_chunks = self.live_chunks.lock().unwrap().take();
        let mut perf = crate::perf::PerfRun::start(stop_started.elapsed().as_millis() as u64);
        perf.model = settings.model_name.clone();
        perf.model_load_mode = settings.model_load_mode.clone();
        perf.recording_sec = recording_sec;
        perf.input_device = input_device;
        log_recording_stop(&self.recording_start, trigger, &captured.stats);

        let app_handle = self.app_handle.clone();
        let phase_arc = Arc::clone(&self.phase);
        let transcriber_arc = Arc::clone(&self.transcriber);
        let parakeet_arc = Arc::clone(&self.parakeet_transcriber);
        let history_arc = Arc::clone(&self.history);
        let last_activity_arc = Arc::clone(&self.last_activity);
        let pipeline_logger_arc = Arc::clone(&self.pipeline_logger);
        let session_id_arc = Arc::clone(&self.session_id);
        let pending_retry_arc = Arc::clone(&self.pending_retry);
        let session = self.session_id.load(Ordering::SeqCst);
        let mode = *self.recording_mode.lock().unwrap();

        // Run processing in background thread using Tauri's async runtime
        let pipeline = async move {
            let is_cancelled = || session_id_arc.load(Ordering::SeqCst) != session;
            let data_dir = get_data_dir();
            if is_cancelled() {
                perf.set_outcome("cancelled");
                log_stage_event(&data_dir, "CANCELLED", "Aborted transcription: cancelled by user");
                return;
            }
            // Leveled 16 kHz speech; history gets the native-rate copy from `captured`.
            let raw_samples = &captured.speech;
            let duration_sec = raw_samples.len() as f32 / 16000.0;
            log_stage_event(&data_dir, "AUDIO", &format!("Captured {} samples ({:.2} s)", raw_samples.len(), duration_sec));
            let plan = captured.level_plan();
            log_stage_event(&data_dir, "LEVEL", &format!("Voice leveling: gain {:+.1} dB, make-up {:+.1} dB", plan.gain_db, plan.makeup_db));

            // Save full original audio (uncut) to latest_recording.wav for playback in UI
            let private = crate::settings::is_private(&settings.storage_mode);
            if !private {
                let _ = crate::history::write_wav_file(&data_dir.join("latest_recording.wav"), &raw_samples, 16000);
            }
            perf.audio_sec = duration_sec;
            perf.lap("audio_prep");

            if duration_sec < 0.2 {
                log_stage_event(&data_dir, "AUDIO", "Audio buffer too short (<0.2s), aborting transcription.");
                perf.set_outcome("too_short");
                fail_recording(&app_handle, &phase_arc, &session_id_arc, session, "Recording Failed", "Audio too short (held < 0.2s)");
                return;
            }

            let is_parakeet = settings.model_name == crate::model_download::PARAKEET_MODEL;

            // Pieces transcribed while recording (see live_chunks): only the audio after the last
            // cut goes to the model now.
            let live = match live_chunks {
                Some(live) => tokio::task::spawn_blocking(move || live.finish()).await.ok().flatten(),
                None => None,
            };
            let model_samples = match &live {
                Some(done) => {
                    // The same 16 kHz timeline as the live audio, give or take the resampler's few
                    // ms; the cut is in the middle of a 2 s pause.
                    let cut = done.cut.min(raw_samples.len());
                    log_stage_event(&data_dir, "LIVE_CHUNKS", &format!("{} piece(s), {:.1} s, transcribed while recording; transcribing the last {:.1} s now", done.pieces, cut as f32 / 16000.0, (raw_samples.len() - cut) as f32 / 16000.0));
                    &raw_samples[cut.saturating_sub(crate::live_chunks::OVERLAP)..]
                }
                None => &raw_samples[..],
            };
            perf.lap("live_chunks");

            // Step 1: Silero VAD silence trimming with 400ms padding. Skipped for Parakeet on the
            // Neural Engine: it takes the whole recording (see transcribe_on_neural_engine), and VAD
            // took 1.4 s on 3 minutes to trim nothing.
            let vad_res = if is_parakeet && crate::parakeet::ParakeetTranscriber::runs_on_neural_engine() {
                log_stage_event(&data_dir, "VAD", "VAD skipped: Parakeet on the Neural Engine takes the whole recording");
                crate::vad::untrimmed(model_samples, 16000)
            } else {
                let vad_res = trim_silence(model_samples, 16000);
                log_stage_event(&data_dir, "VAD", &format!("VAD completed in {} ms: original {:.2} s → trimmed {:.2} s (cut {:.2} s silence){}", vad_res.duration_ms, vad_res.original_duration_sec, vad_res.trimmed_duration_sec, vad_res.silence_removed_sec, if vad_res.no_speech { "; no speech found, kept the whole recording" } else { "" }));
                vad_res
            };

            // Save trimmed (VAD) audio to latest_vad_trimmed.wav for debugging
            if !private {
                let _ = crate::history::write_wav_file(&data_dir.join("latest_vad_trimmed.wav"), &vad_res.samples, 16000);
            }
            perf.speech_sec = vad_res.trimmed_duration_sec;
            perf.vad_no_speech = vad_res.no_speech;
            perf.lap("vad");

            if is_cancelled() {
                perf.set_outcome("cancelled");
                log_stage_event(&data_dir, "CANCELLED", "Aborted before model inference: cancelled by user");
                return;
            }

            // Saved before inference so a failed transcription still keeps the whole recording.
            let session_audio = session_audio_filename(&data_dir, &settings.storage_mode, &captured);
            perf.lap("save_audio");

            // Step 2 & 3: Model inference
            let (raw_text, detected_lang, whisper_inference_ms, thread_count, whisper_raw_output, segments_count) = if is_parakeet {
                // Single Model Policy: drop Whisper from RAM
                {
                    let mut tr = transcriber_arc.lock().unwrap();
                    tr.unload();
                }
                log_stage_event(&data_dir, "PARAKEET_START", &format!("Starting Parakeet TDT inference on {} samples ({:.2} s)...", vad_res.samples.len(), vad_res.trimmed_duration_sec));
                let model_dir = match ensure_model_for_recording(&app_handle, &settings.model_name, &transcribing_subtitle).await {
                    Ok(path) => {
                        log_stage_event(&data_dir, "MODEL", &format!("Using Parakeet model at {:?}", path));
                        path
                    },
                    Err(e) => {
                        let vitals = crate::vitals::SystemVitals::collect();
                        log_stage_event(&data_dir, "MODEL_ERROR", &format!("Failed to ensure Parakeet model: {}\n{}", e, vitals.format_report()));
                        perf.set_outcome("failed");
                        keep_failed_recording(&history_arc, &pipeline_logger_arc, &app_handle, &captured, &settings, &vad_res, duration_sec, &data_dir, &session_audio, pipeline_start.elapsed().as_millis() as u64, &format!("Model download failed: {}", e));
                        log::error!("Failed to ensure Parakeet model: {}", e);
                        fail_recording(&app_handle, &phase_arc, &session_id_arc, session, "Model Download Failed", &format!("{}. {}", e, vitals.short_summary()));
                        return;
                    }
                };

                perf.lap("model_ready");
                let parakeet_clone = Arc::clone(&parakeet_arc);
                let vad_samples = vad_res.samples.clone();
                let chunks = vad_res.ranges(settings.audio_chunking);
                let model_dir_clone = model_dir.clone();

                let timeout = inference_timeout(vad_res.trimmed_duration_sec);
                let pk_transcribe_res = tokio::time::timeout(
                    timeout,
                    tokio::task::spawn_blocking(move || {
                        let mut pk = parakeet_clone.lock().unwrap();
                        pk.transcribe(&vad_samples, &chunks, &model_dir_clone)
                    }),
                )
                .await;

                match pk_transcribe_res {
                    Ok(Ok(Ok(res))) => {
                        let raw_out = if res.0.is_empty() {
                            "Parakeet TDT returned empty string (0 tokens)".to_string()
                        } else {
                            format!("Parakeet TDT Output: \"{}\"", res.0)
                        };
                        log_stage_event(&data_dir, "PARAKEET_DONE", &format!("Parakeet inference took {} ms | text: \"{}\"", res.2, res.0));
                        perf.lap("inference");
                        (res.0, res.1, res.2, res.3, raw_out, 1)
                    },
                    Ok(Ok(Err(e))) => {
                        let vitals = crate::vitals::SystemVitals::collect();
                        log_stage_event(&data_dir, "PARAKEET_ERROR", &format!("Parakeet transcription error: {}\n{}", e, vitals.format_report()));
                        perf.set_outcome("failed");
                        keep_failed_recording(&history_arc, &pipeline_logger_arc, &app_handle, &captured, &settings, &vad_res, duration_sec, &data_dir, &session_audio, pipeline_start.elapsed().as_millis() as u64, &format!("Transcription failed: {}", e));
                        log::error!("Parakeet transcription error: {}", e);
                        fail_recording(&app_handle, &phase_arc, &session_id_arc, session, "Transcription Failed", &format!("{}. {}", e, vitals.short_summary()));
                        return;
                    },
                    Ok(Err(join_err)) => {
                        log::error!("Parakeet worker panicked: {:?}", join_err);
                        (String::new(), "auto".to_string(), 0, 4, format!("Parakeet Panic: {:?}", join_err), 0)
                    },
                    Err(_timeout) => {
                        let vitals = crate::vitals::SystemVitals::collect();
                        let vitals_report = vitals.format_report();
                        log_stage_event(&data_dir, "TRANSCRIPTION_TIMEOUT", &format!("Parakeet timed out after {}s!\n{}", timeout.as_secs(), vitals_report));
                        perf.set_outcome("failed");
                        keep_failed_recording(&history_arc, &pipeline_logger_arc, &app_handle, &captured, &settings, &vad_res, duration_sec, &data_dir, &session_audio, pipeline_start.elapsed().as_millis() as u64, &format!("Transcription timed out after {}s", timeout.as_secs()));
                        log::error!("Parakeet timed out after {}s!\n{}", timeout.as_secs(), vitals_report);

                        let vitals_log = data_dir.join("system_vitals_on_timeout.log");
                        let _ = std::fs::write(&vitals_log, format!("{}\nTimestamp: {}\nAudio Duration: {:.2}s\nModel: Parakeet TDT\n", vitals_report, current_timestamp(), vad_res.trimmed_duration_sec));

                        fail_recording(&app_handle, &phase_arc, &session_id_arc, session, "Transcription Timed Out", &format!("Parakeet {}s timeout. {}", timeout.as_secs(), vitals.short_summary()));
                        return;
                    }
                }
            } else {
                // Single Model Policy: drop Parakeet from RAM
                {
                    let mut pk = parakeet_arc.lock().unwrap();
                    pk.unload();
                }
                let model_path = match ensure_model_for_recording(&app_handle, &settings.model_name, &transcribing_subtitle).await {
                    Ok(path) => {
                        log_stage_event(&data_dir, "MODEL", &format!("Using Whisper model {} at {:?}", settings.model_name, path));
                        path
                    },
                    Err(e) => {
                        let vitals = crate::vitals::SystemVitals::collect();
                        log_stage_event(&data_dir, "MODEL_ERROR", &format!("Failed to ensure model {}: {}\n{}", settings.model_name, e, vitals.format_report()));
                        perf.set_outcome("failed");
                        keep_failed_recording(&history_arc, &pipeline_logger_arc, &app_handle, &captured, &settings, &vad_res, duration_sec, &data_dir, &session_audio, pipeline_start.elapsed().as_millis() as u64, &format!("Model download failed: {}", e));
                        log::error!("Failed to ensure Whisper model: {}", e);
                        fail_recording(&app_handle, &phase_arc, &session_id_arc, session, "Model Download Failed", &format!("{}. {}", e, vitals.short_summary()));
                        return;
                    }
                };

                perf.lap("model_ready");
                log_stage_event(&data_dir, "WHISPER_START", &format!("Starting transcription on {} samples ({:.2} s)...", vad_res.samples.len(), vad_res.trimmed_duration_sec));
                let samples_clone = vad_res.samples.clone();
                let chunks = vad_res.ranges(settings.audio_chunking);
                let model_path_clone = model_path.clone();
                let transcriber_clone = Arc::clone(&transcriber_arc);

                // After live pieces, the rest keeps their language.
                let lang_hint = live
                    .as_ref()
                    .and_then(|done| done.lang.clone())
                    .or_else(|| crate::transcribe::map_language_hint(&settings.source_lang).map(str::to_string));
                let timeout = inference_timeout(vad_res.trimmed_duration_sec);
                let inference_res = tokio::time::timeout(
                    timeout,
                    tokio::task::spawn_blocking(move || {
                        let mut tr = transcriber_clone.lock().unwrap();
                        tr.transcribe_detailed(&samples_clone, &chunks, &model_path_clone, false, lang_hint.as_deref())
                    }),
                )
                .await;

                match inference_res {
                    Ok(Ok(Ok(mut res))) => {
                        log_stage_event(&data_dir, "WHISPER_DONE", &format!("Inference took {} ms | detected_lang: \"{}\" | text: \"{}\"", res.inference_ms, res.detected_lang, res.text));
                        perf.lap("inference");

                        // Automatic re-recognition check for excluded languages:
                        if !settings.excluded_languages.trim().is_empty()
                            && crate::translate::is_language_excluded(&res.detected_lang, &res.text, &settings.excluded_languages)
                        {
                            let fallback_lang = crate::translate::get_fallback_language_for_excluded(
                                &settings.source_lang,
                                &settings.target_lang,
                                &res.text,
                                &settings.excluded_languages,
                            );

                            log_stage_event(
                                &data_dir,
                                "EXCLUDED_LANG_DETECTED",
                                &format!(
                                    "Detected excluded language '{}' (text: \"{}\"). Automatically re-transcribing with forced '{}'...",
                                    res.detected_lang, res.text, fallback_lang
                                ),
                            );

                            let samples_retry = vad_res.samples.clone();
                            let chunks_retry = vad_res.ranges(settings.audio_chunking);
                            let model_retry = model_path.clone();
                            let transcriber_retry = Arc::clone(&transcriber_arc);
                            let forced_hint = crate::transcribe::map_language_hint(&fallback_lang);

                            let retry_res = tokio::time::timeout(
                                timeout,
                                tokio::task::spawn_blocking(move || {
                                    let mut tr = transcriber_retry.lock().unwrap();
                                    tr.transcribe_detailed(&samples_retry, &chunks_retry, &model_retry, false, forced_hint)
                                }),
                            )
                            .await;

                            if let Ok(Ok(Ok(recovered))) = retry_res {
                                log_stage_event(
                                    &data_dir,
                                    "EXCLUDED_LANG_RECOVERED",
                                    &format!(
                                        "Successfully re-transcribed to forced '{}' in {} ms: \"{}\"",
                                        fallback_lang, recovered.inference_ms, recovered.text
                                    ),
                                );
                                res = recovered;
                            }
                        }

                        (res.text, res.detected_lang, res.inference_ms, res.thread_count, res.raw_output, res.segments.len())
                    },
                    Ok(Ok(Err(e))) => {
                        let vitals = crate::vitals::SystemVitals::collect();
                        log_stage_event(&data_dir, "WHISPER_ERROR", &format!("Whisper transcription error: {}\n{}", e, vitals.format_report()));
                        perf.set_outcome("failed");
                        keep_failed_recording(&history_arc, &pipeline_logger_arc, &app_handle, &captured, &settings, &vad_res, duration_sec, &data_dir, &session_audio, pipeline_start.elapsed().as_millis() as u64, &format!("Transcription failed: {}", e));
                        log::error!("Transcription error: {}", e);
                        fail_recording(&app_handle, &phase_arc, &session_id_arc, session, "Transcription Failed", &format!("{}. {}", e, vitals.short_summary()));
                        return;
                    },
                    Ok(Err(join_err)) => {
                        log::error!("Whisper worker panicked: {:?}", join_err);
                        (String::new(), "unknown".to_string(), 0, 4, format!("Whisper Panic: {:?}", join_err), 0)
                    },
                    Err(_timeout) => {
                        let vitals = crate::vitals::SystemVitals::collect();
                        let vitals_report = vitals.format_report();
                        log_stage_event(&data_dir, "TRANSCRIPTION_TIMEOUT", &format!("Transcription timed out after {}s!\n{}", timeout.as_secs(), vitals_report));
                        perf.set_outcome("failed");
                        keep_failed_recording(&history_arc, &pipeline_logger_arc, &app_handle, &captured, &settings, &vad_res, duration_sec, &data_dir, &session_audio, pipeline_start.elapsed().as_millis() as u64, &format!("Transcription timed out after {}s", timeout.as_secs()));
                        log::error!("Transcription timed out after {}s!\n{}", timeout.as_secs(), vitals_report);

                        let vitals_log_path = data_dir.join("system_vitals_on_timeout.log");
                        let _ = std::fs::write(
                            &vitals_log_path,
                            format!("{}\nTimestamp: {}\nAudio Duration: {:.2}s\nModel: {}\n",
                                vitals_report, current_timestamp(), vad_res.trimmed_duration_sec, settings.model_name)
                        );

                        fail_recording(&app_handle, &phase_arc, &session_id_arc, session, "Transcription Timed Out", &format!("{}s timeout on {}. {}", timeout.as_secs(), settings.model_name, vitals.short_summary()));
                        return;
                    }
                }
            };
            let raw_text = match &live {
                // A rest with no speech adds nothing: Whisper makes up words ("Thank you.") on silence.
                Some(done) if vad_res.no_speech || raw_text == "[BLANK_AUDIO]" => done.text.clone(),
                Some(done) => crate::chunking::join_overlapping(&[done.text.as_str(), raw_text.as_str()]),
                None => raw_text,
            };

            crate::tray::update_tray_model_status(&app_handle, true);

            if raw_text.is_empty() || raw_text == "[BLANK_AUDIO]" {
                let (chunks, mut actions) = chunk_diagnostics(
                    settings.audio_chunking,
                    duration_sec,
                    &vad_res,
                    raw_samples.len(),
                );
                actions.push("[RESULT] Audio was blank or below confidence threshold".to_string());

                let empty_log = TranscriptionDiagnosticLog {
                    id: pipeline_logger_arc.next_id(),
                    timestamp: current_timestamp(),
                    audio_duration_sec: duration_sec,
                    audio_samples_count: raw_samples.len(),
                    vad_trim_ms: vad_res.duration_ms,
                    vad_original_sec: vad_res.original_duration_sec,
                    vad_trimmed_sec: vad_res.trimmed_duration_sec,
                    vad_silence_removed_sec: vad_res.silence_removed_sec,
                    model_name: settings.model_name.clone(),
                    gpu_metal_active: Transcriber::is_metal_supported(),
                    threads_count: thread_count,
                    whisper_inference_ms,
                    whisper_speed_factor: 0.0,
                    detected_lang: detected_lang.clone(),
                    raw_text: if raw_text.is_empty() { "[No speech recognized]".to_string() } else { raw_text.clone() },
                    translation_skipped: true,
                    translation_skip_reason: "Empty transcription / Blank audio".to_string(),
                    translation_ms: 0,
                    translation_error: None,
                    translation_provider: None,
                    final_text: String::new(),
                    spoken_text: None,
                    clipboard_paste_ms: 0,
                    history_save_ms: 0,
                    total_pipeline_ms: pipeline_start.elapsed().as_millis() as u64,
                    audio_filename: session_audio.clone(),
                    vad_audio_filename: Some("latest_vad_trimmed.wav".to_string()),
                    whisper_raw_output: Some(whisper_raw_output.clone()),
                    segments_count,
                    chunk_events: chunks,
                    whole_track: !settings.audio_chunking,
                    action_logs: actions,
                    history_id: None,
                    transcription_error: None,
                };
                perf.set_outcome("empty");
                pipeline_logger_arc.add_log(empty_log.clone(), &data_dir);
                let _ = app_handle.emit("transcription-diagnostic-log", &empty_log);

                let (title, subtitle) = if is_digital_silence(&raw_samples) {
                    // The OS delivers zeros instead of an error when microphone access is blocked.
                    log_stage_event(&data_dir, "AUDIO", "Recording is digital silence: microphone blocked or muted");
                    ("Microphone Is Silent", "Allow mic access in privacy settings")
                } else if raw_text == "[BLANK_AUDIO]" {
                    ("Transcription Failed", "Blank audio — No voice detected")
                } else {
                    ("Transcription Failed", "No speech recognized in recording")
                };
                fail_recording(&app_handle, &phase_arc, &session_id_arc, session, title, subtitle);
                return;
            }

            if is_cancelled() {
                perf.set_outcome("cancelled");
                log_stage_event(&data_dir, "CANCELLED", "Aborted before language detection / translation: cancelled by user");
                return;
            }

            // Step 3.5: LLM-friendly post-processing (wraps text_normalizer)
            let post_config = crate::post_processor::PostProcessorConfig {
                normalization: crate::text_normalizer::NormalizationOptions {
                    enabled: settings.text_normalization,
                    remove_fillers: settings.remove_filler_words,
                    convert_numbers: settings.convert_numbers,
                    remove_stutters: settings.remove_stutters,
                },
                apply_self_corrections: settings.apply_self_corrections,
                remove_noise_markers: settings.remove_noise_markers,
                collapse_redundancy: settings.collapse_redundancy,
                annotate_ambiguity: settings.annotate_ambiguity,
                normalize_structured_values: settings.normalize_structured_values,
            };
            let post_result = crate::post_processor::post_process(&raw_text, post_config);
            let normalized_source_text = if !post_result.clean_text.is_empty() {
                post_result.clean_text.clone()
            } else {
                raw_text.clone()
            };

            if normalized_source_text != raw_text {
                log_stage_event(
                    &data_dir,
                    "POST_PROCESSOR",
                    &format!(
                        "Post-processed transcript:\n  Raw: \"{}\"\n  Clean: \"{}\"{}",
                        raw_text,
                        normalized_source_text,
                        if post_result.requires_clarification {
                            format!("\n  ⚠ Uncertain spans: {:?}", post_result.uncertain_spans)
                        } else {
                            String::new()
                        }
                    ),
                );
            }

            // Step 3.6: Confidential text masking and redaction
            let mask_config = mask_config_from(&settings);

            let pre_translate_text = if mask_config.enabled {
                let masked = crate::masker::apply_masking(&normalized_source_text, &mask_config);
                if masked != normalized_source_text {
                    log_stage_event(
                        &data_dir,
                        "CONFIDENTIAL_MASK",
                        &format!("Masked confidential tokens before translation: \"{}\"", masked),
                    );
                }
                masked
            } else {
                normalized_source_text.clone()
            };

            // Step 4: Language detection and instant skip check (0.0001s, 0 network calls)
            // Reconcile Whisper's own LID (98 langs incl. uk) with text-based heuristic:
            // - Whisper is authoritative when it returns a non-auto language
            // - Fall back to detect_text_language only when Whisper says auto/unknown
            let whisper_detected = normalize_lang(&detected_lang);
            let text_detected = crate::translate::detect_text_language(&normalized_source_text);
            let detected_effective = if whisper_detected == "unknown" || whisper_detected == "auto" {
                text_detected.clone()
            } else if (whisper_detected == "russian" || whisper_detected == "ukrainian")
                && text_detected != "unknown"
                && text_detected != whisper_detected
            {
                // Ukrainian is the most common confusion: preserve text signal on uk/ru disagreement
                // Prefer the text heuristic when cyrillic script + glyph check contradicts Whisper.
                if text_detected == "ukrainian" || text_detected == "russian" {
                    crate::translate::detect_text_language(&normalized_source_text)
                } else {
                    whisper_detected.clone()
                }
            } else {
                whisper_detected.clone()
            };
            // Keep detected_lang as the canonical value for logs/history (what Whisper reported)
            // but use detected_effective for skip/translate decisions
            log_stage_event(&data_dir, "LANG_DETECT", &format!("Whisper='{}' text='{}' → effective='{}'", detected_lang, text_detected, detected_effective));
            // Post-processing, the empty/silence checks and language detection.
            perf.lap("post_process");
            perf.inference_ms = Some(whisper_inference_ms as u64);
            perf.threads = Some(thread_count as i32);

            let target_norm = crate::translate::normalize_lang(&settings.target_lang);
            let wants_english = target_norm == "english";
            let is_english = crate::translate::normalize_lang(&detected_effective) == "english";

            let provider_norm = settings.translation_provider.to_lowercase();
            let is_no_trans = settings.translation_off()
                || provider_norm.contains("no translation")
                || provider_norm == "none"
                || target_norm == "none";

            let route = TranslationRoute::from_settings(&settings);

            let can_translate = !is_no_trans && route.is_ready(&settings);

            // The translate hotkey translates whatever was spoken; only automatic translation is
            // limited to the spoken language set in settings.
            let skip_check = if mode == RecordingMode::Translate {
                crate::translate::normalize_lang(&detected_effective) == target_norm
            } else {
                should_skip(&normalized_source_text, &detected_effective, &settings.source_lang, &settings.target_lang)
            };

            let source_norm = crate::translate::normalize_lang(&settings.source_lang);
            let (skip_translation, skip_reason) = if is_no_trans {
                (true, "No translation mode (Voice-to-Text only)".to_string())
            } else if mode.transcribe_only(&settings) {
                (true, "Transcription-only hotkey (translation uses the translate hotkey)".to_string())
            } else if skip_check {
                if crate::translate::normalize_lang(&detected_effective) == target_norm {
                    (true, format!("Already in target language ({})", settings.target_lang))
                } else if source_norm != "auto" && source_norm != "unknown" && crate::translate::normalize_lang(&detected_effective) != source_norm {
                    (true, format!("Spoken '{}' does not match configured source language '{}'", detected_effective, settings.source_lang))
                } else {
                    (true, "Language check skipped translation".to_string())
                }
            } else if !can_translate && !wants_english {
                (true, format!("Translation provider '{}' is not configured", settings.translation_provider))
            } else {
                (false, "Translation required".to_string())
            };

            let mut translation_failed = false;
            let mut translation_fail_reason = String::new();
            // Full description for the diagnostic log: languages, provider, model, endpoint and error.
            let mut translation_error_detail: Option<String> = None;
            // Only service translations can be retried; local Whisper translation needs the audio.
            let mut retryable = false;
            // The service that translated (the fallback's when Apple Translation failed over to it).
            let mut translated_via: Option<(String, &'static str, Option<String>, Option<String>)> = None;

            let trans_start = Instant::now();
            let (final_text, translation_ms) = if skip_translation {
                log_stage_event(&data_dir, "TRANSLATE_SKIP", &format!("Skipped translation: {}", skip_reason));
                (pre_translate_text.clone(), 0)
            } else if can_translate {
                let from_name = crate::translate::format_lang_name(&detected_effective);
                let to_name = crate::translate::format_lang_name(&settings.target_lang);
                let provider_label = route.label();

                log_stage_event(
                    &data_dir,
                    "TRANSLATE_START",
                    &format!(
                        "Translating from {} to {} with {}: \"{}\"",
                        from_name, to_name, route.describe(&settings), pre_translate_text
                    ),
                );
                // Step 5A: Translate with configured provider
                {
                    let mut p = phase_arc.lock().unwrap();
                    *p = AssistantPhase::Translating;
                }

                let _ = app_handle.emit(
                    "assistant-state-changed",
                    serde_json::json!({
                        "state": "translating",
                        "title": "Translating…",
                        "subtitle": format!("{} → {} ({})", from_name, to_name, provider_label)
                    }),
                );

                let (trans_result, ms) = match route.translate(&settings, &pre_translate_text, &detected_effective).await {
                    Ok(t) => {
                        let ms = trans_start.elapsed().as_millis() as u64;
                        log_stage_event(
                            &data_dir,
                            "TRANSLATE_DONE",
                            &format!(
                                "{} translated in {} ms ({} → {} chars): \"{}\"",
                                t.via,
                                ms,
                                pre_translate_text.chars().count(),
                                t.text.chars().count(),
                                t.text
                            ),
                        );
                        translated_via = Some((t.via, t.label, t.pair, t.model));
                        (t.text, ms)
                    }
                    Err(e) => {
                        let ms = trans_start.elapsed().as_millis() as u64;
                        let vitals = crate::vitals::SystemVitals::collect();
                        let detail = format!(
                            "{} → {} via {} failed after {} ms: {}",
                            from_name, to_name, route.describe(&settings), ms, e
                        );
                        log_stage_event(&data_dir, "TRANSLATE_ERROR", &format!("{}\n{}", detail, vitals.format_report()));
                        log::error!("Translation failed: {}. Pasting the spoken text instead.", detail);
                        translation_failed = true;
                        translation_fail_reason = e;
                        translation_error_detail = Some(detail);
                        retryable = true;
                        (pre_translate_text.clone(), 0)
                    }
                };
                (trans_result, ms)
            } else if wants_english && !is_english {
                log_stage_event(&data_dir, "TRANSLATE_WHISPER_START", &format!("Translating locally to English in RAM: \"{}\"", raw_text));
                // Step 5B: Local Whisper translation to English in RAM (0 API key needed)
                {
                    let mut p = phase_arc.lock().unwrap();
                    *p = AssistantPhase::Translating;
                }

                let _ = app_handle.emit(
                    "assistant-state-changed",
                    serde_json::json!({
                        "state": "translating",
                        "title": "Translating locally…",
                        "subtitle": format!("{} → English (Whisper AI)", detected_lang)
                    }),
                );

                let whisper_model_path = Transcriber::get_model_path(
                    if is_parakeet { "ggml-base.bin" } else { &settings.model_name }
                );

                let (translated, _, _, _) = {
                    let mut tr = transcriber_arc.lock().unwrap();
                    match tr.transcribe(&vad_res.samples, &vad_res.ranges(settings.audio_chunking), &whisper_model_path, true) {
                        Ok(res) => res,
                        Err(e) => {
                            let detail = format!("{} → English via local Whisper ({:?}) failed: {}", detected_lang, whisper_model_path, e);
                            log_stage_event(&data_dir, "TRANSLATE_WHISPER_ERROR", &detail);
                            log::error!("Local translation failed: {}. Pasting the spoken text instead.", detail);
                            translation_failed = true;
                            translation_fail_reason = format!("local Whisper translation failed: {}", e);
                            translation_error_detail = Some(detail);
                            (pre_translate_text.clone(), detected_lang.clone(), 0, 4)
                        }
                    }
                };

                let ms = trans_start.elapsed().as_millis() as u64;
                log_stage_event(&data_dir, "TRANSLATE_WHISPER_DONE", &format!("Local translation finished in {} ms: \"{}\"", ms, translated));
                (translated, ms)
            } else {
                (pre_translate_text.clone(), 0)
            };

            // Step 5.5: Final confidential text masking on outgoing text
            let final_text = if mask_config.enabled {
                crate::masker::apply_masking(&final_text, &mask_config)
            } else {
                final_text
            };

            if is_cancelled() {
                perf.set_outcome("cancelled");
                log_stage_event(&data_dir, "CANCELLED", "Aborted before paste/done: cancelled by user");
                return;
            }

            // Kept in the diagnostic log so Audio Lab's second try can show what was actually said.
            let spoken_text = (!skip_translation && !translation_failed && pre_translate_text.trim() != final_text.trim())
                .then(|| pre_translate_text.clone());

            // Step 6: Set phase. A failed translation goes straight back to Idle so the hotkey works
            // while its error is still on screen.
            {
                let mut p = phase_arc.lock().unwrap();
                if translation_failed {
                    *p = AssistantPhase::Idle;
                } else {
                    *p = AssistantPhase::Done;
                }
            }

            perf.lap("translation");
            perf.translation = (!skip_translation).then(|| {
                translated_via.as_ref().map(|(_, label, _, _)| *label).unwrap_or(route.label()).to_string()
            });
            if can_translate && !skip_translation {
                perf.translation_pair = translated_via.as_ref().and_then(|(_, _, pair, _)| pair.clone());
                perf.translation_model = translated_via.as_ref().and_then(|(_, _, _, model)| model.clone());
                perf.translation_ms = Some(translation_ms);
                perf.translation_chars = Some((
                    pre_translate_text.chars().count(),
                    (!translation_failed).then(|| final_text.chars().count()).unwrap_or(0),
                ));
            }
            perf.text_chars = final_text.chars().count();

            // Step 7: Copy to clipboard and auto-paste if enabled
            let paste_start = Instant::now();
            // Err: not pasted (the text is on the clipboard). Ok(Some): pasted with a fallback
            // that may miss some apps; the note says what to do.
            let pasted = if settings.auto_paste {
                copy_and_paste(&final_text, settings.keep_clipboard)
            } else {
                crate::paste::copy_to_clipboard(&final_text).map(|_| None)
            };
            let paste_ms = paste_start.elapsed().as_millis() as u64;
            match &pasted {
                Ok(_) => log_stage_event(&data_dir, "PASTE", &format!("Auto-paste {} in {} ms", if settings.auto_paste { "dispatched" } else { "skipped" }, paste_ms)),
                Err(e) => log_stage_event(&data_dir, "PASTE_FAILED", &format!("After {} ms: {}", paste_ms, e)),
            }
            perf.lap("paste");
            let paste_message = match &pasted {
                Err(e) => Some(("Couldn't Paste", e.clone())),
                Ok(Some(note)) => Some(("Check That It Pasted", note.clone())),
                Ok(None) => None,
            };
            if paste_message.is_some() && !translation_failed {
                // Back to Idle now, so the hotkey works while the message is on screen.
                *phase_arc.lock().unwrap() = AssistantPhase::Idle;
            }

            // A failed translation is announced after it's saved to history (the retry needs its id).
            if let (Some((title, subtitle)), false) = (&paste_message, translation_failed) {
                show_long_error_payload(
                    &app_handle,
                    &session_id_arc,
                    session,
                    serde_json::json!({ "state": "error", "title": title, "subtitle": subtitle, "long": true }),
                );
            } else if !translation_failed {
                let _ = app_handle.emit(
                    "assistant-state-changed",
                    serde_json::json!({
                        "state": "done",
                        "title": "Copied to clipboard",
                        "subtitle": null,
                        "text": final_text
                    }),
                );
            }

            // Step 8: Save to history (save full original recorded audio, uncut!)
            let db_start = Instant::now();
            let history_source_text = if mask_config.enabled {
                crate::masker::apply_masking(&raw_text, &mask_config)
            } else {
                raw_text.clone()
            };
            let history_id = history_arc
                .add_entry(
                    &history_source_text,
                    &final_text,
                    &detected_lang,
                    &settings.target_lang,
                    duration_sec,
                    Some(&captured),
                    &settings,
                )
                .unwrap_or(0);
            let history_ms = db_start.elapsed().as_millis() as u64;
            log_stage_event(&data_dir, "DB", &format!("Saved to database in {} ms", history_ms));
            perf.lap("history_db");

            // Remember a failed service translation so it can be retried from the pill or the tray.
            let pending = (translation_failed && retryable).then(|| PendingRetry {
                text: pre_translate_text.clone(),
                from_lang: detected_effective.clone(),
                history_id,
            });
            let can_retry = pending.is_some();
            let retry_label = pending
                .as_ref()
                .map(|p| crate::tray::retry_label(&p.from_lang, &settings.target_lang, &p.text));
            *pending_retry_arc.lock().unwrap() = pending;
            crate::tray::set_retry_item(retry_label.as_deref());

            if translation_failed {
                let what_was_pasted = if settings.auto_paste { "pasted" } else { "copied" };
                let subtitle = if can_retry {
                    format!("{}. Your spoken text was {} instead. Retry, or check your translation settings.", capitalize(&translation_fail_reason), what_was_pasted)
                } else {
                    format!("{}. Your spoken text was {} instead.", capitalize(&translation_fail_reason), what_was_pasted)
                };
                show_long_error_payload(
                    &app_handle,
                    &session_id_arc,
                    session,
                    serde_json::json!({
                        "state": "error",
                        "title": "Translation Failed",
                        "subtitle": subtitle,
                        "long": true,
                        "retry": can_retry
                    }),
                );
            }

            let total_pipeline_ms = pipeline_start.elapsed().as_millis() as u64;
            let speed_factor = if whisper_inference_ms > 0 {
                (vad_res.trimmed_duration_sec * 1000.0) / (whisper_inference_ms as f32)
            } else {
                0.0
            };
            log_stage_event(&data_dir, "COMPLETE", &format!("Pipeline complete in {} ms! Speed factor: {:.1}x", total_pipeline_ms, speed_factor));
            perf.lap("finalize");
            perf.finish(if translation_failed { "translation_failed" } else { "ok" });

            let (chunks, mut actions) = chunk_diagnostics(
                    settings.audio_chunking,
                    duration_sec,
                    &vad_res,
                    raw_samples.len(),
            );
            actions.push(format!("[MODEL] Transcribed with {} in {} ms ({:.1}x real-time)", settings.model_name, whisper_inference_ms, speed_factor));
            if skip_translation {
                actions.push(format!("[TRANSLATION] Skipped ({})", skip_reason));
            } else if let Some(detail) = &translation_error_detail {
                actions.push(format!("[TRANSLATION] FAILED: {}", detail));
                actions.push("[TRANSLATION] Pasted the spoken text instead".to_string());
                if can_retry {
                    actions.push("[TRANSLATION] Retry available from the pill or the tray menu".to_string());
                }
            } else if can_translate {
                let via = translated_via.as_ref().map(|(via, _, _, _)| via.clone()).unwrap_or_else(|| route.describe(&settings));
                actions.push(format!(
                    "[TRANSLATION] Translated in {} ms via {} ({} → {} chars)",
                    translation_ms,
                    via,
                    pre_translate_text.chars().count(),
                    final_text.chars().count()
                ));
            } else {
                actions.push(format!("[TRANSLATION] Translated locally in {} ms via Whisper", translation_ms));
            }
            match &pasted {
                Err(e) => actions.push(format!("[DISPATCH] Paste failed after {} ms: {}", paste_ms, e)),
                Ok(_) => actions.push(format!("[DISPATCH] Pasted output in {} ms", paste_ms)),
            }
            actions.push(format!("[COMPLETE] Total pipeline finished in {} ms", total_pipeline_ms));

            // Step 9: Diagnostic Logging (stdout report, file write, in-memory ring buffer, Tauri event)
            let log_entry = TranscriptionDiagnosticLog {
                id: pipeline_logger_arc.next_id(),
                timestamp: current_timestamp(),
                audio_duration_sec: duration_sec,
                audio_samples_count: raw_samples.len(),
                vad_trim_ms: vad_res.duration_ms,
                vad_original_sec: vad_res.original_duration_sec,
                vad_trimmed_sec: vad_res.trimmed_duration_sec,
                vad_silence_removed_sec: vad_res.silence_removed_sec,
                model_name: settings.model_name.clone(),
                gpu_metal_active: Transcriber::is_metal_supported(),
                threads_count: thread_count,
                whisper_inference_ms,
                whisper_speed_factor: speed_factor,
                detected_lang,
                raw_text: raw_text.clone(),
                translation_skipped: skip_translation,
                translation_skip_reason: skip_reason,
                translation_ms,
                translation_error: translation_error_detail.clone(),
                translation_provider: (!skip_translation).then(|| match &translated_via {
                    Some((via, _, _, _)) => via.clone(),
                    None if can_translate => route.describe(&settings),
                    None => "Whisper (local)".to_string(),
                }),
                final_text: final_text.clone(),
                spoken_text,
                clipboard_paste_ms: paste_ms,
                history_save_ms: history_ms,
                total_pipeline_ms,
                audio_filename: session_audio.clone(),
                vad_audio_filename: Some("latest_vad_trimmed.wav".to_string()),
                whisper_raw_output: Some(whisper_raw_output),
                segments_count,
                chunk_events: chunks,
                whole_track: !settings.audio_chunking,
                action_logs: actions,
                history_id: (history_id > 0).then_some(history_id),
                transcription_error: None,
            };

            pipeline_logger_arc.add_log(log_entry.clone(), &get_data_dir());
            let _ = app_handle.emit("transcription-diagnostic-log", &log_entry);

            // A failed translation or paste hides itself after LONG_ERROR_HOLD (see show_long_error_payload).
            if translation_failed || paste_message.is_some() {
                return;
            }
            // Show the done pill briefly, then revert to Idle and hide the window.
            tokio::time::sleep(Duration::from_millis(600)).await;
            {
                let mut p = phase_arc.lock().unwrap();
                if *p == AssistantPhase::Done || *p == AssistantPhase::Error {
                    *p = AssistantPhase::Idle;
                    *last_activity_arc.lock().unwrap() = Instant::now();
                    let _ = app_handle.emit(
                        "assistant-state-changed",
                        serde_json::json!({
                            "state": "idle",
                            "title": "Ready",
                            "subtitle": null
                        }),
                    );
                    hide_main_window(&app_handle);
                }
            }
        };
        tauri::async_runtime::spawn(async move {
            pipeline.await;
            // The recording's buffers are freed once the pipeline is done with them.
            crate::memory::trim_freed_memory();
        });

        Ok(())
    }

    pub fn set_audio_device(&self, kind: &str, name: &str) -> Result<(), String> {
        let mut settings = self.get_settings();
        let opt_name = if name == "Default" {
            None
        } else {
            Some(name.to_string())
        };
        if kind == "input" {
            settings.input_device = opt_name;
        } else if kind == "output" {
            settings.output_device = opt_name;
        }
        self.save_settings(settings)
    }

    /// The chosen microphone isn't connected: nothing is recorded; the pill lists the connected
    /// mics instead of silently recording from the system default.
    fn offer_mic_choice(&self, trigger: &str, mode: RecordingMode, missing: &str, settings: &AppSettings) {
        let devices = crate::audio::list_input_devices();
        log_stage_event(
            &get_data_dir(),
            "REC_BLOCKED",
            &format!(
                "Recording requested by {} ({:?} mode) not started: chosen microphone \"{}\" isn't connected; asked which to use (connected: {})",
                trigger,
                mode,
                missing,
                if devices.is_empty() { "none".to_string() } else { devices.join(", ") }
            ),
        );
        let subtitle = if devices.is_empty() {
            format!("\"{missing}\" isn't connected, and no other microphone is. Connect one, then press the hotkey again.")
        } else {
            format!("\"{missing}\" isn't connected. Pick a microphone to record with; it becomes your microphone in Settings.")
        };
        show_main_window(&self.app_handle, settings);
        let session = self.session_id.load(Ordering::SeqCst);
        show_long_message(
            &self.app_handle,
            &self.session_id,
            session,
            serde_json::json!({
                "state": "error",
                "title": "Microphone Not Found",
                "subtitle": subtitle,
                "long": true,
                "devices": devices,
            }),
            MIC_CHOICE_HOLD,
        );
    }

    /// A microphone picked in the pill after the chosen one wasn't found: saves it, then records
    /// what the hotkey press asked for.
    pub fn choose_microphone(&self, name: &str) -> Result<(), String> {
        self.set_audio_device("input", name)?;
        log_stage_event(&get_data_dir(), "MIC_CHOSEN", &format!("Microphone \"{}\" picked in the pill and saved", name));
        if *self.phase.lock().unwrap() != AssistantPhase::Idle {
            return Ok(());
        }
        self.start_listening("microphone pick in the pill")
    }

    pub fn set_target_lang(&self, lang: &str) -> Result<(), String> {
        let mut settings = self.get_settings();
        settings.target_lang = lang.to_string();
        self.save_settings(settings)
    }

    pub fn set_translation_provider(&self, provider: &str) -> Result<(), String> {
        let mut settings = self.get_settings();
        settings.translation_provider = provider.to_string();
        self.save_settings(settings)
    }
}

#[cfg(test)]
mod tests {
    use super::{apple_setup_problem, capitalize, is_digital_silence, list_missing, mic_problem_message, missing_translation_settings, TranslationRoute};
    use crate::settings::AppSettings;

    #[test]
    fn translation_route_prefers_explicit_llm_settings() {
        let mut s = AppSettings::default();
        s.translation_provider = "LLM".into();
        s.llm_endpoint = " http://127.0.0.1:11434/v1/chat/completions ".into();
        s.llm_api_key = " key-1 ".into();
        s.llm_model = "llama3.2".into();
        let route = TranslationRoute::from_settings(&s);
        assert_eq!(route.endpoint, "http://127.0.0.1:11434/v1/chat/completions");
        assert_eq!(route.key, "key-1");
        assert_eq!(route.model, "llama3.2");
        assert_eq!(route.describe(&s), "LLM llama3.2 at http://127.0.0.1:11434/v1/chat/completions");
    }

    #[test]
    fn translation_route_custom_api_needs_its_model() {
        let mut s = AppSettings::default();
        s.translation_provider = "Custom API".into();
        s.custom_api_url = "https://api.example.com/v1/chat/completions".into();
        s.custom_api_model = String::new();
        let route = TranslationRoute::from_settings(&s);
        assert_eq!(route.label(), "Custom API");
        assert_eq!(route.model, "");
        assert!(missing_translation_settings(&s).contains(&"model name"));
    }

    #[tokio::test]
    async fn translation_route_reports_an_unreachable_service_as_an_error() {
        let mut s = AppSettings::default();
        s.translation_provider = "LLM".into();
        // Port 9 (discard) is closed on test machines, so the request fails right away.
        s.llm_endpoint = "http://127.0.0.1:9/v1/chat/completions".into();
        s.llm_model = "test-model".into();
        let err = TranslationRoute::from_settings(&s).translate(&s, "Привіт", "ukrainian").await.map(|t| t.text).unwrap_err();
        assert!(!err.is_empty());
    }

    fn apple_settings() -> AppSettings {
        let mut s = AppSettings::default();
        s.translation_provider = "Apple".into();
        s.source_lang = "Ukrainian".into();
        s.target_lang = "English".into();
        s
    }

    #[test]
    fn apple_intelligence_routes_as_an_llm_without_fields_and_can_be_apples_fallback() {
        let mut s = apple_settings();
        s.translation_provider = "Apple Intelligence".into();
        let route = TranslationRoute::from_settings(&s);
        assert!(route.is_intelligence && !route.is_apple && !route.is_custom);
        assert_eq!(route.label(), "Apple Intelligence");
        assert_eq!(route.describe(&s), "Apple Intelligence (on-device)");
        assert!(missing_translation_settings(&s).is_empty());

        let mut apple = apple_settings();
        apple.apple_fallback_provider = "Apple Intelligence".into();
        let fallback = TranslationRoute::from_settings(&apple).fallback(&apple).expect("needs no fields");
        assert!(fallback.is_intelligence);
    }

    #[test]
    fn apple_provider_routes_to_apple_translation() {
        let s = apple_settings();
        let route = TranslationRoute::from_settings(&s);
        assert!(route.is_apple && !route.is_custom);
        assert_eq!(route.label(), "Apple");
        assert_eq!(route.describe(&s), "Apple Translation (on-device)");
        // Apple's provider has no endpoint, key or model to fill in.
        assert!(missing_translation_settings(&s).is_empty());
        assert!(route.fallback(&s).is_none());

        // The LLM providers are untouched by the new one.
        let mut llm = s.clone();
        llm.translation_provider = "LLM".into();
        assert!(!TranslationRoute::from_settings(&llm).is_apple);
        llm.translation_provider = "Custom API".into();
        assert!(TranslationRoute::from_settings(&llm).is_custom);
    }

    #[test]
    fn offline_provider_routes_like_apple_with_its_own_fallback() {
        let mut s = apple_settings();
        s.translation_provider = "Offline".into();
        let route = TranslationRoute::from_settings(&s);
        assert!(route.is_offline && !route.is_apple && !route.is_custom && !route.is_intelligence);
        assert_eq!(route.label(), "Offline");
        assert!(missing_translation_settings(&s).is_empty());
        assert!(route.is_ready(&s));
        assert!(route.fallback(&s).is_none());

        s.apple_fallback_provider = "LLM".into();
        s.llm_endpoint = "http://localhost:11434/v1/chat/completions".into();
        s.llm_model = "llama3.2".into();
        assert_eq!(route.fallback(&s).map(|f| f.label()), Some("LLM"));
        // With a fallback, a missing language pack doesn't stop a recording.
        assert_eq!(apple_setup_problem(&s), None);
        s.apple_fallback_provider = "Offline".into();
        assert!(route.fallback(&s).is_none(), "it can't fall back to itself");
    }

    #[test]
    fn apple_falls_back_only_to_a_service_that_is_set_up() {
        let mut s = apple_settings();
        s.apple_fallback_provider = "LLM".into();
        s.llm_endpoint = "http://localhost:11434/v1/chat/completions".into();
        s.llm_model = "llama3.2".into();
        let route = TranslationRoute::from_settings(&s);
        let fallback = route.fallback(&s).expect("a local LLM needs no key");
        assert_eq!(fallback.label(), "LLM");
        assert!(route.is_ready(&s));
        // With a fallback, nothing about Apple Translation stops a recording.
        assert_eq!(apple_setup_problem(&s), None);

        s.apple_fallback_provider = "Custom API".into();
        s.custom_api_key = String::new();
        assert!(route.fallback(&s).is_none(), "Custom API without a key");
        s.apple_fallback_provider = "Apple".into();
        assert!(route.fallback(&s).is_none());
    }

    #[test]
    fn apple_setup_problem_explains_why_it_cannot_translate() {
        let s = apple_settings();
        if crate::apple_translate::is_available() {
            // On a Mac with macOS 26 the answer depends on which languages are downloaded.
            return;
        }
        // Tests run without the bundled helper (or on an older macOS), which is reported.
        let problem = apple_setup_problem(&s).expect("the provider can't run here");
        assert!(problem.contains("macOS") || problem.contains("helper"), "{problem}");
        assert!(!TranslationRoute::from_settings(&s).is_ready(&s));

        let mut llm = s.clone();
        llm.translation_provider = "LLM".into();
        assert_eq!(apple_setup_problem(&llm), None);
    }

    #[tokio::test]
    async fn a_failed_apple_translation_reports_the_reason_and_tries_the_fallback() {
        // A target Apple has no code for fails before the helper is asked, on any Mac.
        let mut s = apple_settings();
        s.target_lang = "Klingon language".into();
        let route = TranslationRoute::from_settings(&s);
        let err = route.translate(&s, "Привіт, як справи?", "ukrainian").await.map(|t| t.text).unwrap_err();
        assert_eq!(err, "Apple Translation can't translate into Klingon language");

        // Port 9 is closed, so the fallback fails too; both reasons are kept.
        s.apple_fallback_provider = "LLM".into();
        s.llm_endpoint = "http://127.0.0.1:9/v1/chat/completions".into();
        s.llm_model = "test-model".into();
        let err = route.translate(&s, "Привіт, як справи?", "ukrainian").await.map(|t| t.text).unwrap_err();
        assert!(err.starts_with("Apple Translation can't translate into Klingon language; LLM failed too"), "{err}");
    }

    #[tokio::test]
    async fn apple_translates_with_the_detected_language_when_the_source_is_auto() {
        if !crate::apple_translate::is_available()
            || crate::apple_translate::status("uk", "en").await.ok().as_deref() != Some("installed")
        {
            return;
        }
        let mut s = apple_settings();
        s.source_lang = "Auto".into();
        let done = TranslationRoute::from_settings(&s).translate(&s, "Привіт, як справи?", "ukrainian").await.unwrap();
        assert!(done.text.to_lowercase().contains("hello"), "{}", done.text);
        assert_eq!(done.label, "Apple");
        assert_eq!(done.pair.as_deref(), Some("uk → en"));
        assert_eq!(done.via, "Apple Translation (on-device) uk → en");
    }

    #[test]
    fn cloud_translation_needs_a_key_and_model() {
        let mut s = AppSettings::default();
        s.translation_provider = "LLM".into();
        s.llm_endpoint = "https://llm.example.com/v1/chat/completions".into();
        s.llm_api_key = String::new();
        s.api_key = String::new();
        s.local_llm_api_key = String::new();
        s.llm_model = "llama3.2".into();
        assert_eq!(missing_translation_settings(&s), vec!["API key"]);
        s.llm_model = " ".into();
        assert_eq!(missing_translation_settings(&s), vec!["API key", "model name"]);
        s.llm_api_key = "key".into();
        s.llm_model = "llama3.2".into();
        assert!(missing_translation_settings(&s).is_empty());
    }

    #[test]
    fn local_translation_needs_no_key() {
        let mut s = AppSettings::default();
        s.translation_provider = "LLM".into();
        s.llm_endpoint = "http://localhost:11434/v1/chat/completions".into();
        s.llm_api_key = String::new();
        s.api_key = String::new();
        s.local_llm_api_key = String::new();
        s.llm_model = "llama3.2".into();
        assert!(missing_translation_settings(&s).is_empty());
        s.llm_endpoint = String::new();
        assert_eq!(missing_translation_settings(&s), vec!["endpoint URL", "API key"]);
    }

    #[test]
    fn custom_api_needs_url_and_key() {
        let mut s = AppSettings::default();
        s.translation_provider = "Custom API".into();
        s.custom_api_url = String::new();
        s.custom_api_key = String::new();
        s.custom_api_model = String::new();
        assert_eq!(missing_translation_settings(&s), vec!["endpoint URL", "API key", "model name"]);
        s.custom_api_url = "https://api.example.com/v1/chat/completions".into();
        s.custom_api_key = "sk".into();
        assert_eq!(missing_translation_settings(&s), vec!["model name"]);
        s.custom_api_model = "my-model".into();
        assert!(missing_translation_settings(&s).is_empty());
    }

    #[test]
    fn missing_fields_read_as_a_list() {
        assert_eq!(list_missing(&["API key"]), "the API key");
        assert_eq!(list_missing(&["API key", "model name"]), "the API key and model name");
        assert_eq!(list_missing(&["endpoint URL", "API key", "model name"]), "the endpoint URL, API key and model name");
    }

    #[test]
    fn capitalize_first_letter_only() {
        assert_eq!(capitalize("timed out after 20 s"), "Timed out after 20 s");
        assert_eq!(capitalize(""), "");
    }
    use crate::audio::{MicProblem, MicReport};

    fn report(problem: MicProblem, missing: Option<&str>) -> MicReport {
        MicReport { problem, device: "MacBook Pro Microphone".into(), missing_device: missing.map(Into::into), reconnecting: false }
    }

    #[test]
    fn mic_messages_name_the_device_and_the_setting() {
        let (title, text) = mic_problem_message(&report(MicProblem::NoAudio, None));
        assert_eq!(title, "Microphone Not Working");
        assert!(text.contains("\"MacBook Pro Microphone\""), "{text}");
        assert!(text.contains("Settings → General → Microphone input"), "{text}");

        let (title, text) = mic_problem_message(&report(MicProblem::Silent, Some("Jabra Evolve")));
        assert_eq!(title, "Microphone Not Found");
        assert!(text.contains("\"Jabra Evolve\" isn't connected"), "{text}");
        assert!(text.contains("default \"MacBook Pro Microphone\" sends no audio"), "{text}");
    }

    #[test]
    fn digital_silence_only_for_blocked_input() {
        assert!(is_digital_silence(&[0.0; 16000]));
        assert!(is_digital_silence(&[]));
        // A quiet but live microphone still has a noise floor.
        let noise: Vec<f32> = (0..16000).map(|i| if i % 2 == 0 { 0.002 } else { -0.002 }).collect();
        assert!(!is_digital_silence(&noise));
    }
}
