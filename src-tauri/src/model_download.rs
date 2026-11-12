//! Shared plumbing for speech-model downloads.
//!
//! - Only one download runs at a time; other callers wait for it and then find the model on disk.
//! - Progress goes to Settings (`model-download-progress`) and the tray status line.
//! - The voice pill shows progress only while a recording is actually waiting for the model, and the
//!   waiting pipeline restores the pill afterwards, so a background download never leaves it stuck.
//! - On first launch the configured model downloads in the background, announced by notifications.

use std::sync::atomic::{AtomicU32, AtomicUsize, Ordering};

use tauri::{AppHandle, Emitter};
use tauri_plugin_notification::NotificationExt;

pub const PARAKEET_MODEL: &str = "parakeet-tdt-0.6b-v3";

const PROGRESS_EVENT: &str = "model-download-progress";
const NO_PERCENT: u32 = u32::MAX;

static DOWNLOAD_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());
static PILL_WAITERS: AtomicUsize = AtomicUsize::new(0);
static LAST_PERCENT: AtomicU32 = AtomicU32::new(NO_PERCENT);

/// Serializes downloads. Hold the guard for the whole check-and-download, then re-check the disk.
pub async fn lock() -> tokio::sync::MutexGuard<'static, ()> {
    DOWNLOAD_LOCK.lock().await
}

pub fn is_model_present(model_name: &str) -> bool {
    if crate::parakeet::is_parakeet(model_name) {
        crate::parakeet::ParakeetTranscriber::model_ready(model_name)
    } else {
        crate::transcribe::Transcriber::model_exists(model_name)
    }
}

/// Downloads the model if it's missing and returns its path.
pub async fn ensure(app: &AppHandle, model_name: &str) -> Result<std::path::PathBuf, String> {
    if crate::parakeet::is_parakeet(model_name) {
        crate::parakeet::ParakeetTranscriber::ensure_model(app, model_name).await
    } else {
        crate::transcribe::Transcriber::ensure_model(app, model_name).await
    }
}

pub fn began(app: &AppHandle, model_name: &str) {
    LAST_PERCENT.store(NO_PERCENT, Ordering::SeqCst);
    progress(app, model_name, 0, 0, 0);
}

/// Reports progress; UI updates are throttled to whole-percent changes.
pub fn progress(app: &AppHandle, model_name: &str, percent: u32, downloaded: u64, total: u64) {
    let percent = percent.min(99);
    if LAST_PERCENT.swap(percent, Ordering::SeqCst) == percent {
        return;
    }
    let _ = app.emit(
        PROGRESS_EVENT,
        serde_json::json!({
            "status": "downloading",
            "model": model_name,
            "percent": percent,
            "downloaded_bytes": downloaded,
            "total_bytes": total
        }),
    );
    crate::tray::set_model_download_progress(percent);
    if PILL_WAITERS.load(Ordering::SeqCst) > 0 {
        emit_pill_progress(app, percent);
    }
}

pub fn finished(app: &AppHandle, model_name: &str, result: &Result<std::path::PathBuf, String>) {
    LAST_PERCENT.store(NO_PERCENT, Ordering::SeqCst);
    let payload = match result {
        Ok(_) => serde_json::json!({ "status": "complete", "model": model_name, "percent": 100 }),
        Err(e) => serde_json::json!({ "status": "error", "model": model_name, "error": e }),
    };
    let _ = app.emit(PROGRESS_EVENT, payload);
    crate::tray::update_tray_model_status(app, false);
}

/// Shows download progress in the pill while alive. The pipeline creates one only when a recording
/// is blocked on a missing model, and re-emits its own state after dropping it.
pub struct PillWait;

impl PillWait {
    pub fn start(app: &AppHandle) -> Self {
        PILL_WAITERS.fetch_add(1, Ordering::SeqCst);
        let percent = LAST_PERCENT.load(Ordering::SeqCst);
        emit_pill_progress(app, if percent == NO_PERCENT { 0 } else { percent });
        Self
    }
}

impl Drop for PillWait {
    fn drop(&mut self) {
        PILL_WAITERS.fetch_sub(1, Ordering::SeqCst);
    }
}

fn emit_pill_progress(app: &AppHandle, percent: u32) {
    let _ = app.emit(
        "assistant-state-changed",
        serde_json::json!({
            "state": "transcribing",
            "title": "Downloading speech model…",
            "subtitle": format!("First-time setup · {}%", percent)
        }),
    );
}

/// On first launch (or after the user picks a model that isn't on disk yet), downloads the configured
/// model in the background so the first recording doesn't have to wait for it.
pub fn spawn_initial_download(app: &AppHandle, model_name: String) {
    if is_model_present(&model_name) {
        return;
    }
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        notify(
            &app,
            "Setting up Bifaria",
            "Downloading the speech model in the background. You'll get a notification when it's ready.",
        );
        match ensure(&app, &model_name).await {
            Ok(_) => notify(&app, "Bifaria is ready", "Press your hotkey and start talking."),
            Err(e) => {
                log::error!("Initial model download failed: {}", e);
                let reason: String = e.chars().take(140).collect();
                notify(
                    &app,
                    "Speech model download failed",
                    &format!("{}. Bifaria will try again the next time you record.", reason),
                );
            }
        }
    });
}

fn notify(app: &AppHandle, title: &str, body: &str) {
    if let Err(e) = app.notification().builder().title(title).body(body).show() {
        log::warn!("Could not show notification: {}", e);
    }
}
