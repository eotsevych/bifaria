use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use serde::Serialize;
use tauri::{AppHandle, Emitter};
use tauri_plugin_notification::NotificationExt;
use tauri_plugin_updater::UpdaterExt;

use crate::tray;

/// Delay before the first background check, so startup (model pre-warm, tray, hotkeys) is not slowed down.
const STARTUP_CHECK_DELAY: Duration = Duration::from_secs(30);
/// How often a long-running instance re-checks GitHub Releases.
const PERIODIC_CHECK_INTERVAL: Duration = Duration::from_secs(24 * 60 * 60);

/// Event the settings window listens to; payload is [`UpdateStatus`].
const STATUS_EVENT: &str = "update-status";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum UpdateState {
    Idle,
    Checking,
    UpToDate,
    Available,
    Downloading,
    Installing,
    Error,
}

impl UpdateState {
    fn is_busy(self) -> bool {
        matches!(self, UpdateState::Checking | UpdateState::Downloading | UpdateState::Installing)
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct UpdateStatus {
    pub state: UpdateState,
    pub current_version: String,
    /// Version of the available / downloading update.
    pub version: Option<String>,
    /// Download progress 0-100, when the server reports a size.
    pub percent: Option<u8>,
    pub message: Option<String>,
    /// Installed from the Microsoft Store, which delivers updates itself.
    pub store_managed: bool,
}

/// True when Bifaria runs from its Microsoft Store (MSIX) package. The Store installs updates, and
/// its policies forbid an app replacing its own files, so the built-in updater stays off there.
pub fn store_managed() -> bool {
    static MANAGED: OnceLock<bool> = OnceLock::new();
    *MANAGED.get_or_init(|| {
        #[cfg(windows)]
        {
            extern "system" {
                fn GetCurrentPackageFullName(length: *mut u32, name: *mut u16) -> i32;
            }
            const APPMODEL_ERROR_NO_PACKAGE: i32 = 15700;
            let mut length = 0u32;
            // With no buffer this only reports whether the process has a package identity.
            let rc = unsafe { GetCurrentPackageFullName(&mut length, std::ptr::null_mut()) };
            rc != APPMODEL_ERROR_NO_PACKAGE
        }
        #[cfg(not(windows))]
        {
            false
        }
    })
}

struct Inner {
    state: UpdateState,
    version: Option<String>,
    percent: Option<u8>,
    message: Option<String>,
    /// Last version the user got a system notification for, so each release notifies once.
    notified_version: Option<String>,
}

static STATUS: Mutex<Inner> = Mutex::new(Inner {
    state: UpdateState::Idle,
    version: None,
    percent: None,
    message: None,
    notified_version: None,
});

pub fn status(app: &AppHandle) -> UpdateStatus {
    let inner = STATUS.lock().unwrap_or_else(|e| e.into_inner());
    UpdateStatus {
        state: inner.state,
        current_version: app.package_info().version.to_string(),
        version: inner.version.clone(),
        percent: inner.percent,
        message: inner.message.clone(),
        store_managed: store_managed(),
    }
}

fn set_state(app: &AppHandle, state: UpdateState, version: Option<String>, percent: Option<u8>, message: Option<String>) {
    {
        let mut inner = STATUS.lock().unwrap_or_else(|e| e.into_inner());
        inner.state = state;
        inner.version = version;
        inner.percent = percent;
        inner.message = message;
    }
    let status = status(app);
    tray::set_update_item(app, &tray_label(&status), !status.state.is_busy());
    let _ = app.emit(STATUS_EVENT, &status);
}

/// Moves into a busy state unless another check/install is already running.
fn try_begin(app: &AppHandle, state: UpdateState) -> bool {
    let version = {
        let inner = STATUS.lock().unwrap_or_else(|e| e.into_inner());
        if inner.state.is_busy() {
            return false;
        }
        inner.version.clone()
    };
    set_state(app, state, version, None, None);
    true
}

fn tray_label(status: &UpdateStatus) -> String {
    let version = status.version.as_deref().unwrap_or_default();
    match status.state {
        UpdateState::Idle => "Check for Updates…".to_string(),
        UpdateState::Checking => "Checking for Updates…".to_string(),
        UpdateState::UpToDate => "✓ Bifaria Is Up to Date".to_string(),
        UpdateState::Available => format!("⬆ Install Update {} & Restart", version),
        UpdateState::Downloading => match status.percent {
            Some(p) => format!("Downloading Update… {}%", p),
            None => "Downloading Update…".to_string(),
        },
        UpdateState::Installing => "Installing Update…".to_string(),
        UpdateState::Error => "Update Failed — Try Again".to_string(),
    }
}

/// Starts the background update loop. Release builds only: dev builds have no published update feed.
pub fn spawn_background_checks(app: &AppHandle) {
    if store_managed() {
        tray::set_update_item(app, "Updates via Microsoft Store", false);
        return;
    }
    if cfg!(debug_assertions) {
        return;
    }
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(STARTUP_CHECK_DELAY).await;
        loop {
            check(&app, false).await;
            tokio::time::sleep(PERIODIC_CHECK_INTERVAL).await;
        }
    });
}

/// Tray menu handler: installs a known update, otherwise checks for one.
pub fn on_tray_click(app: &AppHandle) {
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        if status(&app).state == UpdateState::Available {
            if let Err(e) = install(&app).await {
                log::warn!("Update install not started: {}", e);
            }
        } else {
            check(&app, true).await;
        }
    });
}

/// Checks the release feed. Background checks stay silent on "no update" and on errors;
/// user-initiated checks report both.
pub async fn check(app: &AppHandle, user_initiated: bool) {
    if store_managed() {
        return;
    }
    let previous = status(app);
    if !try_begin(app, UpdateState::Checking) {
        return;
    }

    let result = match app.updater() {
        Ok(updater) => updater.check().await.map_err(|e| e.to_string()),
        Err(e) => Err(e.to_string()),
    };

    match result {
        Ok(Some(update)) => {
            log::info!("Update available: {} -> {}", update.current_version, update.version);
            set_state(app, UpdateState::Available, Some(update.version.clone()), None, None);
            if !user_initiated {
                notify_once(app, &update.version);
            }
        }
        Ok(None) => {
            log::info!("Bifaria is up to date");
            let state = if user_initiated { UpdateState::UpToDate } else { UpdateState::Idle };
            set_state(app, state, None, None, None);
        }
        Err(e) => {
            log::warn!("Update check failed: {}", e);
            if user_initiated {
                set_state(app, UpdateState::Error, None, None, Some(e));
            } else {
                // Offline or GitHub hiccup: keep whatever the user saw before.
                set_state(app, previous.state, previous.version, previous.percent, previous.message);
            }
        }
    }
}

/// Downloads and installs the latest update, then restarts. Returns an error without
/// changing anything when the update cannot start (e.g. a recording is in progress).
pub async fn install(app: &AppHandle) -> Result<(), String> {
    if tray::is_recording() {
        // Replacing the app mid-dictation would lose the recording.
        return Err("Finish the current recording first.".to_string());
    }
    if store_managed() {
        return Err("Updates come from the Microsoft Store.".to_string());
    }
    if !try_begin(app, UpdateState::Downloading) {
        return Err("An update check or install is already running.".to_string());
    }

    // Re-check so we install the latest release, not one that was superseded since the last check.
    let update = match app.updater() {
        Ok(updater) => updater.check().await.map_err(|e| e.to_string()),
        Err(e) => Err(e.to_string()),
    };
    let update = match update {
        Ok(Some(update)) => update,
        Ok(None) => {
            set_state(app, UpdateState::UpToDate, None, None, None);
            return Ok(());
        }
        Err(e) => {
            set_state(app, UpdateState::Error, None, None, Some(e.clone()));
            return Err(e);
        }
    };

    let version = update.version.clone();
    set_state(app, UpdateState::Downloading, Some(version.clone()), None, None);

    let mut downloaded: u64 = 0;
    let mut last_percent: Option<u8> = None;
    let result = update
        .download_and_install(
            |chunk, total| {
                downloaded += chunk as u64;
                let percent = total
                    .filter(|t| *t > 0)
                    .map(|t| ((downloaded * 100) / t).min(100) as u8);
                if percent != last_percent {
                    last_percent = percent;
                    set_state(app, UpdateState::Downloading, Some(version.clone()), percent, None);
                }
            },
            || set_state(app, UpdateState::Installing, Some(version.clone()), None, None),
        )
        .await;

    match result {
        Ok(()) => {
            log::info!("Update {} installed, restarting", version);
            app.restart();
        }
        Err(e) => {
            let message = e.to_string();
            log::error!("Update install failed: {}", message);
            set_state(app, UpdateState::Error, Some(version), None, Some(message.clone()));
            Err(message)
        }
    }
}

fn notify_once(app: &AppHandle, version: &str) {
    {
        let mut inner = STATUS.lock().unwrap_or_else(|e| e.into_inner());
        if inner.notified_version.as_deref() == Some(version) {
            return;
        }
        inner.notified_version = Some(version.to_string());
    }

    let place = if cfg!(target_os = "macos") { "menu bar" } else { "tray" };
    let result = app
        .notification()
        .builder()
        .title(format!("Bifaria {} is available", version))
        .body(format!(
            "Click the Bifaria {} icon and choose Install Update, or open Settings → General.",
            place
        ))
        .show();
    if let Err(e) = result {
        log::warn!("Could not show update notification: {}", e);
    }
}

#[tauri::command]
pub fn get_update_status(app: AppHandle) -> UpdateStatus {
    status(&app)
}

#[tauri::command]
pub async fn check_for_updates(app: AppHandle) -> UpdateStatus {
    check(&app, true).await;
    status(&app)
}

#[tauri::command]
pub async fn install_update(app: AppHandle) -> Result<(), String> {
    install(&app).await
}

#[cfg(test)]
mod tests {
    use base64::Engine;

    fn updater_config() -> serde_json::Value {
        let conf: serde_json::Value =
            serde_json::from_str(include_str!("../tauri.conf.json")).expect("tauri.conf.json is valid JSON");
        conf["plugins"]["updater"].clone()
    }

    #[test]
    fn test_updater_config_parses() {
        let config: tauri_plugin_updater::Config =
            serde_json::from_value(updater_config()).expect("plugins.updater matches the plugin's schema");
        assert!(config.endpoints.iter().all(|url| url.scheme() == "https"));
        assert!(config.endpoints.iter().any(|url| url.path().ends_with("/latest.json")));
    }

    #[test]
    fn test_updater_pubkey_is_minisign_key() {
        let pubkey = updater_config()["pubkey"].as_str().unwrap_or_default().to_string();
        let decoded = base64::engine::general_purpose::STANDARD
            .decode(pubkey)
            .expect("pubkey is base64");
        let text = String::from_utf8(decoded).expect("pubkey is UTF-8");
        assert!(text.contains("minisign public key"));
    }

    #[test]
    fn test_busy_states() {
        use super::UpdateState::*;
        for s in [Checking, Downloading, Installing] {
            assert!(s.is_busy());
        }
        for s in [Idle, UpToDate, Available, Error] {
            assert!(!s.is_busy());
        }
    }
}
