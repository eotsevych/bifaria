import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";

export const isTauri = (): boolean => {
  return typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;
};

export type TranslationMode = "off" | "hotkey" | "auto";

export interface BackendSettings {
  api_key: string;
  source_lang: string;
  target_lang: string;
  hotkey: string;
  /** When dictation is translated: never, only with `translate_hotkey`, or automatically by the
   * launch hotkey whenever the speech isn't in the target language. */
  translation_mode?: TranslationMode;
  /** Second hotkey that records and always translates; used in "hotkey" mode. Empty = not set. */
  translate_hotkey?: string;
  sound_start: boolean;
  sound_stop: boolean;
  sound_error: boolean;
  auto_paste: boolean;
  keep_clipboard?: boolean;
  show_hints?: boolean;
  storage_mode: string;
  storage_cap_mb: number;
  retention_days: number;
  model_name: string;
  input_device?: string | null;
  output_device?: string | null;
  theme?: string;
  /** How the floating pill looks (see PILL_STYLES in components/pill/styles.tsx). */
  pill_style?: string;
  bar_scale?: number;
  /** Whether the first-run welcome guide was finished or closed. */
  onboarding_done?: boolean;
  model_idle_unload_sec?: number;
  /** Seconds without speech from the start of a recording before the pill warns; 0 = off. */
  no_speech_alert_sec?: number;
  /** When the speech model loads for a recording: while talking (faster) or after (saves RAM). */
  model_load_mode?: "on_start" | "on_stop";
  pill_center?: [number, number] | null;
  text_normalization?: boolean;
  remove_filler_words?: boolean;
  convert_numbers?: boolean;
  remove_stutters?: boolean;
  apply_self_corrections?: boolean;
  remove_noise_markers?: boolean;
  collapse_redundancy?: boolean;
  annotate_ambiguity?: boolean;
  normalize_structured_values?: boolean;
  /** "LLM", "Custom API", "Apple" (Apple's on-device Translation, macOS 26+), "Apple Intelligence"
   * or "Offline" (Firefox Translations models on the CPU, any OS). */
  translation_provider?: string;
  /** With the Apple or Offline provider: "LLM" or "Custom API" translates when it fails; "" = none. */
  apple_fallback_provider?: string;
  /** Apple Translation model to use when both are downloaded: "fast" or "quality". */
  apple_translation_model?: "fast" | "quality";
  device_backend?: string;
  audio_chunking?: boolean;
  noise_reduction?: NoiseReduction;
  mask_confidential?: boolean;
  mask_words?: string;
  mask_threshold?: number;
  mask_format?: string;
  mask_emails?: boolean;
  mask_phones?: boolean;
  mask_cards?: boolean;
  excluded_languages?: string;
  llm_endpoint?: string;
  llm_model?: string;
  llm_api_key?: string;
  local_llm_url?: string;
  local_llm_model?: string;
  local_llm_api_key?: string;
  custom_api_url?: string;
  custom_api_key?: string;
  custom_api_model?: string;
  prompt_template?: string;
  last_version?: string | null;
}

export interface HardwareProfile {
  os: string;
  arch: string;
  logical_cores: number;
  recommended_threads: number;
  has_avx2: boolean;
  has_avx512: boolean;
  has_vnni: boolean;
  has_neon: boolean;
  has_metal: boolean;
  summary: string;
}

/** The GPUs Whisper can run on (Metal on a Mac, Vulkan on Windows and Linux); empty without one. */
export async function fetchGpuDevices(): Promise<string[]> {
  if (!isTauri()) return ["Apple M2"];
  return await invoke<string[]>("get_gpu_devices");
}

/** Where Parakeet runs, e.g. "the Neural Engine" or "the CPU (6 threads)". */
export async function fetchParakeetEngine(): Promise<string> {
  if (!isTauri()) return "the Neural Engine";
  return await invoke<string>("get_parakeet_engine");
}

export async function fetchHardwareProfile(): Promise<HardwareProfile | null> {
  if (!isTauri()) {
    return {
      os: "macos",
      arch: "aarch64",
      logical_cores: 8,
      recommended_threads: 6,
      has_avx2: false,
      has_avx512: false,
      has_vnni: false,
      has_neon: true,
      has_metal: true,
      summary:
        "MACOS aarch64 (8 cores) | Accelerators: [ARM NEON, Apple Metal/ANE] | Recommended Threads: 6",
    };
  }
  try {
    return await invoke<HardwareProfile>("get_hardware_profile");
  } catch (err) {
    console.error("Failed to get hardware profile:", err);
    return null;
  }
}

export interface HistoryItem {
  id: number;
  timestamp: string;
  raw_text: string;
  translated_text: string;
  source_lang: string;
  target_lang: string;
  duration: number;
  audio_path?: string | null;
}

export interface AssistantStateEvent {
  state: "idle" | "listening" | "transcribing" | "translating" | "done" | "error";
  title: string;
  subtitle: string | null;
  text?: string;
  /** An error the user needs to read: shown in full (wrapped, not truncated) in a larger pill. */
  long?: boolean;
  /** A failed translation that can be sent again: the pill shows a Retry button. */
  retry?: boolean;
  /** The chosen microphone isn't connected: the pill lists these connected mics to pick from. */
  devices?: string[];
  /** While listening: the microphone stopped delivering audio; title and subtitle explain it. */
  warning?: boolean;
  /** While listening: the mic is being reopened, or reopening didn't bring sound back. */
  mic?: "reconnecting" | "down";
  /** While listening: what happens to the speech. "translate" = will be translated (translate
   * hotkey); "auto" = translated if it isn't in the target language; "transcribe" = never. */
  mode?: "translate" | "transcribe" | "auto" | null;
}

export async function fetchSettings(): Promise<BackendSettings | null> {
  if (!isTauri()) return null;
  try {
    return await invoke<BackendSettings>("get_settings");
  } catch (err) {
    console.error("Failed to get settings from Tauri:", err);
    return null;
  }
}

export async function persistSettings(settings: BackendSettings): Promise<boolean> {
  if (!isTauri()) return true;
  try {
    await invoke("save_settings", { newSettings: settings });
    return true;
  } catch (err) {
    console.error("Failed to save settings to Tauri:", err);
    return false;
  }
}

export async function triggerToggleRecording(): Promise<void> {
  if (!isTauri()) return;
  try {
    await invoke("toggle_recording");
  } catch (err) {
    console.error("Failed to toggle recording:", err);
  }
}

/** Sends the last failed translation again. Resolves to an error message, or null when it started. */
export async function triggerRetryTranslation(): Promise<string | null> {
  if (!isTauri()) return null;
  try {
    await invoke("retry_translation");
    return null;
  } catch (err) {
    return String(err);
  }
}

/** Saves the microphone picked in the pill and starts the recording the hotkey asked for. */
export async function chooseMicrophone(name: string): Promise<void> {
  if (!isTauri()) return;
  try {
    await invoke("choose_microphone", { name });
  } catch (err) {
    console.error("Failed to choose microphone:", err);
  }
}

export async function triggerCancelRecording(): Promise<void> {
  if (!isTauri()) return;
  try {
    await invoke("cancel_recording");
  } catch (err) {
    console.error("Failed to cancel recording:", err);
  }
}

export async function fetchHistory(limit = 50): Promise<HistoryItem[]> {
  if (!isTauri()) return [];
  try {
    return await invoke<HistoryItem[]>("get_history", { limit });
  } catch (err) {
    console.error("Failed to fetch history:", err);
    return [];
  }
}

/** Size in bytes of the recordings saved to history. */
export async function fetchHistoryAudioBytes(): Promise<number> {
  if (!isTauri()) return 0;
  try {
    return await invoke<number>("get_history_audio_bytes");
  } catch (err) {
    console.error("Failed to read history size:", err);
    return 0;
  }
}

export async function triggerClearHistory(): Promise<boolean> {
  if (!isTauri()) return true;
  try {
    await invoke("clear_history");
    return true;
  } catch (err) {
    console.error("Failed to clear history:", err);
    return false;
  }
}

export async function fetchAudioDevices(): Promise<{
  input_devices: string[];
  output_devices: string[];
}> {
  if (!isTauri()) return { input_devices: [], output_devices: [] };
  try {
    return await invoke<{ input_devices: string[]; output_devices: string[] }>("get_audio_devices");
  } catch (err) {
    console.error("Failed to fetch audio devices:", err);
    return { input_devices: [], output_devices: [] };
  }
}

export async function startMicTest(deviceName?: string | null): Promise<boolean> {
  if (!isTauri()) return true;
  try {
    await invoke("start_mic_test", { deviceName: deviceName || null });
    return true;
  } catch (err) {
    console.error("Failed to start mic test:", err);
    return false;
  }
}

export async function stopMicTest(): Promise<boolean> {
  if (!isTauri()) return true;
  try {
    await invoke("stop_mic_test");
    return true;
  } catch (err) {
    console.error("Failed to stop mic test:", err);
    return false;
  }
}

export async function playTestSound(): Promise<boolean> {
  if (!isTauri()) return true;
  try {
    await invoke("play_test_sound");
    return true;
  } catch (err) {
    console.error("Failed to play test sound:", err);
    return false;
  }
}

/** While on, the hotkeys send `practice-hotkey` to the welcome guide instead of recording. */
export async function setPracticeMode(on: boolean): Promise<void> {
  if (!isTauri()) return;
  try {
    await invoke("set_practice_mode", { on });
  } catch (err) {
    console.error("Failed to set practice mode:", err);
  }
}

/** A hotkey press during the welcome guide's practice run: which hotkey it was. */
export async function subscribeToPracticeHotkey(
  callback: (mode: "main" | "translate") => void,
): Promise<UnlistenFn | null> {
  if (!isTauri()) return null;
  try {
    return await listen<{ mode: "main" | "translate" }>("practice-hotkey", (event) => {
      callback(event.payload.mode);
    });
  } catch (err) {
    console.error("Failed to listen to practice hotkeys:", err);
    return null;
  }
}

/** Marks the welcome guide done and closes its window. */
export async function finishOnboarding(): Promise<void> {
  if (!isTauri()) return;
  try {
    await invoke("finish_onboarding");
  } catch (err) {
    console.error("Failed to finish the welcome guide:", err);
  }
}

/** The tab Settings was asked to open on (by What's New), if any. Asking clears it. */
export async function takeSettingsTab(): Promise<string | null> {
  if (!isTauri()) return null;
  try {
    return (await invoke<string | null>("take_settings_tab")) ?? null;
  } catch {
    return null;
  }
}

/** Sent to an open Settings window that should switch to a tab. */
export async function subscribeToSettingsTab(
  callback: (tab: string) => void,
): Promise<UnlistenFn | null> {
  if (!isTauri()) return null;
  try {
    return await listen<string>("open-settings-tab", (event) => callback(event.payload));
  } catch (err) {
    console.error("Failed to listen to settings tab requests:", err);
    return null;
  }
}

export async function openWhatsNewWindow(): Promise<void> {
  if (!isTauri()) return;
  try {
    await invoke("open_whats_new_window");
  } catch (err) {
    console.error("Failed to open what's new window:", err);
  }
}

export async function dismissWhatsNew(): Promise<void> {
  if (!isTauri()) return;
  try {
    await invoke("dismiss_whats_new");
  } catch (err) {
    console.error("Failed to dismiss what's new:", err);
  }
}

export async function openSettingsFromWhatsNew(): Promise<void> {
  if (!isTauri()) return;
  try {
    await invoke("open_settings_from_whats_new");
  } catch (err) {
    console.error("Failed to open settings from what's new:", err);
  }
}

export async function closePreferencesWindow(): Promise<void> {
  if (!isTauri()) return;
  try {
    await invoke("close_preferences_window");
  } catch (err) {
    console.error("Failed to close preferences window:", err);
  }
}

/** Asks for the pill window to be resized to fit a long message of this height (CSS px). */
export async function fitPillHeight(height: number): Promise<void> {
  if (!isTauri()) return;
  try {
    await invoke("fit_pill_height", { height });
  } catch (err) {
    console.error("Failed to fit the pill window:", err);
  }
}

export async function triggerStartDragging(): Promise<void> {
  if (!isTauri()) return;
  try {
    await invoke("start_dragging_window");
  } catch {
    try {
      const { getCurrentWebviewWindow } = await import("@tauri-apps/api/webviewWindow");
      await getCurrentWebviewWindow().startDragging();
    } catch (err) {
      console.error("Failed to start dragging window:", err);
    }
  }
}

export async function subscribeToStateChanges(
  callback: (payload: AssistantStateEvent) => void,
): Promise<UnlistenFn | null> {
  if (!isTauri()) return null;
  try {
    return await listen<AssistantStateEvent>("assistant-state-changed", (event) => {
      callback(event.payload);
    });
  } catch (err) {
    console.error("Failed to listen to state events:", err);
    return null;
  }
}

/** Microphone loudness and pitch-band energy, sent about 25 times a second while recording. */
export interface AudioLevels {
  /** Overall loudness, 0..1. */
  volume: number;
  /** Pitch energy per bar, 0..1, mirrored from the centre: [high, high-mid, mid, low, mid, high-mid, high]. */
  bands: number[];
}

export async function subscribeToAudioLevels(
  callback: (levels: AudioLevels) => void,
): Promise<UnlistenFn | null> {
  if (!isTauri()) return null;
  try {
    return await listen<AudioLevels>("audio-level", (event) => {
      callback(event.payload);
    });
  } catch (err) {
    console.error("Failed to listen to audio levels:", err);
    return null;
  }
}

export async function subscribeToMicTestLevels(
  callback: (data: { level: number; peak: number }) => void,
): Promise<UnlistenFn | null> {
  if (!isTauri()) return null;
  try {
    return await listen<{ level: number; peak: number }>("mic-test-level", (event) => {
      callback(event.payload);
    });
  } catch (err) {
    console.error("Failed to listen to mic test levels:", err);
    return null;
  }
}

export async function subscribeToModelDownload(
  callback: (progress: {
    status: string;
    model: string;
    percent: number;
    downloaded_bytes?: number;
    total_bytes?: number;
    error?: string;
  }) => void,
): Promise<UnlistenFn | null> {
  if (!isTauri()) return null;
  try {
    return await listen<{
      status: string;
      model: string;
      percent: number;
      downloaded_bytes?: number;
      total_bytes?: number;
      error?: string;
    }>("model-download-progress", (event) => {
      callback(event.payload);
    });
  } catch (err) {
    console.error("Failed to listen to model download progress:", err);
    return null;
  }
}

export interface ChunkDiagnosticEvent {
  chunk_index: number;
  trigger_reason: string;
  duration_sec: number;
  has_overlap: boolean;
  overlap_ms: number;
  silence_detected_ms: number;
  rms_energy: number;
  timestamp: string;
  detail: string;
}

export interface TranscriptionDiagnosticLog {
  id: number;
  timestamp: string;
  audio_duration_sec: number;
  audio_samples_count: number;
  vad_trim_ms: number;
  vad_original_sec: number;
  vad_trimmed_sec: number;
  vad_silence_removed_sec: number;
  model_name: string;
  gpu_metal_active: boolean;
  threads_count: number;
  whisper_inference_ms: number;
  whisper_speed_factor: number;
  detected_lang: string;
  raw_text: string;
  translation_skipped: boolean;
  translation_skip_reason: string;
  translation_ms: number;
  /** Why translation failed, when it did; final_text is then the spoken text. */
  translation_error?: string | null;
  /** The service that translated, e.g. "Apple Translation (on-device) uk → en"; null when skipped. */
  translation_provider?: string | null;
  final_text: string;
  /** The text in the spoken language, when final_text is its translation. */
  spoken_text?: string | null;
  clipboard_paste_ms: number;
  history_save_ms: number;
  total_pipeline_ms: number;
  audio_filename?: string | null;
  vad_audio_filename?: string | null;
  whisper_raw_output?: string | null;
  segments_count?: number;
  chunk_events?: ChunkDiagnosticEvent[];
  whole_track?: boolean;
  action_logs?: string[];
  /** Why transcription failed, when it did. */
  transcription_error?: string | null;
}

export interface ModelCatalogItem {
  id: string;
  name: string;
  size_desc: string;
  speed_desc: string;
  filename: string;
  downloaded: boolean;
  file_size_bytes: number;
}

export async function fetchTranscriptionLogs(limit = 50): Promise<TranscriptionDiagnosticLog[]> {
  if (!isTauri()) return [];
  try {
    return await invoke<TranscriptionDiagnosticLog[]>("get_transcription_logs", { limit });
  } catch (err) {
    console.error("Failed to fetch transcription logs:", err);
    return [];
  }
}

export async function triggerClearTranscriptionLogs(): Promise<boolean> {
  if (!isTauri()) return true;
  try {
    await invoke("clear_transcription_logs");
    return true;
  } catch (err) {
    console.error("Failed to clear transcription logs:", err);
    return false;
  }
}

export async function fetchAvailableModels(): Promise<ModelCatalogItem[]> {
  if (!isTauri()) return [];
  try {
    return await invoke<ModelCatalogItem[]>("get_available_models");
  } catch (err) {
    console.error("Failed to fetch available models:", err);
    return [];
  }
}

export async function triggerDownloadSpecificModel(modelName: string): Promise<string | null> {
  if (!isTauri()) return null;
  try {
    return await invoke<string>("download_specific_model", { modelName });
  } catch (err) {
    console.error(`Failed to download model ${modelName}:`, err);
    return null;
  }
}

/** Settings saved outside the Settings window, e.g. a microphone picked in the pill or a tray
 * menu choice. */
export async function subscribeToSettingsChanges(
  callback: (settings: BackendSettings) => void,
): Promise<UnlistenFn | null> {
  if (!isTauri()) return null;
  try {
    return await listen<BackendSettings>("settings-changed", (event) => callback(event.payload));
  } catch (err) {
    console.error("Failed to listen to settings changes:", err);
    return null;
  }
}

/** Audio devices plugged or unplugged at runtime. */
export async function subscribeToAudioDeviceChanges(
  callback: (payload: { input_devices: string[]; output_devices: string[] }) => void,
): Promise<UnlistenFn | null> {
  if (!isTauri()) return null;
  try {
    return await listen<{ input_devices: string[]; output_devices: string[] }>(
      "audio-devices-changed",
      (event) => callback(event.payload),
    );
  } catch (err) {
    console.error("Failed to listen to audio device changes:", err);
    return null;
  }
}

/** What an Audio Lab run is doing; `fraction` (0-1) once it is transcribing. */
export type LabProgress = { stage: string; fraction: number | null };

export async function subscribeToLabProgress(
  callback: (payload: LabProgress) => void,
): Promise<UnlistenFn | null> {
  if (!isTauri()) return null;
  try {
    return await listen<LabProgress>("lab-progress", (event) => callback(event.payload));
  } catch (err) {
    console.error("Failed to listen to Audio Lab progress:", err);
    return null;
  }
}

export async function subscribeToDiagnosticLogs(
  callback: (payload: TranscriptionDiagnosticLog) => void,
): Promise<UnlistenFn | null> {
  if (!isTauri()) return null;
  try {
    return await listen<TranscriptionDiagnosticLog>("transcription-diagnostic-log", (event) => {
      callback(event.payload);
    });
  } catch (err) {
    console.error("Failed to listen to diagnostic log events:", err);
    return null;
  }
}

export async function fetchAudioData(filename?: string): Promise<string | null> {
  if (!isTauri()) return null;
  try {
    return await invoke<string>("get_audio_data", { filename: filename || null });
  } catch (err) {
    console.error("Failed to fetch audio data:", err);
    return null;
  }
}

export async function triggerOpenAudioFolder(): Promise<boolean> {
  if (!isTauri()) return false;
  try {
    await invoke("open_audio_folder");
    return true;
  } catch (err) {
    console.error("Failed to open audio folder:", err);
    return false;
  }
}

export interface DiagnosticSegment {
  start_ms: number;
  end_ms: number;
  text: string;
  no_speech_prob: number;
}

export interface LabAudioItem {
  id: string;
  name: string;
  filename: string;
  duration_sec: number;
  size_bytes: number;
  is_vad_trimmed: boolean;
}

export type NoiseReduction = "off" | "light" | "balanced" | "strong";

/** RNNoise strengths, matching Cap's Studio Sound tiers. */
export const NOISE_REDUCTION_OPTIONS: { value: NoiseReduction; label: string }[] = [
  { value: "off", label: "Off" },
  { value: "light", label: "Light" },
  { value: "balanced", label: "Balanced" },
  { value: "strong", label: "Strong" },
];

export interface LabExperimentRequest {
  audio_filename: string;
  model_name: string;
  use_vad: boolean;
  language?: string | null;
  text_normalization?: boolean;
  remove_filler_words?: boolean;
  convert_numbers?: boolean;
  remove_stutters?: boolean;
  apply_self_corrections?: boolean;
  remove_noise_markers?: boolean;
  collapse_redundancy?: boolean;
  annotate_ambiguity?: boolean;
  normalize_structured_values?: boolean;
  noise_reduction?: NoiseReduction;
  /** Same as dictation: long audio is transcribed in chunks cut at pauses. */
  chunking?: boolean;
}

/** Audio Lab's "Translate Again" result. */
export interface LabTranslation {
  text: string;
  ms: number;
  /** Service, model and endpoint that answered. */
  service: string;
}

/** Whether Apple's on-device translation can be used on this Mac (macOS 26+), and why not. */
export interface AppleTranslationInfo {
  available: boolean;
  reason: string | null;
  os_version: string | null;
  min_os: number;
  /** Why the Apple Intelligence provider can't be used; null when it can. */
  intelligence_reason: string | null;
}

export async function fetchAppleTranslationInfo(): Promise<AppleTranslationInfo> {
  if (!isTauri()) {
    return {
      available: false,
      reason: "Works only in the desktop app.",
      os_version: null,
      min_os: 26,
      intelligence_reason: "Works only in the desktop app.",
    };
  }
  return await invoke<AppleTranslationInfo>("apple_translation_info");
}

/** A language Apple translates from, and whether it's downloaded for the current target. */
export interface AppleLanguagePack {
  code: string;
  name: string;
  status: "installed" | "supported" | "unsupported";
}

/** Apple's language packs for translating into `targetLang` (a Bifaria language name), or the
 * Offline provider's with `offline`. Throws when they can't be listed. */
export async function fetchAppleLanguagePacks(
  targetLang: string,
  offline = false,
): Promise<{ target: string; languages: AppleLanguagePack[] }> {
  if (!isTauri()) throw new Error("Works only in the desktop app.");
  const command = offline ? "offline_translation_languages" : "apple_translation_languages";
  return await invoke(command, { targetLang });
}

/** Shows Apple's download prompt for `source` → `target` (codes such as "uk", "en"), or downloads
 * the Offline provider's models with `offline`, and resolves when it's downloaded. Throws when
 * it's cancelled or fails. */
export async function downloadAppleLanguagePack(
  source: string,
  target: string,
  offline = false,
): Promise<void> {
  if (!isTauri()) throw new Error("Works only in the desktop app.");
  const command = offline ? "offline_translation_download" : "apple_translation_download";
  await invoke(command, { source, target });
}

/** Bytes fetched so far while the Offline provider downloads `pair` ("uk>en"). */
export async function subscribeToOfflinePackProgress(
  callback: (p: { pair: string; bytes: number }) => void,
): Promise<UnlistenFn | null> {
  if (!isTauri()) return null;
  return await listen<{ pair: string; bytes: number }>("offline-pack-progress", (e) =>
    callback(e.payload),
  );
}

/** Opens System Settings at Language & Region, where Translation Languages are managed. */
export async function openAppleTranslationSettings(): Promise<void> {
  if (!isTauri()) return;
  await invoke("apple_translation_open_settings");
}

/** Translates text with the chosen service ("LLM", "Custom API", "Offline" or "Apple") without pasting it. Throws
 * the service's error message when it fails. */
export async function triggerLabTranslate(
  text: string,
  provider: string,
  targetLang: string,
): Promise<LabTranslation> {
  if (!isTauri()) throw new Error("Translation works only in the desktop app.");
  return await invoke<LabTranslation>("lab_translate", { text, provider, targetLang });
}

/** Saves an Audio Lab result to a Logs entry and its history row: a transcript of a recording
 * whose transcription failed, or a translation of `source`. Throws when it can't be saved. */
export async function triggerLabSaveResult(
  logId: number,
  kind: "transcription" | "translation",
  text: string,
  source?: string | null,
): Promise<void> {
  if (!isTauri()) throw new Error("Saving works only in the desktop app.");
  await invoke("lab_save_result", { logId, kind, text, source: source ?? null });
}

export interface LabExperimentResult {
  clean_text: string;
  raw_text: string;
  raw_output: string;
  detected_lang: string;
  segments: DiagnosticSegment[];
  original_duration_sec: number;
  processed_duration_sec: number;
  vad_cut_sec: number;
  vad_time_ms: number;
  inference_time_ms: number;
  speed_factor: number;
  hardware_engine: string;
  original_audio_filename: string;
  vad_audio_filename?: string | null;
  post_applied: string[];
  uncertain_spans: string[];
  requires_clarification: boolean;
}

export async function fetchLabAudioFiles(): Promise<LabAudioItem[]> {
  if (!isTauri()) return [];
  try {
    return await invoke<LabAudioItem[]>("list_lab_audio_files");
  } catch (err) {
    console.error("Failed to list lab audio files:", err);
    return [];
  }
}

export async function triggerRunLabExperiment(
  req: LabExperimentRequest,
): Promise<LabExperimentResult | null> {
  if (!isTauri()) return null;
  try {
    return await invoke<LabExperimentResult>("run_lab_experiment", { req });
  } catch (err) {
    console.error("Failed to run lab experiment:", err);
    throw err;
  }
}

/** Saves an audio file for the Audio Lab; `fileName` keeps its extension so its format is known. */
export async function triggerSaveLabCustomAudio(
  base64Wav: string,
  fileName?: string,
): Promise<string | null> {
  if (!isTauri()) return null;
  try {
    return await invoke<string>("save_lab_custom_audio", { base64Wav, fileName });
  } catch (err) {
    console.error("Failed to save custom lab audio:", err);
    return null;
  }
}

/** Whether Bifaria opens when you log in (the system login item, not a saved setting). */
export async function getLaunchAtLogin(): Promise<boolean> {
  if (!isTauri()) return false;
  try {
    return await invoke<boolean>("get_launch_at_login");
  } catch (err) {
    console.error("Failed to read launch at login:", err);
    return false;
  }
}

export async function setLaunchAtLogin(enabled: boolean): Promise<void> {
  if (!isTauri()) return;
  await invoke("set_launch_at_login", { enabled });
}

export async function checkAccessibility(): Promise<boolean> {
  if (!isTauri()) return true;
  try {
    return await invoke<boolean>("check_accessibility");
  } catch (err) {
    console.error("Failed to check accessibility:", err);
    return false;
  }
}

export async function requestAccessibility(): Promise<void> {
  if (!isTauri()) return;
  try {
    await invoke("request_accessibility");
  } catch (err) {
    console.error("Failed to request accessibility:", err);
  }
}

export async function openAccessibilitySettings(): Promise<void> {
  if (!isTauri()) return;
  try {
    await invoke("open_accessibility_settings");
  } catch (err) {
    console.error("Failed to open accessibility settings:", err);
  }
}

export async function openModelsFolder(): Promise<string | null> {
  if (!isTauri()) return null;
  try {
    return await invoke<string>("open_models_folder");
  } catch (err) {
    console.error("Failed to open models folder:", err);
    return null;
  }
}

export async function getModelsDirPath(): Promise<string | null> {
  if (!isTauri()) return null;
  try {
    return await invoke<string>("get_models_dir_path");
  } catch (err) {
    console.error("Failed to get models dir:", err);
    return null;
  }
}

export async function pickAndImportModel(): Promise<ModelCatalogItem | null> {
  if (!isTauri()) return null;
  try {
    return await invoke<ModelCatalogItem>("pick_and_import_model");
  } catch (err) {
    const msg = String(err);
    if (msg.includes("No file selected")) return null;
    console.error("Failed to import model:", err);
    throw err;
  }
}

export async function downloadCustomModel(
  url: string,
  fileName?: string | null,
): Promise<string | null> {
  if (!isTauri()) return null;
  try {
    return await invoke<string>("download_custom_model", { url, fileName: fileName || null });
  } catch (err) {
    console.error("Failed to download custom model:", err);
    throw err;
  }
}

export type UpdateState =
  "idle" | "checking" | "up_to_date" | "available" | "downloading" | "installing" | "error";

export interface UpdateStatus {
  state: UpdateState;
  current_version: string;
  version: string | null;
  percent: number | null;
  message: string | null;
  /** Installed from the Microsoft Store, which delivers updates itself. */
  store_managed?: boolean;
}

export async function fetchUpdateStatus(): Promise<UpdateStatus | null> {
  if (!isTauri()) return null;
  try {
    return await invoke<UpdateStatus>("get_update_status");
  } catch {
    return null;
  }
}

export async function triggerCheckForUpdates(): Promise<UpdateStatus | null> {
  if (!isTauri()) return null;
  try {
    return await invoke<UpdateStatus>("check_for_updates");
  } catch {
    return null;
  }
}

/** Resolves with an error message when the install could not start or failed; the app restarts on success. */
export async function triggerInstallUpdate(): Promise<string | null> {
  if (!isTauri()) return "Updates are only available in the desktop app.";
  try {
    await invoke("install_update");
    return null;
  } catch (err) {
    return typeof err === "string" ? err : String(err);
  }
}

export async function subscribeToUpdateStatus(
  callback: (status: UpdateStatus) => void,
): Promise<UnlistenFn | null> {
  if (!isTauri()) return null;
  try {
    return await listen<UpdateStatus>("update-status", (event) => {
      callback(event.payload);
    });
  } catch (err) {
    console.error("Failed to listen to update status:", err);
    return null;
  }
}
