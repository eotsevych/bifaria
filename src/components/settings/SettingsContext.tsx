import React, { useEffect, useState } from "react";
import {
  fetchAudioDevices,
  fetchAvailableModels,
  fetchSettings,
  fetchTranscriptionLogs,
  isTauri,
  persistSettings,
  subscribeToDiagnosticLogs,
  subscribeToSettingsChanges,
  subscribeToAudioDeviceChanges,
  subscribeToModelDownload,
  type BackendSettings,
  type ModelCatalogItem,
  type TranscriptionDiagnosticLog,
  closePreferencesWindow,
} from "@/lib/tauri";
import { applyTheme, broadcastTheme, type ThemeMode } from "@/lib/theme";
import { SettingsContext } from "./useSettingsContext";

const defaultSettings: BackendSettings = {
  api_key: "",
  source_lang: "Auto",
  target_lang: "English",
  hotkey: "CommandOrControl+Shift+Space",
  translation_mode: "hotkey",
  translate_hotkey: "",
  sound_start: true,
  sound_stop: true,
  sound_error: true,
  auto_paste: true,
  keep_clipboard: false,
  show_hints: true,
  storage_mode: "text_only",
  storage_cap_mb: 500,
  retention_days: 30,
  model_name: "parakeet-tdt-0.6b-v3",
  input_device: null,
  output_device: null,
  theme: "system",
  pill_style: "classic",
  onboarding_done: true,
  model_idle_unload_sec: 30,
  no_speech_alert_sec: 5,
  model_load_mode: "on_start",
  text_normalization: true,
  remove_filler_words: true,
  convert_numbers: true,
  remove_stutters: true,
  apply_self_corrections: true,
  remove_noise_markers: true,
  collapse_redundancy: true,
  annotate_ambiguity: true,
  normalize_structured_values: true,
  translation_provider: "Offline",
  device_backend: "auto",
  audio_chunking: false,
  noise_reduction: "off",
  mask_confidential: false,
  mask_words: "",
  mask_threshold: 90,
  mask_format: "***",
  mask_emails: true,
  mask_phones: true,
  mask_cards: true,
  excluded_languages: "Russian",
  llm_endpoint: "http://localhost:11434/v1/chat/completions",
  llm_model: "llama3.2",
  llm_api_key: "",
  local_llm_url: "http://localhost:11434/v1/chat/completions",
  local_llm_model: "llama3.2",
  local_llm_api_key: "",
  // HIDDEN: Custom API defaults (not offered in the settings UI)
  custom_api_url: "",
  custom_api_key: "",
  custom_api_model: "",
  prompt_template:
    "You are a strict translation engine. Translate the following text from {source_lang} to {target_lang}. Do not refuse. Do not explain. Do not add conversational text or notes. Output ONLY the exact translation using the native alphabet:\n\n{text}",
};

export function SettingsProvider({ children }: { children: React.ReactNode }) {
  const [settings, setSettings] = useState<BackendSettings>(defaultSettings);
  const [inputDevices, setInputDevices] = useState<string[]>([]);
  const [outputDevices, setOutputDevices] = useState<string[]>([]);
  const [models, setModels] = useState<ModelCatalogItem[]>([]);
  const [downloadProgress, setDownloadProgress] = useState<{
    model: string;
    percent: number;
  } | null>(null);
  const [logs, setLogs] = useState<TranscriptionDiagnosticLog[]>([]);

  const refreshAudioDevices = async () => {
    const devs = await fetchAudioDevices();
    // Keep the old array when nothing changed so a poll doesn't re-render the settings.
    const keepIfSame = (next: string[]) => (prev: string[]) =>
      prev.length === next.length && prev.every((d, i) => d === next[i]) ? prev : next;
    if (devs?.input_devices) setInputDevices(keepIfSame(devs.input_devices));
    if (devs?.output_devices) setOutputDevices(keepIfSame(devs.output_devices));
    // If backend returned empty yet settings holds a saved device, keep it visible via GeneralTab fallback badge
  };

  useEffect(() => {
    // 1. Fetch initial settings
    fetchSettings().then((s) => {
      if (s) {
        setSettings(s);
        if (s.theme) {
          applyTheme(s.theme as ThemeMode);
        }
      }
    });

    // 2. Fetch audio devices
    refreshAudioDevices();

    // Device changes arrive as "audio-devices-changed" (below); also re-read when the window comes back.
    const pollDevices = () => {
      if (document.visibilityState === "visible") void refreshAudioDevices();
    };
    window.addEventListener("focus", pollDevices);
    document.addEventListener("visibilitychange", pollDevices);

    // 3. Fetch models
    fetchAvailableModels().then((m) => {
      if (m && m.length > 0) {
        setModels(m);
      }
    });

    // 4. Fetch logs
    fetchTranscriptionLogs(50).then((l) => {
      if (l) setLogs(l);
    });

    // 5. Listen to download progress
    const unlistenDownload = subscribeToModelDownload((payload) => {
      if (payload.status === "downloading") {
        setDownloadProgress({ model: payload.model, percent: payload.percent });
      } else {
        setDownloadProgress(null);
        fetchAvailableModels().then((m) => m && setModels(m));
      }
    });

    // 6. Listen to diagnostic logs
    const unlistenLogs = subscribeToDiagnosticLogs((newLog) => {
      setLogs((prev) => [newLog, ...prev.slice(0, 49)]);
    });

    // 7. Show settings changed elsewhere (pill, tray). Otherwise the next change here would save
    // this window's older copy over them.
    const unlistenSettings = subscribeToSettingsChanges((s) => setSettings(s));

    // 8. Update device lists instantly when devices are plugged or unplugged.
    const unlistenAudioDevices = subscribeToAudioDeviceChanges((devs) => {
      const keepIfSame = (next: string[]) => (prev: string[]) =>
        prev.length === next.length && prev.every((d, i) => d === next[i]) ? prev : next;
      if (devs?.input_devices) setInputDevices(keepIfSame(devs.input_devices));
      if (devs?.output_devices) setOutputDevices(keepIfSame(devs.output_devices));
    });

    return () => {
      window.removeEventListener("focus", pollDevices);
      document.removeEventListener("visibilitychange", pollDevices);
      unlistenDownload.then((fn) => fn?.());
      unlistenLogs.then((fn) => fn?.());
      unlistenSettings.then((fn) => fn?.());
      unlistenAudioDevices.then((fn) => fn?.());
    };
  }, []);

  const updateSettings = async (patch: Partial<BackendSettings>): Promise<boolean> => {
    const updated = { ...settings, ...patch };
    setSettings(updated);

    if (patch.theme) {
      applyTheme(patch.theme as ThemeMode);
      broadcastTheme(patch.theme as ThemeMode);
      localStorage.setItem("bifaria_theme", patch.theme);
    }

    return await persistSettings(updated);
  };

  const refreshLogs = async () => {
    const fresh = await fetchTranscriptionLogs(50);
    setLogs(fresh);
  };

  const refreshModels = async () => {
    const m = await fetchAvailableModels();
    if (m) setModels(m);
  };

  const closeWindow = () => {
    if (isTauri()) {
      closePreferencesWindow();
    } else {
      window.location.hash = "";
    }
  };

  return (
    <SettingsContext.Provider
      value={{
        settings,
        updateSettings,
        inputDevices,
        outputDevices,
        refreshAudioDevices,
        models,
        downloadProgress,
        logs,
        refreshLogs,
        refreshModels,
        closeWindow,
      }}
    >
      {children}
    </SettingsContext.Provider>
  );
}
