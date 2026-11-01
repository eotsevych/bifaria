import React, { useEffect, useRef, useState } from "react";
import {
  fetchSettings,
  fitPillHeight,
  subscribeToAudioLevels,
  subscribeToStateChanges,
  triggerCancelRecording,
  triggerStartDragging,
  type AssistantStateEvent,
  type AudioLevels,
  type BackendSettings,
} from "@/lib/tauri";
import { tok, type AccentColor, type Theme } from "@/lib/tokens";
import { useResolvedTheme } from "@/lib/theme";
import { formatDisplay } from "@/lib/hotkey";
import { PillBody } from "./pill/styles";
import { setBarVoiceScale } from "./pill/voiceLevel";
import { TRANSLATE_ACCENT, parseDirection, pillStyleOf, type PillView } from "./pill/view";

const STATE_META: Record<
  "idle" | "listening" | "transcribing" | "translating" | "done" | "error",
  { label: string; sublabel: string }
> = {
  idle: { label: "Ready", sublabel: "Press hotkey to start" },
  listening: { label: "Listening", sublabel: "Capturing audio input" },
  transcribing: { label: "Transcribing", sublabel: "Converting speech to text" },
  translating: { label: "Translating", sublabel: "Processing translation" },
  done: { label: "Copied!", sublabel: "Text in clipboard" },
  error: { label: "Error", sublabel: "Action could not complete" },
};

/** How many recordings show "Press … to finish" after install or after a hotkey changes. */
const FINISH_HINT_RECORDINGS = 3;
const FINISH_HINT_KEY = "bifaria_finish_hint";

/** Whether this recording shows the finish hint; counts it if so. The count restarts whenever
 * either hotkey changes, so a new key is explained a few times too. */
function takeFinishHint(s: BackendSettings): boolean {
  if (s.show_hints === false) return false;
  const keys = `${s.hotkey}|${s.translate_hotkey ?? ""}`;
  try {
    const saved = JSON.parse(localStorage.getItem(FINISH_HINT_KEY) || "null") as {
      keys: string;
      shown: number;
    } | null;
    const shown = saved?.keys === keys ? saved.shown : 0;
    if (shown >= FINISH_HINT_RECORDINGS) return false;
    localStorage.setItem(FINISH_HINT_KEY, JSON.stringify({ keys, shown: shown + 1 }));
    return true;
  } catch {
    return true;
  }
}

function getStateAccent(
  state: "idle" | "listening" | "transcribing" | "translating" | "done" | "error",
  accentColor: string,
  title?: string,
): string {
  if (state === "transcribing") return "#00d4ff";
  if (state === "translating") return "#e58bdb"; // Pink, apart from the violet and orange recordings
  if (state === "done") return "#00e5a0";
  if (state === "error") {
    const lower = (title || "").toLowerCase();
    if (lower.includes("translat")) return "#ff9f43"; // Amber for translation
    if (lower.includes("record")) return "#ff4d6d"; // Crimson for recording
    return "#ff6b6b"; // Coral for transcription
  }
  return accentColor;
}

export function VoicePill() {
  const [state, setState] = useState<AssistantStateEvent>({
    state: "idle",
    title: "Ready",
    subtitle: null,
  });
  const [audioLevels, setAudioLevels] = useState<AudioLevels | undefined>(undefined);
  const [settings, setSettings] = useState<BackendSettings | null>(null);
  const [elapsedSec, setElapsedSec] = useState(0);
  // How long the last recording ran, shown by the smaller styles while it's processed.
  const [heardSec, setHeardSec] = useState<number | null>(null);
  const elapsedRef = useRef(0);
  const [showFinishHint, setShowFinishHint] = useState(false);
  // The backend sends the mode only while listening; keep it so processing stays in its color.
  const [runMode, setRunMode] = useState<AssistantStateEvent["mode"]>(undefined);

  useEffect(() => {
    fetchSettings().then((s) => {
      if (s) setSettings(s);
    });

    const unlistenState = subscribeToStateChanges((payload) => {
      setState(payload);
      if (payload.state === "listening") setRunMode(payload.mode);
      else if (payload.state === "idle") setRunMode(undefined);
    });

    const unlistenAudio = subscribeToAudioLevels((levels) => {
      if (levels?.bands) setAudioLevels(levels);
    });

    // Where the pill is dragged is saved by the backend when it hides (see pill_position.rs).
    const handleKeyDown = (e: KeyboardEvent) => {
      if (e.key === "Escape") {
        e.preventDefault();
        triggerCancelRecording();
      }
    };

    window.addEventListener("keydown", handleKeyDown);

    return () => {
      unlistenState.then((fn) => fn?.());
      unlistenAudio.then((fn) => fn?.());
      window.removeEventListener("keydown", handleKeyDown);
    };
  }, []);

  useEffect(() => {
    let timer: ReturnType<typeof setInterval> | null = null;
    if (state.state === "listening") {
      setElapsedSec(0);
      elapsedRef.current = 0;
      setHeardSec(null);
      setShowFinishHint(false);
      fetchSettings().then((s) => {
        if (!s) return;
        setSettings(s);
        setShowFinishHint(takeFinishHint(s));
      });
      const start = Date.now();
      timer = setInterval(() => {
        elapsedRef.current = Math.floor((Date.now() - start) / 1000);
        setElapsedSec(elapsedRef.current);
      }, 200);
    } else {
      if (state.state === "idle") setHeardSec(null);
      else if (elapsedRef.current > 0) setHeardSec(elapsedRef.current);
      elapsedRef.current = 0;
      setElapsedSec(0);
    }
    return () => {
      if (timer) clearInterval(timer);
    };
  }, [state.state]);

  const handleMouseDown = async (e: React.MouseEvent) => {
    if (e.button === 0 && !(e.target as HTMLElement).closest("button, select")) {
      await triggerStartDragging();
    }
  };

  useEffect(() => setBarVoiceScale(settings?.bar_scale), [settings?.bar_scale]);

  const themeMode: Theme = useResolvedTheme(settings?.theme);
  const accentKey: AccentColor =
    (localStorage.getItem("bifaria_accent") as AccentColor) || "violet";
  const t = tok(themeMode, accentKey);
  const micWarning = state.state === "listening" && state.warning === true;
  // A recording started with the translate hotkey stays orange until it's done; automatic
  // translation only gets an outlined badge, since it may not translate at all.
  const isTranslateMode = runMode === "translate";
  const isAutoMode = runMode === "auto";
  const inRun = ["listening", "transcribing", "translating"].includes(state.state);
  const inTranslateRun = isTranslateMode && inRun;
  const accent = micWarning
    ? "#ffd166"
    : inTranslateRun
      ? TRANSLATE_ACCENT
      : getStateAccent(state.state, t.accent, state.title);

  const targetLang = settings?.target_lang || "English";
  const isNoTranslation =
    settings?.translation_mode === "off" ||
    targetLang.toLowerCase().includes("no translation") ||
    targetLang.toLowerCase() === "none";
  const spokenOnly =
    settings?.source_lang && settings.source_lang !== "Auto" ? settings.source_lang : null;

  const meta = STATE_META[state.state];
  // The backend says what this recording does with the speech (see AssistantStateEvent.mode).
  const isTranscribeOnly = runMode === "transcribe";
  const startedWith = isTranslateMode ? settings?.translate_hotkey : settings?.hotkey;
  const finishHint =
    state.state === "listening" && showFinishHint && startedWith
      ? `Press ${formatDisplay(startedWith).replace(/ \(.*\)/, "")} to finish`
      : null;
  // While listening, say what will happen to the speech.
  const listeningIntent = isTranslateMode
    ? `Translating to ${targetLang}`
    : isNoTranslation || isTranscribeOnly
      ? `In ${spokenOnly ?? "your language"}`
      : // The AUTO→EN badge already says translation is on.
        spokenOnly
        ? `${spokenOnly} → ${targetLang}`
        : `Translates if not ${targetLang}`;
  const dynamicSublabel =
    state.subtitle || finishHint || (state.state === "listening" ? listeningIntent : meta.sublabel);

  const displayedSubtitle =
    state.state === "error" && state.text ? `Copied: "${state.text}"` : dynamicSublabel;

  // Errors and mic warnings that explain what to fix are shown in full, in a pill that grows to fit them.
  const isLong = (state.state === "error" || state.state === "listening") && state.long === true;

  // The long-message window starts at a fixed size; grow it to whatever the text needs.
  const pillRef = useRef<HTMLDivElement>(null);
  useEffect(() => {
    const el = pillRef.current;
    if (!isLong || !el) return;
    let sent = 0;
    const fit = () => {
      // The page's padding (p-2) on both sides, plus room for the pill's shadow.
      const needed = Math.ceil(el.getBoundingClientRect().height) + 16 + 12;
      if (needed !== sent) {
        sent = needed;
        void fitPillHeight(needed);
      }
    };
    const observer = new ResizeObserver(fit);
    observer.observe(el);
    fit();
    return () => observer.disconnect();
    // Every new message refits: showing the pill again resets the window to its base size even
    // when the content (and so its observed size) is unchanged.
  }, [isLong, state]);

  if (state.state === "idle") {
    return null;
  }

  const direction = state.state === "translating" ? parseDirection(state.subtitle) : null;
  const view: PillView = {
    state: state.state,
    accent,
    title: state.title && (state.state !== "listening" || micWarning) ? state.title : meta.label,
    rawTitle: state.title,
    subtitle: displayedSubtitle,
    elapsedSec,
    heardSec,
    run: inRun && (isTranslateMode || isAutoMode) ? (isTranslateMode ? "translate" : "auto") : null,
    targetLang: direction?.[1] ?? targetLang,
    sourceLang: direction?.[0] ?? spokenOnly,
    micWarning,
    micDown: state.mic,
    long: isLong,
    retry: state.state === "error" && state.retry === true,
    devices: state.state === "error" ? state.devices : undefined,
  };

  return (
    <main
      data-tauri-drag-region
      onMouseDown={handleMouseDown}
      style={{ WebkitAppRegion: "drag" } as React.CSSProperties}
      className="w-full h-full min-h-screen flex items-center justify-center p-2 bg-transparent select-none overflow-hidden cursor-grab active:cursor-grabbing"
    >
      <div ref={pillRef} className="flex w-full justify-center">
        <PillBody
          style={pillStyleOf(settings?.pill_style)}
          view={view}
          t={t}
          levels={audioLevels}
        />
      </div>
    </main>
  );
}

export default VoicePill;
