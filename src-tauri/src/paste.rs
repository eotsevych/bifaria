use crate::clipboard_snapshot::Snapshot;
use arboard::Clipboard;
use std::sync::{Mutex, OnceLock};
use std::thread::sleep;
use std::time::Duration;

#[cfg(not(target_os = "macos"))]
use enigo::{Direction, Enigo, Key, Keyboard, Settings};

/// One clipboard for the whole run. On Linux the copied text is served by the app that set it,
/// so a `Clipboard` dropped right after `set_text` can take the text with it before Ctrl+V reads it.
static CLIPBOARD: OnceLock<Mutex<Clipboard>> = OnceLock::new();

pub(crate) fn clipboard() -> Result<&'static Mutex<Clipboard>, String> {
    if let Some(clipboard) = CLIPBOARD.get() {
        return Ok(clipboard);
    }
    let clipboard = Clipboard::new().map_err(|e| format!("Clipboard error: {}", e))?;
    Ok(CLIPBOARD.get_or_init(|| Mutex::new(clipboard)))
}

pub fn copy_to_clipboard(text: &str) -> Result<(), String> {
    // Native Wayland apps read the Wayland clipboard; wl-copy writes it directly.
    #[cfg(target_os = "linux")]
    if crate::linux_paste::Session::detect().wayland && crate::linux_paste::on_path("wl-copy") {
        match crate::linux_paste::wl_copy(text) {
            Ok(()) => return Ok(()),
            Err(e) => log::warn!("wl-copy failed, using the X11 clipboard: {}", e),
        }
    }
    clipboard()?
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .set_text(text)
        .map_err(|e| format!("Failed to set clipboard text: {}", e))?;
    Ok(())
}

/// Copies `text` and sends the paste keys. `Ok(Some(note))` means the keys went out through a
/// fallback that may not reach every app; the note says what to do, and is given only once per
/// run. `Err` means the text is on the clipboard but couldn't be pasted. With `keep_clipboard`,
/// what was on the clipboard before (text, an image, files; see `clipboard_snapshot`) is put back
/// after a paste known to have gone out; otherwise `text` stays for the user to paste by hand.
pub fn copy_and_paste(text: &str, keep_clipboard: bool) -> Result<Option<String>, String> {
    let previous = if keep_clipboard { take_previous() } else { None };
    let pasted = paste(text);
    if let (Ok(None), Some(previous)) = (&pasted, previous) {
        restore_later(text.to_string(), previous, RESTORE_DELAY);
    }
    pasted
}

/// How long after the paste keys the previous clipboard is put back: the target app reads the
/// clipboard when it handles the keys, not when they are sent.
const RESTORE_DELAY: Duration = Duration::from_millis(500);

/// The clipboard the latest paste will put back, and that paste's number.
static RESTORE: Mutex<(u64, Option<Snapshot>)> = Mutex::new((0, None));

/// What to put back after this paste: what an earlier paste is still waiting to restore (so two
/// dictations in a row bring back the user's clipboard, not the first transcript), else what is
/// on the clipboard now.
fn take_previous() -> Option<Snapshot> {
    let pending = RESTORE.lock().unwrap_or_else(|e| e.into_inner()).1.take();
    pending.or_else(Snapshot::take)
}

/// Puts `previous` back after `delay`, unless a newer paste has taken it over or the clipboard no
/// longer holds `pasted` (the user copied something else meanwhile, which is kept).
fn restore_later(pasted: String, previous: Snapshot, delay: Duration) {
    let id = {
        let mut restore = RESTORE.lock().unwrap_or_else(|e| e.into_inner());
        restore.0 += 1;
        restore.1 = Some(previous);
        restore.0
    };
    std::thread::spawn(move || {
        sleep(delay);
        let mut restore = RESTORE.lock().unwrap_or_else(|e| e.into_inner());
        if restore.0 != id {
            return;
        }
        let Some(previous) = restore.1.take() else {
            return;
        };
        if read_clipboard_text().as_deref() != Some(pasted.as_str()) {
            log::info!("The clipboard changed after the paste; leaving it as it is");
            return;
        }
        if let Err(e) = previous.restore() {
            log::warn!("Couldn't put the previous clipboard back: {}", e);
        }
    });
}

fn read_clipboard_text() -> Option<String> {
    // Native Wayland apps' copies are on the Wayland clipboard; XWayland's can be stale.
    #[cfg(target_os = "linux")]
    if crate::linux_paste::Session::detect().wayland && crate::linux_paste::on_path("wl-paste") {
        match crate::linux_paste::wl_paste() {
            Ok(text) => return text,
            Err(e) => log::warn!("wl-paste failed, reading the X11 clipboard: {}", e),
        }
    }
    clipboard().ok()?.lock().unwrap_or_else(|e| e.into_inner()).get_text().ok()
}

fn paste(text: &str) -> Result<Option<String>, String> {
    // Step 1: Copy to clipboard
    copy_to_clipboard(text)?;

    // Give OS a moment to register clipboard change
    sleep(Duration::from_millis(60));

    // Step 2: Simulate keyboard paste
    #[cfg(target_os = "macos")]
    {
        use std::ffi::c_void;
        type CGEventRef = *mut c_void;
        type CGEventSourceRef = *mut c_void;

        #[link(name = "CoreGraphics", kind = "framework")]
        extern "C" {
            fn CGEventSourceCreate(stateID: i32) -> CGEventSourceRef;
            fn CGEventCreateKeyboardEvent(source: CGEventSourceRef, virtualKey: u16, keyDown: bool) -> CGEventRef;
            fn CGEventSetFlags(event: CGEventRef, flags: u64);
            fn CGEventPost(tap: u32, event: CGEventRef);
            fn CFRelease(cf: *const c_void);
        }

        const K_CG_EVENT_SOURCE_STATE_COMBINED_SESSION_STATE: i32 = 0;
        const K_CG_SESSION_EVENT_TAP: u32 = 1;
        const K_CG_EVENT_FLAG_MASK_COMMAND: u64 = 0x00100000;
        const K_VK_ANSI_V: u16 = 0x09;

        // Without Accessibility macOS drops the keys silently. A new signing certificate also
        // voids the old grant, even though Settings still shows Bifaria switched on.
        if !crate::global_key_listener::is_accessibility_trusted() {
            static PROMPT: std::sync::Once = std::sync::Once::new();
            PROMPT.call_once(crate::global_key_listener::request_accessibility_prompt);
            return Err("Turn on Bifaria in Privacy & Security → Accessibility (remove it and add it again if it's already on). The text is on the clipboard.".to_string());
        }

        unsafe {
            let source = CGEventSourceCreate(K_CG_EVENT_SOURCE_STATE_COMBINED_SESSION_STATE);
            let down = CGEventCreateKeyboardEvent(source, K_VK_ANSI_V, true);
            let up = CGEventCreateKeyboardEvent(source, K_VK_ANSI_V, false);

            CGEventSetFlags(down, K_CG_EVENT_FLAG_MASK_COMMAND);
            CGEventSetFlags(up, 0);

            // Post single Cmd+V keyboard event to Session Event Tap
            CGEventPost(K_CG_SESSION_EVENT_TAP, down);
            CGEventPost(K_CG_SESSION_EVENT_TAP, up);

            CFRelease(down as _);
            CFRelease(up as _);
            if !source.is_null() {
                CFRelease(source as _);
            }
        }
        Ok(None)
    }

    #[cfg(target_os = "linux")]
    {
        use crate::linux_paste::{send_ctrl_v, PasteToolError, Session};
        let session = Session::detect();
        let tool_problem = match send_ctrl_v(session) {
            Ok(tool) => {
                log::info!("Pasted with {}", tool.command());
                return Ok(None);
            }
            Err(PasteToolError::NoTool) => None,
            Err(PasteToolError::Failed(why)) => Some(why),
        };
        enigo_ctrl_v().map_err(|e| {
            let why = tool_problem.as_deref().map(|t| format!("{t}; {e}")).unwrap_or(e);
            format!("The text is on the clipboard: press Ctrl+V. Bifaria couldn't send the paste keys ({why}).")
        })?;
        // On Wayland, enigo's keys reach only XWayland apps.
        if session.wayland && take_wayland_note() {
            let note = match tool_problem {
                Some(_) => "If nothing was pasted, press Ctrl+V: the text is on the clipboard. Your paste tool failed; see the log in Settings › Logs.",
                None => "If nothing was pasted, press Ctrl+V: the text is on the clipboard. To paste into every app on Wayland, install ydotool, dotool or wtype.",
            };
            return Ok(Some(note.to_string()));
        }
        Ok(None)
    }

    #[cfg(target_os = "windows")]
    {
        enigo_ctrl_v().map_err(|e| format!("The text is on the clipboard: press Ctrl+V. Bifaria couldn't send the paste keys ({e})."))?;
        Ok(None)
    }
}

#[cfg(not(target_os = "macos"))]
fn enigo_ctrl_v() -> Result<(), String> {
    let mut enigo = Enigo::new(&Settings::default()).map_err(|e| e.to_string())?;
    enigo.key(Key::Control, Direction::Press).map_err(|e| e.to_string())?;
    let click = enigo.key(Key::Unicode('v'), Direction::Click).map_err(|e| e.to_string());
    // Release Control even if the V failed, so it doesn't stay held.
    enigo.key(Key::Control, Direction::Release).map_err(|e| e.to_string())?;
    click
}

/// True the first time it's called in this run.
#[cfg(target_os = "linux")]
fn take_wayland_note() -> bool {
    use std::sync::atomic::{AtomicBool, Ordering};
    static SHOWN: AtomicBool = AtomicBool::new(false);
    !SHOWN.swap(true, Ordering::SeqCst)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Uses the real clipboard (and overwrites it), so it runs only on request:
    /// `cargo test --lib keep_clipboard -- --ignored --test-threads=1`.
    #[test]
    #[ignore]
    fn keep_clipboard_puts_back_the_users_clipboard_unless_they_copied_something_new() {
        let delay = Duration::from_millis(100);
        let wait = || sleep(Duration::from_millis(300));

        // One dictation: the user's text comes back.
        copy_to_clipboard("mine").unwrap();
        let previous = take_previous().unwrap();
        copy_to_clipboard("dictated").unwrap();
        restore_later("dictated".into(), previous, delay);
        wait();
        assert_eq!(read_clipboard_text().as_deref(), Some("mine"));

        // Two in a row: still the user's text, not the first transcript.
        let previous = take_previous().unwrap();
        copy_to_clipboard("first").unwrap();
        restore_later("first".into(), previous, delay);
        let previous = take_previous().unwrap();
        copy_to_clipboard("second").unwrap();
        restore_later("second".into(), previous, delay);
        wait();
        assert_eq!(read_clipboard_text().as_deref(), Some("mine"));

        // Copied something new before the restore: that is kept.
        let previous = take_previous().unwrap();
        copy_to_clipboard("dictated").unwrap();
        restore_later("dictated".into(), previous, delay);
        copy_to_clipboard("new copy").unwrap();
        wait();
        assert_eq!(read_clipboard_text().as_deref(), Some("new copy"));

        // An image comes back as the same image.
        let pixels: Vec<u8> = (0..4 * 4 * 4).map(|i| (i * 7) as u8 | 0x0F).collect();
        let image = arboard::ImageData { width: 4, height: 4, bytes: pixels.into() };
        clipboard().unwrap().lock().unwrap().set_image(image.clone()).unwrap();
        let previous = take_previous().unwrap();
        copy_to_clipboard("dictated").unwrap();
        restore_later("dictated".into(), previous, delay);
        wait();
        let back = clipboard().unwrap().lock().unwrap().get_image().unwrap();
        assert_eq!((back.width, back.height), (4, 4));
        assert_eq!(read_clipboard_text(), None, "only the image, as before");

        // Files come back as the same files.
        let file = std::env::temp_dir().join(format!("bifaria_keep_clipboard_{}.txt", std::process::id()));
        std::fs::write(&file, "x").unwrap();
        clipboard().unwrap().lock().unwrap().set().file_list(&[&file]).unwrap();
        let previous = take_previous().unwrap();
        copy_to_clipboard("dictated").unwrap();
        restore_later("dictated".into(), previous, delay);
        wait();
        let files = clipboard().unwrap().lock().unwrap().get().file_list().unwrap();
        let _ = std::fs::remove_file(&file);
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].file_name(), file.file_name());
    }
}
