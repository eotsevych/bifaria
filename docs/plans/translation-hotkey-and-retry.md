# Plan: Separate translation hotkey and retry in Audio Lab

Status: part 1 (translation hotkey) implemented 2026-10-05; part 3 (tray label) and part 2 (Audio Lab retry) implemented 2026-10-14

## Background: what "Retry Last Translation" does today

The tray menu item "Retry Last Translation" (`src-tauri/src/tray.rs`) and the pill's
"↻ Retry translation" button both call `AppController::retry_translation`
(`src-tauri/src/app_controller.rs`).

- When a dictation's **service translation** (LLM / API provider) fails, Bifaria pastes the
  untranslated text instead and stores a `PendingRetry` (the text sent for translation, the
  detected source language, and the history row id). The tray item becomes enabled.
- Retrying sends that same text to the translation provider again, using the *current* settings,
  so fixing an API key or endpoint and then retrying works. On success it pastes (or copies) the
  translation, updates the history entry and disables the menu item.
- Only one failure is kept. The next recording that finishes replaces or clears it.
- Local Whisper translation failures are not retryable because they would need the audio again.
- Transcription failures are not retryable from anywhere.

The menu label is unclear: users don't know a failure is waiting, or what "last" means.

## Today's translation trigger

There is one hotkey (`AppSettings.hotkey`). Translation happens automatically when the detected
language differs from `target_lang` and isn't in `skip_languages`. A dictation can't be done
"transcribe only" or "translate now" on demand.

## Goals

1. **Two hotkeys:** one for transcription only, and one that records and then translates to
   `target_lang`.
2. **Retry in Audio Lab:** retry a failed transcription, or a failed translation, from a saved
   recording.
3. **Clearer retry in the tray:** make it obvious what will be retried.

## 1. Translation hotkey

Settings (`src-tauri/src/settings.rs`, mirrored in `src/lib/tauri.ts` / `SettingsContext.tsx`):
- Keep `hotkey` as the transcription hotkey.
- Add `translate_hotkey: String`, default empty (off) so existing users see no change.
- Decide what the main hotkey does once a translate hotkey is set. Options:
  - a) main hotkey never translates (clean split; recommended once `translate_hotkey` is set);
  - b) main hotkey keeps today's automatic translation (fully backward compatible).
  Proposal: add `auto_translate: bool`. Default `true` while `translate_hotkey` is empty, so
  behavior matches today. Setting a translate hotkey suggests turning it off.

Hotkey listener (`src-tauri/src/global_key_listener.rs`):
- The macOS CGEventTap path matches one modifier keycode (`target_code`). Extend it to two codes,
  each mapped to an intent (`Transcribe` / `Translate`), keeping both the press-to-toggle and the
  hold-to-talk gestures.
- The `tauri_plugin_global_shortcut` fallback (combos like `Control+Shift+Space`) registers both
  shortcuts.
- `update_hotkey` becomes `update_hotkeys(transcribe, translate)`. Reject identical keys.

Controller (`app_controller.rs`):
- `toggle_recording(reason)` takes an intent (e.g. `RecordingIntent { translate: bool }`), stored
  on the session when recording starts. The hotkey that stops the recording doesn't change it.
- In the processing pipeline, the `skip_translation` decision uses the intent:
  - `Transcribe` → always skip ("transcription-only hotkey").
  - `Translate` → translate even if the language is in `skip_languages`, but still skip when
    the detected language already equals `target_lang`.
  - Legacy (no translate hotkey, `auto_translate` on) → today's logic.
- Pill: show the intent while listening (e.g. "Listening… → DE") so the user knows which mode
  they started.

UI (`GeneralTab.tsx`): a second `HotkeyRecorder` "Translate hotkey", plus the
`auto_translate` toggle. Update README, the website copy (`site/index.html`, `site/i18n.js`) and
the pill hint (`show_hints`) to mention both keys.

## 2. Retry in Audio Lab

Audio Lab (`src/components/settings/AudioLabTab.tsx`, `src-tauri/src/lab.rs`) runs transcription
experiments on saved recordings but has no translation step and no link to failed dictations.

- Add an optional "Translate to" step to `LabExperimentRequest` / `LabExperimentResult` that uses
  the same `TranslationRoute` as live dictation, so provider errors show up in the lab with the
  full detail from `translation_error_detail`.
- Let the lab open a history entry (needs `storage_mode = text_audio`, so the audio is saved):
  - **Failed transcription:** rerun the pipeline on the saved audio.
  - **Failed translation:** retranslate the stored source text without retranscribing.
- On success, offer "Save to history" (reuse `history.update_translation` and add an
  equivalent for transcription text) and "Copy".
- In History/Logs, put a "Retry in Audio Lab" action on failed entries. This replaces the
  single-slot `PendingRetry` as the way to recover older failures.

## 3. Tray menu clarity

- Rename the item to show what's pending, e.g. "Retry Failed Translation (EN → DE)", and add a
  tooltip or subtitle with the start of the text.
- Keep the quick one-click path for the most recent failure. Older failures go through Audio Lab.

## Order of work

1. Translation hotkey (settings, listener, controller intent, UI). Ship on its own.
2. Tray label clarity (small).
3. Audio Lab translation step and history-backed retry.

## Open questions

- Should the translate hotkey let the user pick the target language per press (e.g. hold Shift)?
  Not for now.
- ~~Windows/Linux: confirm the global-shortcut fallback can register two modifier-only keys.~~
  Checked: Windows and Linux don't support modifier-only hotkeys at all (the listener there is a
  stub, and the recorder only offers modifier keys on Mac). Both hotkeys are key combos registered
  with tauri-plugin-global-shortcut, which handles several at once.
- `skip_languages` is stored but never read by the pipeline. Translation depends only on the
  detected, source and target languages, so the translate hotkey reuses that logic unchanged.
