pub mod app_controller;
pub mod audio;
pub mod chunking;
pub mod clipboard_snapshot;
pub mod cli;
pub mod cpu_features;
pub mod custom_models;
pub mod global_key_listener;
pub mod history;
pub mod linux_paste;
pub mod memory;
pub mod paste;
pub mod secure_input;
pub mod perf;
pub mod pipeline_logger;
pub mod settings;
pub mod text_normalizer;
pub mod post_processor;
pub mod masker;
pub mod transcribe;
pub mod translate;
pub mod apple_translate;
pub mod offline_translate;
pub mod tray;
pub mod updater;
pub mod vad;
pub mod parakeet;
pub mod lab;
pub mod live_chunks;
pub mod denoise;
pub mod fullscreen_pill;
pub mod pill_position;
pub mod leveler;
pub mod model_download;
pub mod sound;
pub mod vitals;

use std::sync::Arc;
use tauri::{AppHandle, Emitter, Listener, Manager, State};
use tauri_plugin_global_shortcut::{GlobalShortcutExt, Shortcut, ShortcutState};

use app_controller::{AppController, RecordingMode};
use history::HistoryRecord;
use settings::AppSettings;

pub struct AppState {
    pub controller: Arc<AppController>,
    pub key_listener: Arc<global_key_listener::GlobalKeyListenerHandle>,
}

#[tauri::command]
fn get_settings(state: State<'_, AppState>) -> AppSettings {
    state.controller.get_settings()
}

#[tauri::command]
fn check_accessibility() -> bool {
    global_key_listener::is_accessibility_trusted()
}

#[tauri::command]
fn request_accessibility() {
    global_key_listener::request_accessibility_prompt();
}

#[tauri::command]
fn open_accessibility_settings() {
    #[cfg(target_os = "macos")]
    {
        let _ = std::process::Command::new("open")
            .arg("x-apple.systempreferences:com.apple.preference.security?Privacy_Accessibility")
            .spawn();
    }
}

#[tauri::command]
fn save_settings(
    state: State<'_, AppState>,
    app_handle: AppHandle,
    mut new_settings: AppSettings,
) -> Result<(), String> {
    new_settings.migrate();
    let old_settings = state.controller.get_settings();
    // A Settings window opened before the welcome guide finished still holds `false`.
    new_settings.onboarding_done |= old_settings.onboarding_done;
    state.controller.save_settings_from_window(new_settings.clone())?;

    // Update global hotkeys if changed (the translate hotkey is only active in its mode)
    if old_settings.hotkey != new_settings.hotkey
        || old_settings.active_translate_hotkey() != new_settings.active_translate_hotkey()
    {
        apply_hotkeys(&app_handle, &state, &new_settings);
    }

    Ok(())
}

/// Points the modifier-key listener and the key-combo shortcuts at the settings' hotkeys.
pub(crate) fn apply_hotkeys(app_handle: &AppHandle, state: &AppState, settings: &AppSettings) {
    state.key_listener.update_hotkeys(&settings.hotkey, settings.active_translate_hotkey());
    register_global_shortcuts(app_handle, settings);
    prompt_accessibility_for_modifier_hotkeys(settings);
}

/// (Re)registers the key-combo hotkeys (e.g. `Control+Shift+Space`) with the global-shortcut
/// plugin. Modifier-only hotkeys (e.g. `RightOption`, `Alt+Shift`) don't parse as shortcuts; on
/// macOS the event tap in global_key_listener.rs handles those.
pub(crate) fn register_global_shortcuts(app_handle: &AppHandle, settings: &AppSettings) {
    let gs = app_handle.global_shortcut();
    let _ = gs.unregister_all();
    let combo = |hotkey: &str| {
        if global_key_listener::modifier_hotkey_id(hotkey).is_some() {
            return None;
        }
        hotkey.parse::<Shortcut>().ok()
    };
    let main = combo(&settings.hotkey);
    if let Some(shortcut) = main {
        if let Err(e) = gs.register(shortcut) {
            log::warn!("Could not register hotkey {}: {}", settings.hotkey, e);
        }
    }
    if let Some(shortcut) = combo(settings.active_translate_hotkey()) {
        if Some(shortcut) == main {
            log::warn!("Translate hotkey is the same as the main hotkey; ignoring it");
        } else if let Err(e) = gs.register(shortcut) {
            log::warn!("Could not register translate hotkey {}: {}", settings.active_translate_hotkey(), e);
        }
    }
}

/// Which recording a pressed key-combo shortcut starts: the translate one if it matches
/// the active translate hotkey, otherwise the main one.
fn shortcut_mode(shortcut: &Shortcut, settings: &AppSettings) -> RecordingMode {
    let is_translate = settings.active_translate_hotkey().parse::<Shortcut>().map(|t| &t == shortcut).unwrap_or(false);
    let is_main = settings.hotkey.parse::<Shortcut>().map(|m| &m == shortcut).unwrap_or(false);
    if is_translate && !is_main {
        RecordingMode::Translate
    } else {
        RecordingMode::Main
    }
}

fn prompt_accessibility_for_modifier_hotkeys(settings: &AppSettings) {
    let uses_modifier = [settings.hotkey.as_str(), settings.active_translate_hotkey()]
        .iter()
        .any(|h| global_key_listener::modifier_hotkey_id(h).is_some());
    if uses_modifier && !global_key_listener::is_accessibility_trusted() {
        global_key_listener::request_accessibility_prompt();
    }
}

#[tauri::command]
fn toggle_recording(state: State<'_, AppState>) -> Result<(), String> {
    state.controller.toggle_recording("app")
}

#[tauri::command]
fn cancel_recording(state: State<'_, AppState>) -> Result<(), String> {
    state.controller.cancel("Esc in the pill");
    Ok(())
}

#[tauri::command]
fn retry_translation(state: State<'_, AppState>) -> Result<(), String> {
    state.controller.retry_translation()
}

/// A microphone picked in the pill when the chosen one isn't connected.
#[tauri::command]
fn choose_microphone(state: State<'_, AppState>, name: String) -> Result<(), String> {
    state.controller.choose_microphone(&name)
}

#[tauri::command]
fn get_history(
    state: State<'_, AppState>,
    limit: Option<usize>,
) -> Result<Vec<HistoryRecord>, String> {
    state.controller.get_history(limit.unwrap_or(50))
}

#[tauri::command]
fn get_history_audio_bytes(state: State<'_, AppState>) -> u64 {
    state.controller.history_audio_bytes()
}

#[tauri::command]
fn clear_history(state: State<'_, AppState>) -> Result<(), String> {
    state.controller.clear_history()
}

/// Async and on a blocking thread: on Windows listing devices opens each one and can take seconds,
/// which froze the UI thread (and any open file dialog). The device monitor in audio.rs keeps the
/// tray menu up to date, so this only reads the lists.
#[tauri::command]
async fn get_audio_devices() -> Result<serde_json::Value, String> {
    tauri::async_runtime::spawn_blocking(|| {
        let (input_devices, output_devices) = audio::list_devices();
        serde_json::json!({ "input_devices": input_devices, "output_devices": output_devices })
    })
    .await
    .map_err(|e| e.to_string())
}

/// Closes the welcome guide; closing it marks it done (see `tray::open_onboarding`).
#[tauri::command]
fn finish_onboarding(app_handle: AppHandle) {
    mark_onboarding_done(&app_handle);
    if let Some(win) = app_handle.get_webview_window("onboarding") {
        let _ = win.close();
    }
}

#[tauri::command]
fn open_whats_new_window(app_handle: AppHandle) {
    tray::open_whats_new(&app_handle);
}

/// Closes What's New; closing it marks it seen (see `tray::open_whats_new`).
#[tauri::command]
fn dismiss_whats_new(app_handle: AppHandle) {
    close_whats_new(&app_handle);
}

/// What's New's "Open Settings": on Text, where the new text styles are.
#[tauri::command]
fn open_settings_from_whats_new(app_handle: AppHandle) {
    close_whats_new(&app_handle);
    tray::open_preferences_tab(&app_handle, "text");
}

fn close_whats_new(app_handle: &AppHandle) {
    if let Some(win) = app_handle.get_webview_window("whats-new") {
        let _ = win.close();
    }
}

/// The Settings tab to show when the window opens, if something asked for one.
#[tauri::command]
fn take_settings_tab() -> Option<String> {
    tray::take_settings_tab()
}

/// Routes the hotkeys to the welcome guide's practice run (see `app_controller::set_practice_mode`).
#[tauri::command]
fn set_practice_mode(on: bool) {
    app_controller::set_practice_mode(on);
}

/// Saves that the welcome guide was seen, so it doesn't open on the next launch.
pub(crate) fn mark_onboarding_done(app_handle: &AppHandle) {
    let Some(state) = app_handle.try_state::<AppState>() else {
        return;
    };
    let mut settings = state.controller.get_settings();
    if settings.onboarding_done {
        return;
    }
    settings.onboarding_done = true;
    settings.last_version = Some(env!("CARGO_PKG_VERSION").to_string());
    if let Err(e) = state.controller.save_settings(settings) {
        log::warn!("Could not save that the welcome guide was seen: {}", e);
    }
}

#[tauri::command]
fn start_mic_test(app_handle: AppHandle, device_name: Option<String>) -> Result<(), String> {
    audio::start_mic_test(app_handle, device_name)
}

#[tauri::command]
fn stop_mic_test() -> Result<(), String> {
    audio::stop_mic_test();
    Ok(())
}

#[tauri::command]
fn play_test_sound() -> Result<(), String> {
    sound::play_sound(sound::AppSound::StartRecording);
    Ok(())
}

/// Whether Bifaria opens when you log in. The system's login item is the source of truth, so it
/// isn't kept in the settings file.
#[tauri::command]
fn get_launch_at_login(app_handle: AppHandle) -> Result<bool, String> {
    use tauri_plugin_autostart::ManagerExt;
    app_handle.autolaunch().is_enabled().map_err(|e| e.to_string())
}

#[tauri::command]
fn set_launch_at_login(app_handle: AppHandle, enabled: bool) -> Result<(), String> {
    use tauri_plugin_autostart::ManagerExt;
    let autolaunch = app_handle.autolaunch();
    let result = if enabled { autolaunch.enable() } else { autolaunch.disable() };
    result.map_err(|e| e.to_string())
}

#[tauri::command]
fn fit_pill_height(app_handle: AppHandle, height: f64) {
    app_controller::fit_long_pill(&app_handle, height);
}

#[tauri::command]
fn close_preferences_window(app_handle: AppHandle) {
    audio::stop_mic_test();
    if let Some(win) = app_handle.get_webview_window("preferences") {
        let _ = win.hide();
    }
    tray::sync_dock_icon(&app_handle, None);
}

/// Drags the pill; where it ends up is saved when it hides (see `pill_position`).
#[tauri::command]
fn start_dragging_window(window: tauri::WebviewWindow) -> Result<(), String> {
    pill_position::mark_dragged();
    if fullscreen_pill::start_dragging() {
        return Ok(());
    }
    window.start_dragging().map_err(|e| e.to_string())
}

#[tauri::command]
fn get_transcription_logs(
    state: State<'_, AppState>,
    limit: Option<usize>,
) -> Vec<pipeline_logger::TranscriptionDiagnosticLog> {
    state.controller.get_diagnostic_logs(limit.unwrap_or(50))
}

#[tauri::command]
fn clear_transcription_logs(state: State<'_, AppState>) -> Result<(), String> {
    state.controller.clear_diagnostic_logs();
    Ok(())
}

#[tauri::command]
fn get_available_models() -> Vec<transcribe::ModelCatalogItem> {
    transcribe::get_model_catalog()
}

#[tauri::command]
async fn download_specific_model(
    app_handle: AppHandle,
    model_name: String,
) -> Result<String, String> {
    if crate::parakeet::is_parakeet(&model_name) {
        let path = parakeet::ParakeetTranscriber::ensure_model(&app_handle, &model_name).await?;
        Ok(path.to_string_lossy().to_string())
    } else {
        let path = transcribe::Transcriber::ensure_model(&app_handle, &model_name).await?;
        Ok(path.to_string_lossy().to_string())
    }
}

#[tauri::command]
async fn get_audio_data(
    state: State<'_, AppState>,
    filename: Option<String>,
) -> Result<String, String> {
    let controller = state.controller.clone();
    tauri::async_runtime::spawn_blocking(move || controller.get_audio_data(filename))
        .await
        .map_err(|e| e.to_string())?
}

#[tauri::command]
fn open_audio_folder(state: State<'_, AppState>) -> Result<(), String> {
    state.controller.open_audio_folder()
}

#[tauri::command]
fn list_lab_audio_files(state: State<'_, AppState>) -> Vec<lab::LabAudioItem> {
    state.controller.list_lab_audio()
}

#[tauri::command]
async fn run_lab_experiment(
    state: State<'_, AppState>,
    req: lab::LabExperimentRequest,
) -> Result<lab::LabExperimentResult, String> {
    state.controller.run_lab_experiment(req).await
}

#[tauri::command]
async fn lab_translate(
    state: State<'_, AppState>,
    text: String,
    provider: String,
    target_lang: String,
) -> Result<app_controller::LabTranslation, String> {
    state.controller.lab_translate(text, provider, target_lang).await
}

/// Whether the Apple translation provider can be used on this Mac, and why not.
#[derive(serde::Serialize)]
struct AppleTranslationInfo {
    available: bool,
    reason: Option<String>,
    /// e.g. "26.1"; None off macOS.
    os_version: Option<String>,
    min_os: u32,
    /// Why the Apple Intelligence provider can't be used; None when it can.
    intelligence_reason: Option<String>,
}

#[tauri::command]
async fn apple_translation_info() -> AppleTranslationInfo {
    let reason = apple_translate::unavailable_reason();
    AppleTranslationInfo {
        available: reason.is_none(),
        reason,
        os_version: apple_translate::macos_version().map(|(major, minor)| format!("{major}.{minor}")),
        min_os: apple_translate::MIN_MACOS,
        intelligence_reason: apple_translate::intelligence_unavailable_reason().await,
    }
}

/// Apple's language packs for translating into `target_lang` (a Bifaria language name), and the
/// target's code.
#[derive(serde::Serialize)]
struct AppleLanguagePacks {
    target: String,
    languages: Vec<apple_translate::LanguagePack>,
}

#[tauri::command]
async fn apple_translation_languages(target_lang: String) -> Result<AppleLanguagePacks, String> {
    let target = apple_translate::to_bcp47(&target_lang)
        .ok_or_else(|| format!("Apple Translation can't translate into {}", target_lang))?;
    let languages = apple_translate::languages(&target)
        .await
        .map_err(|e| e.describe(&format!("translating into {}", target_lang)))?;
    Ok(AppleLanguagePacks { target, languages })
}

/// Shows Apple's download prompt for `source` → `target` (codes) and waits for it to finish.
#[tauri::command]
async fn apple_translation_download(state: State<'_, AppState>, source: String, target: String) -> Result<(), String> {
    let pair = apple_translate::pair_name(Some(&source), &target);
    let model = apple_translate::model_name(&state.controller.get_settings().apple_translation_model);
    log::info!("Apple Translation: downloading {} ({} model)", pair, model);
    apple_translate::download(&source, &target, model).await.map_err(|e| e.describe(&pair))
}

/// The offline models' language packs for translating into `target_lang` (a Bifaria language name),
/// and the target's code; the same shape as Apple's.
#[tauri::command]
async fn offline_translation_languages(target_lang: String) -> Result<AppleLanguagePacks, String> {
    let target = apple_translate::to_bcp47(&target_lang)
        .ok_or_else(|| format!("Offline translation can't translate into {}", target_lang))?;
    let languages = offline_translate::languages(&target).await?;
    Ok(AppleLanguagePacks { target, languages })
}

/// Downloads the offline models for `source` → `target` (codes).
#[tauri::command]
async fn offline_translation_download(app: AppHandle, source: String, target: String) -> Result<(), String> {
    log::info!("Offline translation: downloading {}", apple_translate::pair_name(Some(&source), &target));
    // At most ten updates a second: the downloader reports every chunk.
    let last = std::sync::Mutex::new(std::time::Instant::now() - std::time::Duration::from_secs(1));
    let pair = format!("{source}>{target}");
    offline_translate::download(&source, &target, move |bytes| {
        let mut last = last.lock().unwrap();
        if last.elapsed() >= std::time::Duration::from_millis(100) {
            *last = std::time::Instant::now();
            let _ = app.emit("offline-pack-progress", serde_json::json!({ "pair": pair, "bytes": bytes }));
        }
    })
    .await
}

#[tauri::command]
fn apple_translation_open_settings() -> Result<(), String> {
    apple_translate::open_system_settings()
}

#[tauri::command]
fn lab_save_result(
    state: State<'_, AppState>,
    log_id: u64,
    kind: String,
    text: String,
    source: Option<String>,
) -> Result<(), String> {
    state.controller.lab_save_result(log_id, &kind, text, source)
}

#[tauri::command]
async fn save_lab_custom_audio(
    state: State<'_, AppState>,
    base64_wav: String,
    file_name: Option<String>,
) -> Result<String, String> {
    let controller = state.controller.clone();
    tauri::async_runtime::spawn_blocking(move || controller.save_lab_custom_audio(base64_wav, file_name))
        .await
        .map_err(|e| e.to_string())?
}

#[tauri::command]
fn get_hardware_profile() -> cpu_features::HardwareProfile {
    cpu_features::detect_hardware_profile()
}

#[derive(serde::Serialize)]
struct DeviceOption {
    value: &'static str,
    label: String,
}

/// Where a model can run on this computer, for Settings → Runs on, next to the model picker.
#[derive(serde::Serialize)]
struct ModelDevices {
    /// The setting the choice is saved in: each model family keeps its own.
    setting: &'static str,
    options: Vec<DeviceOption>,
    hint: String,
}

#[tauri::command]
async fn get_model_devices(model_name: String) -> ModelDevices {
    let parakeet = parakeet::is_parakeet(&model_name);
    let options = if parakeet { parakeet::devices(&model_name) } else { transcribe::devices() };
    let hint = match (parakeet, options.len()) {
        (true, 1) => "Parakeet runs on the CPU on this computer".to_string(),
        (true, _) => "Parakeet runs through Core ML. The Neural Engine is the fastest and easiest on the battery; \
            GPU and CPU are there to compare. The first load on a new device takes longer"
            .to_string(),
        (false, 1) => "No GPU found, Whisper runs on the CPU".to_string(),
        (false, _) => "Whisper is fastest on the GPU; pick CPU if the GPU misbehaves".to_string(),
    };
    ModelDevices {
        setting: if parakeet { "parakeet_device" } else { "device_backend" },
        options: options.into_iter().map(|(value, label)| DeviceOption { value, label }).collect(),
        hint,
    }
}

/// Runs a command from a second `bifaria` process (see cli.rs) in this one.
fn handle_cli(app: &AppHandle, args: &[String]) {
    let Some(state) = app.try_state::<AppState>() else {
        return;
    };
    let controller = Arc::clone(&state.controller);
    match cli::parse(args.iter().skip(1)) {
        Some(cli::CliCommand::Toggle) => {
            let _ = controller.toggle_recording_as("command line", RecordingMode::Main);
        }
        Some(cli::CliCommand::Translate) => {
            let _ = controller.toggle_recording_as("command line (translate)", RecordingMode::Translate);
        }
        Some(cli::CliCommand::Cancel) => controller.cancel("command line"),
        // Help is printed by the second process itself.
        Some(cli::CliCommand::Help) => {}
        // Launched again from the app menu: show Settings instead of starting a second copy.
        None => tray::open_preferences(app),
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    memory::init_allocator();
    if cli::parse(std::env::args().skip(1)) == Some(cli::CliCommand::Help) {
        println!("{}", cli::USAGE);
        return;
    }
    settings::import_from_revfly();

    tauri::Builder::default()
        // Must be the first plugin, so a second launch exits before anything else starts.
        .plugin(tauri_plugin_single_instance::init(|app, args, _cwd| handle_cli(app, &args)))
        .plugin(tauri_plugin_log::Builder::default().build())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_autostart::init(tauri_plugin_autostart::MacosLauncher::LaunchAgent, None))
        .plugin(
            tauri_plugin_global_shortcut::Builder::new()
                .with_handler(|app, shortcut, event| {
                    if event.state() == ShortcutState::Pressed {
                        if let Some(state) = app.try_state::<AppState>() {
                            let controller = Arc::clone(&state.controller);
                            // Registered only while Secure Input is on (see secure_input.rs).
                            if *shortcut == secure_input::escape_shortcut() {
                                controller.cancel("Esc (Secure Input)");
                                return;
                            }
                            let mode = shortcut_mode(shortcut, &controller.get_settings());
                            let trigger = match mode {
                                RecordingMode::Main => "keyboard shortcut",
                                RecordingMode::Translate => "translate keyboard shortcut",
                            };
                            tauri::async_runtime::spawn(async move {
                                let _ = controller.toggle_recording_as(trigger, mode);
                            });
                        }
                    }
                })
                .build(),
        )
        .setup(|app| {
            #[cfg(target_os = "macos")]
            app.set_activation_policy(tauri::ActivationPolicy::Accessory);

            sound::preload();
            perf::init();

            let hw = cpu_features::detect_hardware_profile();
            log::info!("Hardware profile detected: {}", hw.summary);
            pipeline_logger::log_stage_event(&settings::get_data_dir(), "HARDWARE_DETECT", &hw.summary);

            let controller = Arc::new(AppController::new(app.handle().clone())?);
            let settings = controller.get_settings();
            let hotkey_str = settings.hotkey.clone();

            // Create macOS menu bar tray icon
            let _ = tray::create_tray(&app.handle(), &settings);
            crate::tray::update_tray_model_status(&app.handle(), false);
            audio::start_device_monitor(app.handle().clone());
            updater::spawn_background_checks(app.handle());
            model_download::spawn_initial_download(app.handle(), settings.model_name.clone());

            // Configure window collection behavior; each show places the pill (see pill_position).
            if let Some(win) = app.get_webview_window("main") {
                let _ = win.set_always_on_top(true);
                let _ = win.set_shadow(false);
                app_controller::apply_pill_window_behavior(&win, false);
            }

            // Return the pill to its own window once it goes idle, if it was moved into the
            // full-screen panel (see fullscreen_pill.rs).
            {
                let app_handle = app.handle().clone();
                app.listen("assistant-state-changed", move |event| {
                    if let Ok(payload) = serde_json::from_str::<serde_json::Value>(event.payload()) {
                        let state = payload.get("state").and_then(|v| v.as_str()).unwrap_or("idle");
                        fullscreen_pill::on_state_changed(&app_handle, state);
                        // Errors and mic warnings are heard too, for when nobody is looking at the pill.
                        let warning = payload.get("warning").and_then(|v| v.as_bool()).unwrap_or(false);
                        let sounds_on = app_handle.try_state::<AppState>().is_some_and(|s| s.controller.get_settings().sound_error);
                        if (state == "error" || warning) && sounds_on {
                            sound::play_sound(sound::AppSound::Error);
                        }
                    }
                });
            }

            let key_listener = global_key_listener::start_global_key_listener(
                Arc::clone(&controller),
                &hotkey_str,
                settings.active_translate_hotkey(),
                app.handle().clone(),
            );

            // On first launch the welcome guide asks for Accessibility itself, with an explanation.
            if settings.onboarding_done {
                prompt_accessibility_for_modifier_hotkeys(&settings);
            }

            app.manage(AppState {
                controller,
                key_listener,
            });

            register_global_shortcuts(app.handle(), &settings);

            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            get_settings,
            save_settings,
            toggle_recording,
            cancel_recording,
            retry_translation,
            choose_microphone,
            check_accessibility,
            request_accessibility,
            open_accessibility_settings,
            get_history,
            get_history_audio_bytes,
            clear_history,
            finish_onboarding,
            open_whats_new_window,
            dismiss_whats_new,
            open_settings_from_whats_new,
            take_settings_tab,
            set_practice_mode,
            get_audio_devices,
            close_preferences_window,
            fit_pill_height,
            start_dragging_window,
            get_transcription_logs,
            clear_transcription_logs,
            get_available_models,
            download_specific_model,
            get_audio_data,
            open_audio_folder,
            list_lab_audio_files,
            run_lab_experiment,
            lab_translate,
            apple_translation_info,
            apple_translation_languages,
            apple_translation_download,
            apple_translation_open_settings,
            offline_translation_languages,
            offline_translation_download,
            lab_save_result,
            save_lab_custom_audio,
            get_hardware_profile,
            get_model_devices,
            start_mic_test,
            stop_mic_test,
            play_test_sound,
            custom_models::open_models_folder,
            custom_models::get_models_dir_path,
            custom_models::import_model_file,
            custom_models::pick_and_import_model,
            custom_models::download_custom_model,
            get_launch_at_login,
            set_launch_at_login,
            updater::get_update_status,
            updater::check_for_updates,
            updater::install_update,
        ])
        .build(tauri::generate_context!())
        .expect("error while running tauri application")
        .run(|app, event| match event {
            // Opened only once launching has finished: a window built during `setup` on macOS is
            // left off screen when the menu-bar-only policy is applied at launch.
            tauri::RunEvent::Ready => {
                let (done, announce) = app
                    .try_state::<AppState>()
                    .map(|state| {
                        let s = state.controller.get_settings();
                        (s.onboarding_done, s.should_show_whats_new())
                    })
                    .unwrap_or((true, false));
                if !done {
                    tray::open_onboarding(app);
                } else if announce {
                    tray::open_whats_new(app);
                }
            }
            #[cfg(target_os = "macos")]
            tauri::RunEvent::Reopen { .. } => tray::reopen_from_dock(app),
            _ => {}
        });
}
