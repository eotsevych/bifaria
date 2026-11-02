use tauri::{AppHandle, Emitter, Manager, WebviewUrl, WebviewWindowBuilder};
use tauri::menu::{CheckMenuItem, Menu, MenuItem, PredefinedMenuItem, Submenu};
use tauri::tray::{TrayIcon, TrayIconBuilder};

use crate::audio::{list_input_devices, list_output_devices};
use crate::settings::AppSettings;

/// Windows that make Bifaria behave like a regular app while they're open.
const DOCK_WINDOWS: [&str; 3] = ["preferences", "onboarding", "whats-new"];

/// Bifaria lives in the menu bar, but while Settings or the welcome guide is open it shows in the Dock
/// and Cmd+Tab like a normal app, so those windows are easy to get back to. `closing` names a window
/// that is going away but may still be registered.
pub fn sync_dock_icon(app: &AppHandle, closing: Option<&str>) {
    #[cfg(target_os = "macos")]
    {
        let open = DOCK_WINDOWS.iter().filter(|label| Some(**label) != closing).any(|label| {
            app.get_webview_window(label).is_some_and(|w| {
                w.is_visible().unwrap_or(false) || w.is_minimized().unwrap_or(false)
            })
        });
        let policy = if open {
            tauri::ActivationPolicy::Regular
        } else {
            tauri::ActivationPolicy::Accessory
        };
        let _ = app.set_activation_policy(policy);
    }
    #[cfg(not(target_os = "macos"))]
    let _ = (app, closing);
}

/// Brings back Settings or the welcome guide when the Dock icon is clicked.
pub fn reopen_from_dock(app: &AppHandle) {
    if app.get_webview_window("whats-new").is_some() {
        open_whats_new(app);
    } else if app.get_webview_window("onboarding").is_some() {
        open_onboarding(app);
    } else if app.get_webview_window("preferences").is_some() {
        open_preferences(app);
    }
}

/// The tab Settings should show once its window is up; it takes this with `take_settings_tab`.
static PENDING_SETTINGS_TAB: std::sync::Mutex<Option<String>> = std::sync::Mutex::new(None);

/// Opens Settings on `tab` (a tab id such as "translations"). A window that is already open
/// switches on the `open-settings-tab` event; a new one asks for the pending tab when it loads.
pub fn open_preferences_tab(app: &AppHandle, tab: &str) {
    *PENDING_SETTINGS_TAB.lock().unwrap() = Some(tab.to_string());
    open_preferences(app);
    let _ = app.emit_to("preferences", "open-settings-tab", tab);
}

pub fn take_settings_tab() -> Option<String> {
    PENDING_SETTINGS_TAB.lock().unwrap().take()
}

pub fn open_preferences(app: &AppHandle) {
    if let Some(win) = app.get_webview_window("preferences") {
        let _ = win.unminimize();
        let _ = win.show();
        sync_dock_icon(app, None);
        let _ = win.set_focus();
    } else {
        let mut builder = WebviewWindowBuilder::new(
            app,
            "preferences",
            WebviewUrl::App("index.html#preferences".into()),
        )
        .title("Settings")
        .inner_size(880.0, 580.0)
        .min_inner_size(680.0, 480.0)
        .resizable(true)
        .devtools(true)
        .center();

        #[cfg(target_os = "macos")]
        {
            builder = builder.title_bar_style(tauri::TitleBarStyle::Overlay).hidden_title(true);
        }

        if let Ok(win) = builder.build() {
            sync_dock_icon(app, None);
            let _ = win.set_focus();
            let win_clone = win.clone();
            win.on_window_event(move |event| {
                if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                    api.prevent_close();
                    let _ = win_clone.hide();
                    crate::audio::stop_mic_test();
                    sync_dock_icon(win_clone.app_handle(), None);
                }
            });
        }
    }
}

/// Opens the first-run welcome guide, or brings it forward if it's already open. Closing it counts
/// as finishing it, so it doesn't come back on the next launch.
pub fn open_onboarding(app: &AppHandle) {
    if let Some(win) = app.get_webview_window("onboarding") {
        let _ = win.unminimize();
        let _ = win.show();
        sync_dock_icon(app, None);
        let _ = win.set_focus();
        return;
    }
    // A borderless rounded card (the page draws it), so the window itself is transparent.
    let builder = WebviewWindowBuilder::new(app, "onboarding", WebviewUrl::App("index.html#onboarding".into()))
        .title("Welcome to Bifaria")
        .inner_size(780.0, 680.0)
        .resizable(false)
        .maximizable(false)
        .decorations(false)
        .transparent(true)
        .shadow(true)
        .center()
        .focused(true);

    match builder.build() {
        Ok(win) => {
            sync_dock_icon(app, None);
            let _ = win.set_focus();
            let app = app.clone();
            win.on_window_event(move |event| {
                if let tauri::WindowEvent::Destroyed = event {
                    crate::audio::stop_mic_test();
                    crate::app_controller::set_practice_mode(false);
                    crate::mark_onboarding_done(&app);
                    sync_dock_icon(&app, Some("onboarding"));
                }
            });
        }
        Err(e) => log::warn!("Could not open the welcome guide: {}", e),
    }
}

/// Opens the "What's New in Bifaria" announcement modal, or brings it forward if it's already open.
pub fn open_whats_new(app: &AppHandle) {
    if let Some(win) = app.get_webview_window("whats-new") {
        let _ = win.unminimize();
        let _ = win.show();
        sync_dock_icon(app, None);
        let _ = win.set_focus();
        return;
    }
    let builder = WebviewWindowBuilder::new(app, "whats-new", WebviewUrl::App("index.html#whats-new".into()))
        .title("What's New in Bifaria")
        .inner_size(540.0, 620.0)
        .resizable(false)
        .maximizable(false)
        .decorations(false)
        .transparent(true)
        .shadow(true)
        .center()
        .focused(true);

    match builder.build() {
        Ok(win) => {
            sync_dock_icon(app, None);
            let _ = win.set_focus();
            let app = app.clone();
            win.on_window_event(move |event| {
                if let tauri::WindowEvent::Destroyed = event {
                    if let Some(state) = app.try_state::<crate::AppState>() {
                        let mut s = state.controller.get_settings();
                        if s.last_version.as_deref() != Some(env!("CARGO_PKG_VERSION")) {
                            s.last_version = Some(env!("CARGO_PKG_VERSION").to_string());
                            let _ = state.controller.save_settings(s);
                        }
                    }
                    sync_dock_icon(&app, Some("whats-new"));
                }
            });
        }
        Err(e) => log::warn!("Could not open what's new window: {}", e),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrayIconState {
    Ejected,
    Loaded,
    Recording,
}

/// Sleeping-state tray icon. macOS gets a white template image that the system tints to match the
/// menu bar; Windows and Linux draw icons as-is, so a white glyph vanishes on light taskbars and they
/// get the full-color app icon instead.
#[cfg(target_os = "macos")]
const SLEEPING_ICON: &[u8] = include_bytes!("../icons/tray-ejected@2x.png");
#[cfg(not(target_os = "macos"))]
const SLEEPING_ICON: &[u8] = include_bytes!("../icons/32x32.png");
const SLEEPING_ICON_IS_TEMPLATE: bool = cfg!(target_os = "macos");

pub fn update_tray_icon(app: &AppHandle, state: TrayIconState) {
    if let Some(tray) = app.tray_by_id("main-tray") {
        let (icon_bytes, is_template, title): (&[u8], bool, Option<&str>) = match state {
            TrayIconState::Recording => (include_bytes!("../icons/tray-recording@2x.png"), false, Some(" REC")),
            TrayIconState::Loaded => (include_bytes!("../icons/tray-loaded@2x.png"), false, Some("")),
            TrayIconState::Ejected => (SLEEPING_ICON, SLEEPING_ICON_IS_TEMPLATE, Some("")),
        };
        if let Ok(icon) = tauri::image::Image::from_bytes(icon_bytes) {
            let _ = tray.set_icon(Some(icon));
            let _ = tray.set_icon_as_template(is_template);
        }
        let tip = match state {
            TrayIconState::Recording => "Bifaria — Recording in progress…",
            TrayIconState::Loaded => "Bifaria — Speech Model Active in RAM",
            TrayIconState::Ejected => "Bifaria — Sleeping (RAM Saved)",
        };
        let _ = tray.set_tooltip(Some(tip));
        #[cfg(target_os = "macos")]
        let _ = tray.set_title(title);
    }
}

static STATUS_ITEM: std::sync::OnceLock<tauri::menu::MenuItem<tauri::Wry>> = std::sync::OnceLock::new();
static EJECT_ITEM: std::sync::OnceLock<tauri::menu::MenuItem<tauri::Wry>> = std::sync::OnceLock::new();
static TOGGLE_ITEM: std::sync::OnceLock<tauri::menu::MenuItem<tauri::Wry>> = std::sync::OnceLock::new();
static UPDATE_ITEM: std::sync::OnceLock<tauri::menu::MenuItem<tauri::Wry>> = std::sync::OnceLock::new();
static RETRY_ITEM: std::sync::OnceLock<tauri::menu::MenuItem<tauri::Wry>> = std::sync::OnceLock::new();
/// The tray menu, so "Retry Failed Translation" can be added and removed.
static TRAY_MENU: std::sync::OnceLock<Menu<tauri::Wry>> = std::sync::OnceLock::new();
/// Whether "Retry Failed Translation" is in the menu right now.
static RETRY_SHOWN: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
/// Menu position of "Retry Failed Translation": right after "Start Recording".
const RETRY_POSITION: usize = 4;
/// Translation submenu items with the setting value each stands for ("mode:off", "provider:LLM").
static TRANSLATION_ITEMS: std::sync::OnceLock<Vec<(String, CheckMenuItem<tauri::Wry>)>> = std::sync::OnceLock::new();
static INPUT_SUBMENU: std::sync::OnceLock<Submenu<tauri::Wry>> = std::sync::OnceLock::new();
static OUTPUT_SUBMENU: std::sync::OnceLock<Submenu<tauri::Wry>> = std::sync::OnceLock::new();
static IS_RECORDING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

pub fn update_tray_model_status(app: &AppHandle, is_loaded: bool) {
    // If user is actively recording, preserve the Recording icon & REC title
    if !IS_RECORDING.load(std::sync::atomic::Ordering::SeqCst) {
        update_tray_icon(app, if is_loaded { TrayIconState::Loaded } else { TrayIconState::Ejected });
    }
    if let Some(item) = STATUS_ITEM.get() {
        let label = if is_loaded {
            "● Speech Model: Active (In RAM) — tap to eject"
        } else {
            "○ Speech Model: Sleeping (RAM Saved)"
        };
        let _ = item.set_text(label);
        let _ = item.set_enabled(is_loaded);
    }
    if let Some(item) = EJECT_ITEM.get() {
        let _ = item.set_enabled(is_loaded && !IS_RECORDING.load(std::sync::atomic::Ordering::SeqCst));
        let _ = item.set_text(if is_loaded { "Eject Model from RAM" } else { "Model Ejected (Sleeping)" });
    }
    let _ = app; // silence unused if no tray yet
}

pub fn set_model_download_progress(percent: u32) {
    if let Some(item) = STATUS_ITEM.get() {
        let _ = item.set_text(format!("↓ Downloading Speech Model… {}%", percent));
        let _ = item.set_enabled(false);
    }
}

const TRANSLATION_MODES: [(&str, &str); 3] = [
    ("off", "Off (Voice to Text only)"),
    ("hotkey", "With the Translate Hotkey"),
    ("auto", "Automatic (Launch Hotkey)"),
];
/// Apple's providers are listed only where they can run (macOS 26+); the others everywhere.
const TRANSLATION_PROVIDERS: [(&str, &str); 4] = [
    ("LLM", "LLM (local, or your own server)"),
    (crate::offline_translate::PROVIDER, "Offline (on-device)"),
    (crate::apple_translate::PROVIDER, "Apple Native"),
    (crate::apple_translate::INTELLIGENCE_PROVIDER, "Apple Intelligence"),
];

fn provider_key(settings: &AppSettings) -> &'static str {
    if crate::apple_translate::is_apple(&settings.translation_provider) {
        crate::apple_translate::PROVIDER
    } else if crate::apple_translate::is_apple_intelligence(&settings.translation_provider) {
        crate::apple_translate::INTELLIGENCE_PROVIDER
    } else if crate::offline_translate::is_offline(&settings.translation_provider) {
        crate::offline_translate::PROVIDER
    } else if settings.translation_provider.to_lowercase().contains("custom") {
        // HIDDEN: Custom API has no tray entry any more; a saved one shows no tick.
        "Custom API"
    } else {
        "LLM"
    }
}

/// Ticks the translation mode and provider the settings use. Called whenever settings are saved,
/// so the menu matches changes made in the Settings window too.
pub fn sync_translation_menu(settings: &AppSettings) {
    let Some(items) = TRANSLATION_ITEMS.get() else {
        return;
    };
    let mode = format!("mode:{}", settings.translation_mode);
    let provider = format!("provider:{}", provider_key(settings));
    for (key, item) in items {
        let _ = item.set_checked(*key == mode || *key == provider);
        if key.starts_with("provider:") {
            let _ = item.set_enabled(!settings.translation_off());
        }
    }
}

/// Longest start of the failed text shown in the retry item, in characters.
const RETRY_SNIPPET_CHARS: usize = 30;

/// Label of the retry item, naming what will be retried:
/// `Retry Failed Translation (Ukrainian → English): "Привіт, як справи…"`.
pub fn retry_label(from_lang: &str, to_lang: &str, text: &str) -> String {
    let from = crate::translate::format_lang_name(from_lang);
    let to = crate::translate::format_lang_name(to_lang);
    let text = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if text.is_empty() {
        return format!("Retry Failed Translation ({} → {})", from, to);
    }
    let mut snippet: String = text.chars().take(RETRY_SNIPPET_CHARS).collect();
    if text.chars().count() > RETRY_SNIPPET_CHARS {
        snippet = format!("{}…", snippet.trim_end());
    }
    format!("Retry Failed Translation ({} → {}): \"{}\"", from, to, snippet)
}

/// Shows the retry item with `label` while a failed translation is waiting to be retried;
/// `None` removes it.
pub fn set_retry_item(label: Option<&str>) {
    let (Some(item), Some(menu)) = (RETRY_ITEM.get(), TRAY_MENU.get()) else {
        return;
    };
    if let Some(label) = label {
        let _ = item.set_text(label);
    }
    let shown = label.is_some();
    if RETRY_SHOWN.swap(shown, std::sync::atomic::Ordering::SeqCst) == shown {
        return;
    }
    let _ = if shown { menu.insert(item, RETRY_POSITION) } else { menu.remove(item) };
}

pub fn is_recording() -> bool {
    IS_RECORDING.load(std::sync::atomic::Ordering::SeqCst)
}

pub fn set_update_item(_app: &AppHandle, label: &str, enabled: bool) {
    if let Some(item) = UPDATE_ITEM.get() {
        let _ = item.set_text(label);
        let _ = item.set_enabled(enabled);
    }
}

pub fn set_tray_recording(app: &AppHandle, is_recording: bool, llm_loaded: bool) {
    IS_RECORDING.store(is_recording, std::sync::atomic::Ordering::SeqCst);
    if is_recording {
        update_tray_icon(app, TrayIconState::Recording);
        if let Some(item) = TOGGLE_ITEM.get() {
            let _ = item.set_text("Stop Recording");
        }
        // While recording, keep menu items reflective but not ejectable mid-capture
        if let Some(item) = EJECT_ITEM.get() {
            let _ = item.set_enabled(false);
        }
    } else {
        if let Some(item) = TOGGLE_ITEM.get() {
            let _ = item.set_text("Start Recording");
        }
        update_tray_model_status(app, llm_loaded);
    }
}

pub fn update_tray_audio_devices(app: &AppHandle, settings: &AppSettings) {
    let input_devices = list_input_devices();
    let output_devices = list_output_devices();
    update_tray_audio_devices_with_lists(app, settings, &input_devices, &output_devices);
}

pub fn update_tray_audio_devices_with_lists(
    app: &AppHandle,
    settings: &AppSettings,
    input_devices: &[String],
    output_devices: &[String],
) {
    if let Some(submenu) = INPUT_SUBMENU.get() {
        while let Ok(Some(_)) = submenu.remove_at(0) {}

        let current_input = settings.input_device.as_deref().unwrap_or("Default");
        let is_default = current_input == "Default" || current_input.is_empty();

        if let Ok(def_item) = CheckMenuItem::with_id(
            app,
            "input_dev:Default",
            "System Default",
            true,
            is_default,
            None::<&str>,
        ) {
            let _ = submenu.append(&def_item);
        }

        if !is_default && !input_devices.iter().any(|d| d == current_input) {
            if let Ok(item) = CheckMenuItem::with_id(
                app,
                format!("input_dev:{}", current_input),
                format!("{} (not connected)", current_input),
                true,
                true,
                None::<&str>,
            ) {
                let _ = submenu.append(&item);
            }
        }

        for dev in input_devices {
            let is_checked = current_input == dev;
            if let Ok(item) = CheckMenuItem::with_id(
                app,
                format!("input_dev:{}", dev),
                dev,
                true,
                is_checked,
                None::<&str>,
            ) {
                let _ = submenu.append(&item);
            }
        }
    }

    if let Some(submenu) = OUTPUT_SUBMENU.get() {
        while let Ok(Some(_)) = submenu.remove_at(0) {}

        let current_output = settings.output_device.as_deref().unwrap_or("Default");
        let is_default = current_output == "Default" || current_output.is_empty();

        if let Ok(def_item) = CheckMenuItem::with_id(
            app,
            "output_dev:Default",
            "System Default",
            true,
            is_default,
            None::<&str>,
        ) {
            let _ = submenu.append(&def_item);
        }

        if !is_default && !output_devices.iter().any(|d| d == current_output) {
            if let Ok(item) = CheckMenuItem::with_id(
                app,
                format!("output_dev:{}", current_output),
                format!("{} (not connected)", current_output),
                true,
                true,
                None::<&str>,
            ) {
                let _ = submenu.append(&item);
            }
        }

        for dev in output_devices {
            let is_checked = current_output == dev;
            if let Ok(item) = CheckMenuItem::with_id(
                app,
                format!("output_dev:{}", dev),
                dev,
                true,
                is_checked,
                None::<&str>,
            ) {
                let _ = submenu.append(&item);
            }
        }
    }
}

pub fn create_tray(app: &AppHandle, settings: &AppSettings) -> Result<TrayIcon, tauri::Error> {
    // Cheap RAM check at tray creation: any model loaded in this process? Check via controller if available later
    // Initial label: assume ejected; will be corrected immediately by update_tray_model_status after startup
    let status_item = MenuItem::with_id(
        app,
        "model_status",
        "○ Speech Model: Sleeping (RAM Saved)",
        false,
        None::<&str>,
    )?;
    let eject_item = MenuItem::with_id(
        app,
        "eject_model",
        "Eject Model from RAM",
        false,
        None::<&str>,
    )?;
    let _ = STATUS_ITEM.set(status_item.clone());
    let _ = EJECT_ITEM.set(eject_item.clone());
    let sep0 = PredefinedMenuItem::separator(app)?;

    let toggle_item = MenuItem::with_id(app, "toggle_recording", "Start Recording", true, None::<&str>)?;
    let _ = TOGGLE_ITEM.set(toggle_item.clone());
    // In the menu only while the last translation failed and can be sent again (see set_retry_item).
    let retry_item = MenuItem::with_id(app, "retry_translation", "Retry Failed Translation", true, None::<&str>)?;
    let _ = RETRY_ITEM.set(retry_item.clone());
    let sep1 = PredefinedMenuItem::separator(app)?;

    // Microphone Input submenu
    let input_submenu = Submenu::new(app, "Microphone Input", true)?;
    let _ = INPUT_SUBMENU.set(input_submenu.clone());

    // Audio Output submenu
    let output_submenu = Submenu::new(app, "Audio Output", true)?;
    let _ = OUTPUT_SUBMENU.set(output_submenu.clone());

    let input_devices = list_input_devices();
    let output_devices = list_output_devices();
    update_tray_audio_devices_with_lists(app, settings, &input_devices, &output_devices);

    // Translation submenu: when to translate (the mode), then which service does it.
    let mut translation_items = Vec::new();
    for (mode, label) in TRANSLATION_MODES {
        let item = CheckMenuItem::with_id(app, format!("trans_mode:{}", mode), label, true, settings.translation_mode == mode, None::<&str>)?;
        translation_items.push((format!("mode:{}", mode), item));
    }
    for (provider, label) in TRANSLATION_PROVIDERS {
        // Apple's engines only where Apple Translation runs; the chosen one always stays listed.
        let apple = crate::apple_translate::is_apple(provider) || crate::apple_translate::is_apple_intelligence(provider);
        let checked = provider_key(settings) == provider;
        let fits = !apple || crate::apple_translate::is_available();
        if !fits && !checked {
            continue;
        }
        let item = CheckMenuItem::with_id(app, format!("trans_provider:{}", provider), label, !settings.translation_off(), checked, None::<&str>)?;
        translation_items.push((format!("provider:{}", provider), item));
    }
    let trans_sep = PredefinedMenuItem::separator(app)?;
    let mut trans_items_refs: Vec<&dyn tauri::menu::IsMenuItem<tauri::Wry>> = Vec::new();
    for (i, (_, item)) in translation_items.iter().enumerate() {
        if i == TRANSLATION_MODES.len() {
            trans_items_refs.push(&trans_sep);
        }
        trans_items_refs.push(item);
    }
    let trans_submenu = Submenu::with_items(app, "Translation", true, &trans_items_refs)?;
    let _ = TRANSLATION_ITEMS.set(translation_items.clone());

    let sep2 = PredefinedMenuItem::separator(app)?;
    let update_item = MenuItem::with_id(app, "check_updates", "Check for Updates…", true, None::<&str>)?;
    let _ = UPDATE_ITEM.set(update_item.clone());
    let pref_item = MenuItem::with_id(app, "preferences", "Settings...", true, None::<&str>)?;
    let guide_item = MenuItem::with_id(app, "onboarding", "Welcome Guide…", true, None::<&str>)?;
    let sep3 = PredefinedMenuItem::separator(app)?;
    let quit_item = MenuItem::with_id(app, "quit", "Quit Bifaria", true, None::<&str>)?;

    let menu = Menu::with_items(app, &[
        &status_item,
        &eject_item,
        &sep0,
        &toggle_item,
        &sep1,
        &input_submenu,
        &output_submenu,
        &trans_submenu,
        &sep2,
        &update_item,
        &pref_item,
        &guide_item,
        &sep3,
        &quit_item,
    ])?;

    let _ = TRAY_MENU.set(menu.clone());

    let mut builder = TrayIconBuilder::with_id("main-tray")
        .menu(&menu)
        .show_menu_on_left_click(true)
        .tooltip("Bifaria — LLM Ejected (RAM Saved)")
        .icon_as_template(SLEEPING_ICON_IS_TEMPLATE)
        .on_menu_event(|app, event| {
            let id = event.id().as_ref();
            if id == "toggle_recording" {
                if let Some(state) = app.try_state::<crate::AppState>() {
                    let controller = std::sync::Arc::clone(&state.controller);
                    tauri::async_runtime::spawn(async move {
                        let _ = controller.toggle_recording("tray menu");
                    });
                }
            } else if id == "retry_translation" {
                if let Some(state) = app.try_state::<crate::AppState>() {
                    if let Err(e) = state.controller.retry_translation() {
                        log::warn!("Retry translation from tray: {}", e);
                    }
                }
            } else if id == "preferences" {
                open_preferences(app);
            } else if id == "onboarding" {
                open_onboarding(app);
            } else if id == "check_updates" {
                crate::updater::on_tray_click(app);
            } else if id == "eject_model" {
                if let Some(state) = app.try_state::<crate::AppState>() {
                    state.controller.eject_model();
                }
            } else if id == "model_status" {
                // status is informational only; also trigger eject when loaded (tapping the status line)
                if let Some(state) = app.try_state::<crate::AppState>() {
                    if state.controller.is_model_loaded() {
                        state.controller.eject_model();
                    }
                }
            } else if id == "quit" {
                std::process::exit(0);
            } else if let Some(dev) = id.strip_prefix("input_dev:") {
                if let Some(state) = app.try_state::<crate::AppState>() {
                    let _ = state.controller.set_audio_device("input", dev);
                }
            } else if let Some(dev) = id.strip_prefix("output_dev:") {
                if let Some(state) = app.try_state::<crate::AppState>() {
                    let _ = state.controller.set_audio_device("output", dev);
                }
            } else if let Some(provider) = id.strip_prefix("trans_provider:") {
                if let Some(state) = app.try_state::<crate::AppState>() {
                    let _ = state.controller.set_translation_provider(provider);
                }
            } else if let Some(mode) = id.strip_prefix("trans_mode:") {
                if let Some(state) = app.try_state::<crate::AppState>() {
                    let mut settings = state.controller.get_settings();
                    settings.translation_mode = mode.to_string();
                    if state.controller.save_settings(settings.clone()).is_ok() {
                        crate::apply_hotkeys(app, &state, &settings);
                    }
                }
            }
        });

    if let Ok(icon) = tauri::image::Image::from_bytes(SLEEPING_ICON) {
        builder = builder.icon(icon);
    } else if let Some(icon) = app.default_window_icon() {
        builder = builder.icon(icon.clone());
    }

    let tray = builder.build(app)?;
    #[cfg(target_os = "macos")]
    let _ = tray.set_title(Some(""));

    Ok(tray)
}

#[cfg(test)]
mod tests {
    use super::retry_label;

    #[test]
    fn retry_label_names_languages_and_text() {
        assert_eq!(
            retry_label("uk", "English", "Привіт"),
            "Retry Failed Translation (Ukrainian → English): \"Привіт\""
        );
        assert_eq!(retry_label("de", "en", "  "), "Retry Failed Translation (German → English)");
    }

    #[test]
    fn retry_label_shortens_long_text() {
        let label = retry_label("uk", "en", "Привіт, як справи?\nЦе дуже довгий текст для меню");
        assert_eq!(label, "Retry Failed Translation (Ukrainian → English): \"Привіт, як справи? Це дуже дов…\"");
    }
}
