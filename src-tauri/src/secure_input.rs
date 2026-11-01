//! Keeps Esc cancelling a recording while macOS Secure Input is on.
//!
//! A password field, or Terminal's Secure Keyboard Entry, turns on Secure Input, and then event
//! taps stop receiving key presses. Bifaria's hotkeys still work: modifier-only ones arrive as
//! flag changes, which keep flowing, and key combos go through the global-shortcut plugin, which
//! isn't affected. Only Esc is read from the tap (global_key_listener.rs), so while Secure Input
//! is on it is registered through the plugin as well, for as long as Bifaria is busy.

use std::sync::Arc;
use tauri::{AppHandle, Manager};
use tauri_plugin_global_shortcut::{Code, GlobalShortcutExt, Shortcut};

pub fn escape_shortcut() -> Shortcut {
    Shortcut::new(None, Code::Escape)
}

#[cfg(target_os = "macos")]
pub fn is_enabled() -> bool {
    #[link(name = "Carbon", kind = "framework")]
    extern "C" {
        fn IsSecureEventInputEnabled() -> u8;
    }
    // SAFETY: takes no arguments and only reads the system's Secure Input state.
    unsafe { IsSecureEventInputEnabled() != 0 }
}

#[cfg(not(target_os = "macos"))]
pub fn is_enabled() -> bool {
    false
}

/// If Secure Input is on, registers Esc until Bifaria is idle again. Call when a recording starts.
pub fn guard_escape(app_handle: &AppHandle) {
    if !is_enabled() {
        return;
    }
    let Some(controller) = app_handle.try_state::<crate::AppState>().map(|s| Arc::clone(&s.controller)) else {
        return;
    };
    let gs = app_handle.global_shortcut();
    let esc = escape_shortcut();
    if gs.is_registered(esc) {
        return;
    }
    if let Err(e) = gs.register(esc) {
        log::warn!("Secure Input is on, and Esc could not be registered to cancel: {}", e);
        return;
    }
    log::info!("Secure Input is on; Esc is registered as a shortcut until the recording ends");
    let app = app_handle.clone();
    tauri::async_runtime::spawn(async move {
        while controller.is_active() {
            tokio::time::sleep(std::time::Duration::from_millis(250)).await;
        }
        let _ = app.global_shortcut().unregister(escape_shortcut());
    });
}
