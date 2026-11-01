import React, { useEffect, useRef, useState } from "react";
import { useResolvedTheme } from "@/lib/theme";
import { createHotkeyCapture, formatDisplay } from "@/lib/hotkey";
import { SettingsProvider } from "@/components/settings/SettingsContext";
import { useSettingsContext } from "@/components/settings/useSettingsContext";
import { PRESETS, STYLES, presetOf, type Preset } from "@/lib/textStyles";
import {
  checkAccessibility,
  finishOnboarding,
  openAccessibilitySettings,
  requestAccessibility,
  setPracticeMode,
  startMicTest,
  stopMicTest,
  subscribeToAudioLevels,
  subscribeToMicTestLevels,
  subscribeToModelDownload,
  subscribeToPracticeHotkey,
  subscribeToStateChanges,
  triggerDownloadSpecificModel,
  triggerToggleRecording,
} from "@/lib/tauri";

/** Hotkeys and pasting need the Accessibility permission only on macOS. */
const IS_MAC = typeof navigator !== "undefined" && /Mac/i.test(navigator.userAgent);

type StepId = "welcome" | "mic" | "access" | "keys" | "lang" | "style" | "try";

const STEPS: StepId[] = IS_MAC
  ? ["welcome", "mic", "access", "keys", "lang", "style", "try"]
  : ["welcome", "mic", "keys", "lang", "style", "try"];

/** Animation clock of the stage illustrations, in ms per tick (as in the design). */
const TICK_MS = 80;
/** Mic test level (0–1) that shows the microphone delivers sound, so access was allowed. */
const MIC_ALIVE_LEVEL = 0.01;

const MONO = "'JetBrains Mono', ui-monospace, monospace";
const SANS = "Inter, system-ui, sans-serif";

// ── Palette (from the Onboarding Modal design; light is derived from it) ───

const DARK = {
  card: "#14141f",
  cardBorder: "rgba(255,255,255,.08)",
  stage: "#0f0f19",
  stageBorder: "rgba(255,255,255,.05)",
  row: "#181828",
  rowBorder: "rgba(255,255,255,.07)",
  key: "#22223a",
  keyBorder: "rgba(255,255,255,.1)",
  text: "#e8e8f0",
  body: "#9a9ab0",
  dim: "#8a8aa2",
  faint: "#6b6b80",
  accent: "#a594ff",
  accentSoft: "rgba(165,148,255,.14)",
  accentSoftBorder: "rgba(165,148,255,.3)",
  accentText: "#c4b8ff",
  onAccent: "#0b0b12",
  orange: "#ff8a4c",
  orangeSoft: "rgba(255,138,76,.14)",
  orangeBorder: "rgba(255,138,76,.5)",
  cyan: "#22d3ee",
  green: "#34d399",
  greenBorder: "rgba(52,211,153,.4)",
  red: "#f87171",
  segOff: "rgba(255,255,255,.08)",
  ghostBorder: "rgba(255,255,255,.1)",
  switchOff: "rgba(255,255,255,.14)",
  dashed: "rgba(255,255,255,.18)",
  pillShadow: "0 6px 20px rgba(0,0,0,.35)",
  barGradient: "linear-gradient(180deg,#a593ff,#7c6fff)",
};

type Palette = typeof DARK;

const LIGHT: Palette = {
  card: "#ffffff",
  cardBorder: "rgba(0,0,0,.08)",
  stage: "#f5f5fa",
  stageBorder: "rgba(0,0,0,.05)",
  row: "#ffffff",
  rowBorder: "rgba(0,0,0,.08)",
  key: "#f0f0f6",
  keyBorder: "rgba(0,0,0,.12)",
  text: "#1a1a24",
  body: "#5c5c70",
  dim: "#6e6e84",
  faint: "#9a9aae",
  accent: "#7c6af0",
  accentSoft: "rgba(124,106,240,.12)",
  accentSoftBorder: "rgba(124,106,240,.3)",
  accentText: "#5b4bd6",
  onAccent: "#ffffff",
  orange: "#ea580c",
  orangeSoft: "rgba(234,88,12,.1)",
  orangeBorder: "rgba(234,88,12,.45)",
  cyan: "#0891b2",
  green: "#059669",
  greenBorder: "rgba(5,150,105,.4)",
  red: "#dc2626",
  segOff: "rgba(0,0,0,.08)",
  ghostBorder: "rgba(0,0,0,.12)",
  switchOff: "rgba(0,0,0,.15)",
  dashed: "rgba(0,0,0,.2)",
  pillShadow: "0 6px 20px rgba(0,0,0,.08)",
  barGradient: "linear-gradient(180deg,#a593ff,#7c6fff)",
};

const LANGUAGES: [code: string, name: string][] = [
  ["EN", "English"],
  ["UK", "Ukrainian"],
  ["DE", "German"],
  ["ES", "Spanish"],
  ["FR", "French"],
  ["PL", "Polish"],
];

function langCode(name: string): string {
  return LANGUAGES.find(([, n]) => n === name)?.[0] ?? name.slice(0, 2).toUpperCase();
}

/** Keycaps for a hotkey: "⌘ + Shift + Space" → ["⌘", "Shift", "Space"]. */
function keycaps(raw: string): string[] {
  return formatDisplay(raw)
    .replace(/ \(.*\)/, "")
    .split(" + ")
    .filter(Boolean);
}

/** A hotkey's name for running text, kept on one line ("Right ⌥", "Ctrl + Shift + Space"). */
function hotkeyName(raw: string): string {
  return (formatDisplay(raw).replace(/ \(.*\)/, "") || "your hotkey").replace(/ /g, "\u00a0");
}

// ── Model download ──────────────────────────────────────────────────────────

/** The speech model download, which starts on first launch and runs while the guide is open. */
interface ModelDownload {
  phase: "checking" | "ready" | "downloading" | "starting" | "idle" | "failed";
  percent: number;
  downloaded: number;
  total: number;
  error: string | null;
  start: () => void;
}

/** How long to show "Starting…" before offering a Download button (the first launch starts it). */
const START_GRACE_MS = 4000;

function useModelDownload(): ModelDownload {
  const { settings, models, refreshModels } = useSettingsContext();
  const [progress, setProgress] = useState<{
    percent: number;
    downloaded: number;
    total: number;
  } | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [graceOver, setGraceOver] = useState(false);
  const model = models.find((m) => m.filename === settings.model_name);

  useEffect(() => {
    const timer = setTimeout(() => setGraceOver(true), START_GRACE_MS);
    const unlisten = subscribeToModelDownload((p) => {
      if (p.status === "downloading") {
        setError(null);
        setProgress({
          percent: p.percent,
          downloaded: p.downloaded_bytes ?? 0,
          total: p.total_bytes ?? 0,
        });
      } else {
        setProgress(null);
        setError(p.status === "error" ? p.error || "The download failed." : null);
      }
    });
    return () => {
      clearTimeout(timer);
      unlisten.then((fn) => fn?.());
    };
  }, []);

  const start = async () => {
    setError(null);
    setGraceOver(false);
    const path = await triggerDownloadSpecificModel(settings.model_name);
    if (!path) setError((e) => e ?? "The download failed. Check your internet connection.");
    setGraceOver(true);
    refreshModels();
  };

  const phase: ModelDownload["phase"] = error
    ? "failed"
    : progress
      ? "downloading"
      : models.length === 0
        ? "checking"
        : model?.downloaded
          ? "ready"
          : graceOver
            ? "idle"
            : "starting";

  return {
    phase,
    percent: progress?.percent ?? 0,
    downloaded: progress?.downloaded ?? 0,
    total: progress?.total ?? 0,
    error,
    start: () => void start(),
  };
}

function formatBytes(bytes: number): string {
  const mb = bytes / 1_000_000;
  return mb >= 1000 ? `${(mb / 1000).toFixed(1)} GB` : `${Math.round(mb)} MB`;
}

/** Download progress in the header, on every step. */
function ModelStatus({ dl, p }: { dl: ModelDownload; p: Palette }) {
  const actionable = dl.phase === "idle" || dl.phase === "failed";
  const label =
    dl.phase === "ready"
      ? "✓ SPEECH MODEL READY"
      : dl.phase === "failed"
        ? "MODEL FAILED · RETRY"
        : dl.phase === "idle"
          ? "GET SPEECH MODEL"
          : dl.phase === "checking"
            ? "SPEECH MODEL"
            : `SPEECH MODEL ${dl.percent}%`;
  const color = dl.phase === "ready" ? p.green : dl.phase === "failed" ? p.red : p.dim;
  const title =
    dl.phase === "downloading" && dl.total > 0
      ? `Downloading the speech model: ${formatBytes(dl.downloaded)} of ${formatBytes(dl.total)}`
      : dl.phase === "failed"
        ? (dl.error ?? undefined)
        : dl.phase === "ready"
          ? "The speech model is downloaded"
          : "The speech model downloads in the background while you set up";
  const showBar = dl.phase === "downloading" || dl.phase === "starting";
  return (
    <button
      type="button"
      onClick={actionable ? dl.start : undefined}
      title={title}
      style={{
        display: "flex",
        alignItems: "center",
        gap: 8,
        background: "none",
        border: "none",
        padding: "4px 2px",
        cursor: actionable ? "pointer" : "default",
        font: `500 11px ${MONO}`,
        color,
        whiteSpace: "nowrap",
      }}
    >
      {showBar && (
        <span
          style={{
            width: 44,
            height: 3,
            borderRadius: 2,
            background: p.segOff,
            overflow: "hidden",
            display: "block",
          }}
        >
          <span
            style={{
              display: "block",
              height: "100%",
              width: `${dl.percent}%`,
              background: p.accent,
              transition: "width 400ms",
            }}
          />
        </span>
      )}
      {label}
    </button>
  );
}

// ── Stage illustrations (ported from the design) ───────────────────────────

/** Voice bars: driven by a real level (0–1) when given, otherwise a made-up voice. */
function Bars({
  t,
  p,
  silent,
  level,
  n = 9,
  max = 20,
}: {
  t: number;
  p: Palette;
  silent?: boolean;
  level?: number | undefined;
  n?: number;
  max?: number;
}) {
  return (
    <div style={{ display: "flex", alignItems: "center", gap: 3, height: 20 }}>
      {Array.from({ length: n }, (_, i) => {
        const wobble = Math.abs(Math.sin(t * 0.35 + i * 1.3) * Math.cos(t * 0.13 + i * 0.7));
        const v = silent
          ? 0
          : level !== undefined
            ? Math.min(1, level * 2.4) * (0.55 + 0.45 * wobble)
            : wobble;
        const env = 1 - Math.abs(i - (n - 1) / 2) / n;
        return (
          <div
            key={i}
            style={{
              width: 3,
              height: 3 + (max - 3) * v * env,
              borderRadius: 3,
              background: p.barGradient,
              transition: "height 80ms linear",
            }}
          />
        );
      })}
    </div>
  );
}

function Scan({ t, color }: { t: number; color: string }) {
  const n = 9;
  const pos = ((t * 0.45) % (n + 4)) - 2;
  return (
    <div style={{ display: "flex", alignItems: "center", gap: 3, height: 20 }}>
      {Array.from({ length: n }, (_, i) => {
        const k = Math.max(0, 1 - Math.abs(i - pos) / 1.6);
        return (
          <div
            key={i}
            style={{
              width: 3,
              height: 3 + 11 * k,
              borderRadius: 3,
              background: color,
              opacity: 0.35 + 0.65 * k,
              transition: "height 80ms linear",
            }}
          />
        );
      })}
    </div>
  );
}

function Orbit({ t, p }: { t: number; p: Palette }) {
  const a = t * 0.22;
  const R = 14;
  const d = 7;
  const dot = (ang: number, color: string) => (
    <div
      style={{
        position: "absolute",
        left: "50%",
        top: "50%",
        width: d,
        height: d,
        marginLeft: -d / 2,
        marginTop: -d / 2,
        borderRadius: "50%",
        background: color,
        transform: `translate(${Math.cos(ang) * R}px,${Math.sin(ang) * R * 0.45}px)`,
        opacity: 0.55 + 0.45 * Math.sin(ang),
      }}
    />
  );
  return (
    <div style={{ position: "relative", width: R * 2 + d, height: 20 }}>
      {dot(a, p.cyan)}
      {dot(a + Math.PI, p.orange)}
    </div>
  );
}

function CopyIcon({ color }: { color: string }) {
  return (
    <svg width={20} height={20} viewBox="0 0 20 20" fill="none">
      <rect
        x={7}
        y={2.5}
        width={10.5}
        height={10.5}
        rx={2.5}
        stroke={color}
        strokeWidth={1.6}
        opacity={0.55}
      />
      <rect x={2.5} y={7} width={10.5} height={10.5} rx={2.5} stroke={color} strokeWidth={1.6} />
      <path
        d="M5.3 12.4l1.7 1.6 3.2-3.4"
        stroke={color}
        strokeWidth={1.6}
        strokeLinecap="round"
        strokeLinejoin="round"
      />
    </svg>
  );
}

function XIcon({ color }: { color: string }) {
  return (
    <svg width={20} height={20} viewBox="0 0 20 20" fill="none">
      <circle cx={10} cy={10} r={8.5} stroke={color} strokeWidth={1.6} />
      <path d="M6.5 6.5l7 7M13.5 6.5l-7 7" stroke={color} strokeWidth={1.8} strokeLinecap="round" />
    </svg>
  );
}

function MicIcon({ color }: { color: string }) {
  return (
    <svg
      width={28}
      height={28}
      viewBox="0 0 24 24"
      fill="none"
      stroke={color}
      strokeWidth={1.7}
      strokeLinecap="round"
    >
      <rect x={9} y={3} width={6} height={11} rx={3} />
      <path d="M5.5 11a6.5 6.5 0 0013 0M12 17.5V21" />
    </svg>
  );
}

type MorphKind = "silent" | "bars" | "scan" | "orbit" | "copy" | "x";

/** The small pill of the design, showing one stage of a recording. */
function MiniPill({
  kind,
  t,
  p,
  level,
  slot,
  opacity = 1,
}: {
  kind: MorphKind;
  t: number;
  p: Palette;
  level?: number | undefined;
  slot?: string | null;
  opacity?: number;
}) {
  return (
    <div
      style={{
        display: "flex",
        alignItems: "center",
        justifyContent: "center",
        gap: 8,
        minWidth: 124,
        padding: "0 14px",
        boxSizing: "border-box",
        height: 36,
        borderRadius: 18,
        background: p.row,
        border: `1px solid ${p.rowBorder}`,
        boxShadow: p.pillShadow,
        whiteSpace: "nowrap",
        opacity,
        transition: "opacity 200ms",
      }}
    >
      {kind === "silent" && <Bars t={t} p={p} silent />}
      {kind === "bars" && <Bars t={t} p={p} level={level} />}
      {kind === "scan" && <Scan t={t} color={p.cyan} />}
      {kind === "orbit" && <Orbit t={t} p={p} />}
      {kind === "copy" && <CopyIcon color={p.green} />}
      {kind === "x" && <XIcon color={p.red} />}
      {slot && <span style={{ font: `600 11px ${MONO}`, color: p.orange }}>{slot}</span>}
    </div>
  );
}

function Row({ p, children }: { p: Palette; children: React.ReactNode }) {
  return (
    <div
      style={{
        display: "flex",
        alignItems: "center",
        gap: 10,
        width: "100%",
        padding: "10px 12px",
        borderRadius: 10,
        background: p.row,
        border: `1px solid ${p.rowBorder}`,
        boxSizing: "border-box",
      }}
    >
      {children}
    </div>
  );
}

const AppMark = () => (
  <div
    style={{
      width: 22,
      height: 22,
      borderRadius: 7,
      background: "linear-gradient(135deg,#6a5cff,#ff7aa8,#ff8a3d)",
      flexShrink: 0,
    }}
  />
);

// ── Steps: each has a stage illustration plus a title and body ─────────────

/** The welcome demo: one recording on a loop, then a translated one. */
const DEMO_FLOW: [MorphKind, number, string][] = [
  ["silent", 12, "LISTENING"],
  ["bars", 36, "LISTENING"],
  ["scan", 20, "TRANSCRIBING"],
  ["copy", 18, "COPIED"],
  ["orbit", 22, "TRANSLATING"],
  ["copy", 16, "COPIED"],
];
const DEMO_TOTAL = DEMO_FLOW.reduce((sum, f) => sum + f[1], 0);

function WelcomeStage({ t, p, target }: { t: number; p: Palette; target: string }) {
  let rest = t % DEMO_TOTAL;
  let cur = DEMO_FLOW[0]!;
  for (const f of DEMO_FLOW) {
    if (rest < f[1]) {
      cur = f;
      break;
    }
    rest -= f[1];
  }
  const translating = cur[2] === "TRANSLATING";
  const source = target === "UK" ? "EN" : "UK";
  return (
    <div style={{ display: "flex", flexDirection: "column", alignItems: "center", gap: 14 }}>
      <MiniPill kind={cur[0]} t={t} p={p} slot={translating ? `${source} → ${target}` : null} />
      <span style={{ font: `500 11px ${MONO}`, letterSpacing: ".08em", color: p.dim }}>
        {cur[2]}
      </span>
    </div>
  );
}

type MicState = "off" | "waiting" | "allowed";

function MicStage({
  p,
  micState,
  level,
  onAllow,
}: {
  p: Palette;
  micState: MicState;
  level: number;
  onAllow: () => void;
}) {
  const { settings, updateSettings, inputDevices } = useSettingsContext();
  const ring = micState === "off" ? 0 : Math.min(1, level * 3);
  return (
    <div
      style={{
        display: "flex",
        flexDirection: "column",
        alignItems: "center",
        gap: 14,
        width: "100%",
        maxWidth: 320,
      }}
    >
      <div
        style={{
          width: 64,
          height: 64,
          borderRadius: 32,
          background: p.accentSoft,
          display: "flex",
          alignItems: "center",
          justifyContent: "center",
          boxShadow: `0 0 0 ${Math.round(ring * 10)}px ${p.accentSoft}`,
          transition: "box-shadow 80ms linear",
        }}
      >
        <MicIcon color={p.accent} />
      </div>
      <Row p={p}>
        <span style={{ fontSize: 13, flex: 1 }}>Microphone</span>
        {micState === "allowed" ? (
          <span style={{ font: `600 11px ${MONO}`, color: p.green }}>ALLOWED</span>
        ) : micState === "waiting" ? (
          <span style={{ font: `600 11px ${MONO}`, color: p.dim }}>SAY SOMETHING…</span>
        ) : (
          <button
            type="button"
            onClick={onAllow}
            style={{
              background: p.accent,
              color: p.onAccent,
              border: "none",
              borderRadius: 8,
              padding: "7px 12px",
              font: `600 12px ${SANS}`,
              cursor: "pointer",
            }}
          >
            Allow
          </button>
        )}
      </Row>
      <select
        value={settings.input_device ?? ""}
        onChange={(e) => updateSettings({ input_device: e.target.value || null })}
        style={{
          maxWidth: "100%",
          background: "none",
          border: "none",
          color: p.dim,
          font: `500 11px ${MONO}`,
          cursor: "pointer",
          textAlign: "center",
        }}
      >
        <option value="">INPUT: SYSTEM DEFAULT</option>
        {inputDevices.map((d) => (
          <option key={d} value={d}>
            INPUT: {d.toUpperCase()}
          </option>
        ))}
      </select>
    </div>
  );
}

function AccessStage({ p, granted }: { p: Palette; granted: boolean | null }) {
  const on = granted === true;
  return (
    <div style={{ display: "flex", flexDirection: "column", gap: 8, width: "100%", maxWidth: 340 }}>
      <span style={{ font: `600 10.5px ${MONO}`, letterSpacing: ".08em", color: p.dim }}>
        SYSTEM SETTINGS › ACCESSIBILITY
      </span>
      <Row p={p}>
        <AppMark />
        <span style={{ fontSize: 13, flex: 1 }}>Bifaria</span>
        <div
          role="switch"
          aria-checked={on}
          onClick={() => (on ? openAccessibilitySettings() : requestAccessibility())}
          style={{
            width: 36,
            height: 20,
            borderRadius: 10,
            background: on ? p.green : p.switchOff,
            position: "relative",
            cursor: "pointer",
            transition: "background 160ms",
          }}
        >
          <div
            style={{
              position: "absolute",
              top: 2,
              left: on ? 18 : 2,
              width: 16,
              height: 16,
              borderRadius: 8,
              background: "#ffffff",
              transition: "left 160ms",
            }}
          />
        </div>
      </Row>
      <span style={{ fontSize: 12, color: p.dim, lineHeight: 1.5 }}>
        {on ? (
          "Done. Bifaria can paste for you."
        ) : (
          <>
            Click the switch, then turn Bifaria on in{" "}
            <a
              href="#"
              onClick={(e) => {
                e.preventDefault();
                openAccessibilitySettings();
              }}
              style={{ color: p.accent }}
            >
              System Settings
            </a>
            . This page updates by itself.
          </>
        )}
      </span>
    </div>
  );
}

function Keycap({ p, children }: { p: Palette; children: React.ReactNode }) {
  return (
    <span
      style={{
        minWidth: 24,
        height: 26,
        padding: "0 7px",
        boxSizing: "border-box",
        display: "flex",
        alignItems: "center",
        justifyContent: "center",
        borderRadius: 6,
        background: p.key,
        border: `1px solid ${p.keyBorder}`,
        borderBottomWidth: 2,
        font: `600 12px ${MONO}`,
        whiteSpace: "nowrap",
      }}
    >
      {children}
    </span>
  );
}

/** Presets offered while changing a hotkey (Right ⌥ / ⌃ need the macOS event tap). */
const HOTKEY_PRESETS: { value: string; label: string }[] = [
  ...(IS_MAC
    ? [
        { value: "RightOption", label: "Right ⌥" },
        { value: "RightControl", label: "Right ⌃" },
      ]
    : []),
  { value: "CommandOrControl+Shift+Space", label: IS_MAC ? "⌘ ⇧ Space" : "Ctrl ⇧ Space" },
];

function Chip({
  p,
  on,
  onClick,
  children,
}: {
  p: Palette;
  on: boolean;
  onClick: () => void;
  children: React.ReactNode;
}) {
  return (
    <button
      type="button"
      onClick={(e) => {
        e.stopPropagation();
        onClick();
      }}
      style={{
        padding: "2px 7px",
        borderRadius: 6,
        background: on ? p.accentSoft : "transparent",
        border: `1px solid ${on ? p.accentSoftBorder : p.rowBorder}`,
        color: on ? p.accentText : p.dim,
        font: `500 11px ${MONO}`,
        cursor: "pointer",
        whiteSpace: "nowrap",
      }}
    >
      {children}
    </button>
  );
}

function KeysStage({ p }: { p: Palette }) {
  const { settings, updateSettings } = useSettingsContext();
  const [editing, setEditing] = useState<"main" | "translate" | null>(null);
  const mode = settings.translation_mode ?? "hotkey";
  const target = langCode(settings.target_lang || "English");
  const translateKey = settings.translate_hotkey ?? "";
  const [conflict, setConflict] = useState(false);

  const save = (which: "main" | "translate", v: string) => {
    if (which === "main") {
      // A translate hotkey on the same key would never fire; turn it off.
      updateSettings(v && v === translateKey ? { hotkey: v, translate_hotkey: "" } : { hotkey: v });
    } else if (v && v === settings.hotkey) {
      setConflict(true);
      return;
    } else {
      updateSettings({ translate_hotkey: v });
    }
    setConflict(false);
    setEditing(null);
  };
  const saveRef = useRef(save);
  saveRef.current = save;

  // While changing a hotkey: the next key press (or a modifier pressed and released alone) is
  // the new hotkey, Esc cancels. The current hotkeys are held off meanwhile, so pressing one
  // doesn't start a recording.
  useEffect(() => {
    if (!editing) return;
    setPracticeMode(true);
    const capture = createHotkeyCapture((hotkey) => saveRef.current(editing, hotkey));
    const onDown = (e: KeyboardEvent) => {
      e.preventDefault();
      if (e.key === "Escape") {
        setConflict(false);
        setEditing(null);
        return;
      }
      capture.keydown(e);
    };
    const onUp = (e: KeyboardEvent) => {
      e.preventDefault();
      capture.keyup();
    };
    window.addEventListener("keydown", onDown);
    window.addEventListener("keyup", onUp);
    return () => {
      window.removeEventListener("keydown", onDown);
      window.removeEventListener("keyup", onUp);
      setPracticeMode(false);
    };
  }, [editing]);

  const rows: {
    id: "main" | "translate";
    name: string;
    hint: string;
    value: string;
    color: string;
    editable: boolean;
  }[] = [
    {
      id: "main",
      name: "Dictate",
      hint: "Press, speak, press again",
      value: settings.hotkey,
      color: p.accent,
      editable: true,
    },
    {
      id: "translate",
      name: "Translate",
      hint:
        mode === "off"
          ? "Off. Turn it on in Settings → General"
          : mode === "auto"
            ? `Dictate translates other languages → ${target}`
            : `Press, speak, press again → ${target}`,
      value: mode === "hotkey" ? translateKey : "",
      color: p.orange,
      editable: mode === "hotkey",
    },
  ];

  return (
    <div style={{ display: "flex", flexDirection: "column", gap: 8, width: "100%", maxWidth: 420 }}>
      {rows.map((k) => {
        const active = editing === k.id;
        return (
          <div
            key={k.id}
            onClick={k.editable ? () => setEditing(active ? null : k.id) : undefined}
            title={k.editable && !active ? "Click to change" : undefined}
            style={{
              display: "flex",
              alignItems: "center",
              gap: 12,
              padding: 12,
              borderRadius: 10,
              background: p.row,
              border: `1px solid ${active ? p.accentSoftBorder : p.rowBorder}`,
              cursor: k.editable ? "pointer" : "default",
            }}
          >
            <div
              style={{ width: 8, height: 8, borderRadius: 4, background: k.color, flexShrink: 0 }}
            />
            <div style={{ display: "flex", flexDirection: "column", gap: 3, flex: 1, minWidth: 0 }}>
              <span style={{ fontSize: 13, fontWeight: 500 }}>{k.name}</span>
              {active ? (
                <div style={{ display: "flex", gap: 4, flexWrap: "wrap" }}>
                  {HOTKEY_PRESETS.map((preset) => (
                    <Chip
                      key={preset.value}
                      p={p}
                      on={k.value === preset.value}
                      onClick={() => save(k.id, preset.value)}
                    >
                      {preset.label}
                    </Chip>
                  ))}
                  {k.id === "translate" && (
                    <Chip p={p} on={!k.value} onClick={() => save(k.id, "")}>
                      Off
                    </Chip>
                  )}
                </div>
              ) : (
                <span style={{ fontSize: 12, color: p.dim }}>{k.hint}</span>
              )}
            </div>
            <div style={{ display: "flex", gap: 4, flexShrink: 0 }}>
              {active ? (
                <span
                  style={{
                    font: `600 11px ${MONO}`,
                    color: conflict ? p.red : p.accent,
                    padding: "6px 8px",
                    borderRadius: 6,
                    border: `1px dashed ${conflict ? p.red : p.accentSoftBorder}`,
                  }}
                >
                  {conflict ? "USED BY DICTATE" : "PRESS KEYS…"}
                </span>
              ) : k.value ? (
                keycaps(k.value).map((key) => (
                  <Keycap key={key} p={p}>
                    {key}
                  </Keycap>
                ))
              ) : k.editable ? (
                <span style={{ font: `600 11px ${MONO}`, color: p.accent }}>SET</span>
              ) : null}
            </div>
          </div>
        );
      })}
    </div>
  );
}

/** The text style: the same presets as Settings → Text. */
function StyleStage({ p }: { p: Palette }) {
  const { settings, updateSettings } = useSettingsContext();
  const current = presetOf(settings);
  const styles = STYLES.filter((s) => s.value !== "custom");
  return (
    <div
      style={{ display: "flex", flexDirection: "column", gap: 10, width: "100%", maxWidth: 420 }}
    >
      <div style={{ display: "grid", gridTemplateColumns: "repeat(3,minmax(0,1fr))", gap: 6 }}>
        {styles.map(({ value, label }) => {
          const on = value === current;
          return (
            <button
              key={value}
              type="button"
              onClick={() => updateSettings(PRESETS[value as Preset])}
              style={{
                padding: "9px 10px",
                borderRadius: 9,
                background: on ? p.orangeSoft : p.row,
                border: `1px solid ${on ? p.orangeBorder : p.rowBorder}`,
                color: on ? p.orange : p.text,
                font: `500 13px ${SANS}`,
                cursor: "pointer",
              }}
            >
              {label}
            </button>
          );
        })}
      </div>
      <div style={{ font: `400 12px ${SANS}`, color: p.dim, minHeight: 32, textAlign: "center" }}>
        {styles.find((s) => s.value === current)?.hint ?? "Your own mix, set in Settings → Text"}
      </div>
    </div>
  );
}

function LangStage({ p }: { p: Palette }) {
  const { settings, updateSettings } = useSettingsContext();
  const target = settings.target_lang || "English";
  const options = LANGUAGES.some(([, n]) => n === target)
    ? LANGUAGES
    : [...LANGUAGES.slice(0, 5), [langCode(target), target] as [string, string]];
  return (
    <div
      style={{ display: "flex", flexDirection: "column", gap: 12, width: "100%", maxWidth: 400 }}
    >
      <div
        style={{
          display: "flex",
          alignItems: "center",
          gap: 8,
          font: `600 11px ${MONO}`,
          color: p.dim,
        }}
      >
        <span style={{ padding: "3px 7px", borderRadius: 5, border: `1px dashed ${p.dashed}` }}>
          ANY LANGUAGE
        </span>
        <span>→</span>
        <span
          style={{ padding: "3px 7px", borderRadius: 5, background: p.orangeSoft, color: p.orange }}
        >
          {langCode(target)}
        </span>
      </div>
      <div style={{ display: "grid", gridTemplateColumns: "repeat(3,minmax(0,1fr))", gap: 6 }}>
        {options.map(([code, name]) => {
          const on = name === target;
          return (
            <button
              key={code}
              type="button"
              onClick={() => updateSettings({ target_lang: name })}
              style={{
                display: "flex",
                alignItems: "center",
                gap: 8,
                padding: "9px 10px",
                borderRadius: 9,
                background: on ? p.orangeSoft : p.row,
                border: `1px solid ${on ? p.orangeBorder : p.rowBorder}`,
                color: p.text,
                font: `500 13px ${SANS}`,
                cursor: "pointer",
                textAlign: "left",
              }}
            >
              <span style={{ font: `600 10px ${MONO}`, color: on ? p.orange : p.dim }}>{code}</span>
              {name}
            </button>
          );
        })}
      </div>
    </div>
  );
}

// ── Try it: practice while the model downloads, then the real thing ────────

/** Example dictation for the practice run (unlike the translation example, so the two differ). */
const PRACTICE_DICTATION = "Quick note: the report is ready, I'll send it over after lunch.";

/** Example translation output for the practice run, by target language. */
const PRACTICE_SAMPLES: Record<string, string> = {
  English: "Hey team, let's move the sync to 3 pm tomorrow.",
  Ukrainian: "Привіт, команда! Давайте перенесемо зустріч на завтра на 15:00.",
  German: "Hallo Team, lasst uns das Meeting auf morgen 15 Uhr verschieben.",
  Spanish: "Hola, equipo: movamos la reunión a mañana a las 15:00.",
  French: "Salut l'équipe, décalons la réunion à demain 15 h.",
  Polish: "Cześć zespole, przenieśmy spotkanie na jutro na 15:00.",
};

/** Simulated stage lengths in ms, and the shortest recording that isn't "too short". */
const PRACTICE_TRANSCRIBE_MS = 1300;
const PRACTICE_TRANSLATE_MS = 1300;
const PRACTICE_MIN_MS = 600;
const PRACTICE_FAIL_MS = 1600;
/** Typing speed of the example text, in ms per character. */
const PRACTICE_TYPE_MS = 18;

type TryPhase = "idle" | "listen" | "tx" | "translate" | "done" | "fail";

interface TryControls {
  phase: TryPhase;
  /** Starts or finishes a recording (the footer button). */
  press: () => void;
}

function TryStage({
  t,
  p,
  practice,
  onState,
}: {
  t: number;
  p: Palette;
  practice: boolean;
  onState: (c: TryControls) => void;
}) {
  const { settings } = useSettingsContext();
  const [phase, setPhase] = useState<TryPhase>("idle");
  const [run, setRun] = useState<"dictate" | "translate">("dictate");
  const [text, setText] = useState("");
  const [heard, setHeard] = useState<string | null>(null);
  const [level, setLevel] = useState(0);
  const areaRef = useRef<HTMLTextAreaElement>(null);
  const timers = useRef<ReturnType<typeof setTimeout>[]>([]);
  const startedAt = useRef(0);

  const target = PRACTICE_SAMPLES[settings.target_lang] ? settings.target_lang : "English";
  const source = target === "Ukrainian" ? "English" : "Ukrainian";

  const later = (ms: number, fn: () => void) => {
    timers.current.push(setTimeout(fn, ms));
  };
  const textRef = useRef(text);
  textRef.current = text;
  /** The box's text with `add` on a new line. */
  const withLine = (add: string) => {
    const prev = textRef.current.trimEnd();
    return prev ? `${prev}\n${add}` : add;
  };

  const typeOut = (sample: string, then: () => void) => {
    const base = withLine("");
    let n = 0;
    const tick = () => {
      n += 1;
      setText(base + sample.slice(0, n));
      if (n < sample.length) later(PRACTICE_TYPE_MS, tick);
      else then();
    };
    later(PRACTICE_TYPE_MS, tick);
  };

  // ── Practice: the hotkeys come here instead of starting a real recording.
  const practiceFinish = (current: "dictate" | "translate") => {
    if (performance.now() - startedAt.current < PRACTICE_MIN_MS) {
      setPhase("fail");
      later(PRACTICE_FAIL_MS, () => setPhase("idle"));
      return;
    }
    const translated = current === "translate";
    setPhase("tx");
    later(PRACTICE_TRANSCRIBE_MS, () => {
      if (translated) {
        setHeard(PRACTICE_SAMPLES[source] ?? null);
        setPhase("translate");
      }
      later(translated ? PRACTICE_TRANSLATE_MS : 0, () => {
        setPhase("done");
        typeOut((translated ? PRACTICE_SAMPLES[target] : PRACTICE_DICTATION) ?? "", () => {
          later(PRACTICE_FAIL_MS, () => setPhase("idle"));
        });
      });
    });
  };

  const live = useRef({ phase, run });
  live.current = { phase, run };

  const practicePress = (hotkey: "main" | "translate") => {
    const { phase: cur, run: curRun } = live.current;
    if (cur === "listen") {
      practiceFinish(curRun);
      return;
    }
    if (cur !== "idle" && cur !== "done") return;
    timers.current.forEach(clearTimeout);
    setRun(hotkey === "translate" ? "translate" : "dictate");
    setHeard(null);
    startedAt.current = performance.now();
    setPhase("listen");
  };
  const practicePressRef = useRef(practicePress);
  practicePressRef.current = practicePress;

  useEffect(() => {
    if (!practice) return;
    setPracticeMode(true);
    const unlisten = subscribeToPracticeHotkey((mode) => practicePressRef.current(mode));
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape" && live.current.phase === "listen") setPhase("idle");
    };
    window.addEventListener("keydown", onKey);
    const pending = timers.current;
    return () => {
      setPracticeMode(false);
      unlisten.then((fn) => fn?.());
      window.removeEventListener("keydown", onKey);
      pending.forEach(clearTimeout);
    };
  }, [practice]);

  // While practising, the bars move with the real microphone.
  useEffect(() => {
    if (!practice || phase !== "listen") return;
    let alive = true;
    let unlisten: (() => void) | null = null;
    startMicTest(settings.input_device ?? null);
    subscribeToMicTestLevels((data) => {
      if (alive) setLevel(typeof data === "number" ? data : (data?.level ?? 0));
    }).then((un) => {
      if (alive) unlisten = un;
      else un?.();
    });
    return () => {
      alive = false;
      unlisten?.();
      stopMicTest();
      setLevel(0);
    };
  }, [practice, phase, settings.input_device]);

  // ── Real: a real recording, pasted into the box (or added from the result if not pasted).
  useEffect(() => {
    if (practice) return;
    areaRef.current?.focus();
    const unState = subscribeToStateChanges((e) => {
      if (e.state === "listening") {
        setRun(e.mode === "translate" ? "translate" : "dictate");
        setPhase("listen");
      } else if (e.state === "transcribing") setPhase("tx");
      else if (e.state === "translating") setPhase("translate");
      else if (e.state === "error") {
        setPhase("fail");
        later(PRACTICE_FAIL_MS, () => setPhase("idle"));
      } else if (e.state === "done") {
        setPhase("done");
        const result = e.text;
        if (result)
          later(400, () => {
            if (!textRef.current.includes(result)) setText(withLine(result));
          });
      }
    });
    const unLevels = subscribeToAudioLevels((l) => setLevel(l.volume));
    const pending = timers.current;
    return () => {
      unState.then((fn) => fn?.());
      unLevels.then((fn) => fn?.());
      pending.forEach(clearTimeout);
    };
    // `later` and `withLine` only touch refs.
  }, [practice]);

  const press = () => {
    if (practice) {
      practicePressRef.current("main");
      return;
    }
    // Keep the box focused so the result is pasted into it.
    areaRef.current?.focus();
    triggerToggleRecording();
  };
  const pressRef = useRef(press);
  pressRef.current = press;
  useEffect(() => {
    onState({ phase, press: () => pressRef.current() });
  }, [phase, onState]);

  const kind: MorphKind = (
    {
      idle: "silent",
      listen: "bars",
      tx: "scan",
      translate: "orbit",
      done: "copy",
      fail: "x",
    } as const
  )[phase];
  const placeholder =
    phase === "fail"
      ? practice
        ? "Too short. Speak a little longer before you press again."
        : "That didn't work. Try again."
      : phase === "listen"
        ? "Listening…"
        : phase === "tx"
          ? "Transcribing…"
          : phase === "translate"
            ? "Translating…"
            : "Your text will appear here";

  return (
    <div
      style={{
        display: "flex",
        flexDirection: "column",
        alignItems: "center",
        gap: 12,
        width: "100%",
      }}
    >
      <MiniPill
        kind={kind}
        t={t}
        p={p}
        level={level}
        opacity={phase === "idle" ? 0.5 : 1}
        slot={
          phase === "translate" || (run === "translate" && phase === "listen")
            ? `${langCode(source)} → ${langCode(target)}`
            : null
        }
      />
      <div style={{ width: "100%", maxWidth: 400 }}>
        <textarea
          ref={areaRef}
          value={text}
          onChange={(e) => setText(e.target.value)}
          placeholder={placeholder}
          rows={2}
          style={{
            width: "100%",
            boxSizing: "border-box",
            padding: "10px 12px",
            borderRadius: 10,
            background: p.row,
            border: `1px solid ${phase === "done" ? p.greenBorder : p.rowBorder}`,
            color: p.text,
            font: `400 14px/1.45 ${SANS}`,
            resize: "none",
            outline: "none",
            userSelect: "text",
            transition: "border-color 200ms",
          }}
        />
        <div style={{ font: `500 10.5px ${MONO}`, color: p.faint, marginTop: 4, minHeight: 14 }}>
          {heard
            ? `EXAMPLE · HEARD: "${heard}"`
            : practice
              ? "PRACTICE RUN · EXAMPLE TEXT · NOTHING IS RECORDED"
              : ""}
        </div>
      </div>
    </div>
  );
}

// ── Shell ───────────────────────────────────────────────────────────────────

function OnboardingInner() {
  const { settings } = useSettingsContext();
  const p = useResolvedTheme(settings.theme) === "dark" ? DARK : LIGHT;
  const [index, setIndex] = useState(0);
  const [t, setT] = useState(0);
  const dl = useModelDownload();
  const step = STEPS[index] ?? "welcome";
  const last = index === STEPS.length - 1;
  const go = (i: number) => setIndex(Math.max(0, Math.min(STEPS.length - 1, i)));

  // Mic step: opening the microphone is what makes macOS ask for access.
  const [micState, setMicState] = useState<MicState>("off");
  const [micLevel, setMicLevel] = useState(0);
  const micOn = micState !== "off";
  useEffect(() => {
    if (step !== "mic" || !micOn) return;
    let alive = true;
    let unlisten: (() => void) | null = null;
    startMicTest(settings.input_device ?? null);
    subscribeToMicTestLevels((data) => {
      if (!alive) return;
      const level = typeof data === "number" ? data : (data?.level ?? 0);
      setMicLevel(level);
      if (level >= MIC_ALIVE_LEVEL) setMicState("allowed");
    }).then((un) => {
      if (alive) unlisten = un;
      else un?.();
    });
    return () => {
      alive = false;
      unlisten?.();
      stopMicTest();
      setMicLevel(0);
    };
  }, [step, micOn, settings.input_device]);

  // Accessibility step: macOS doesn't announce the grant, so check while it's open.
  const [granted, setGranted] = useState<boolean | null>(null);
  useEffect(() => {
    if (step !== "access") return;
    let alive = true;
    const check = async () => {
      const ok = await checkAccessibility().catch(() => false);
      if (alive) setGranted(ok);
    };
    check();
    const timer = setInterval(check, 1000);
    return () => {
      alive = false;
      clearInterval(timer);
    };
  }, [step]);

  // Animation clock for the illustrations.
  useEffect(() => {
    const id = setInterval(() => setT((n) => n + 1), TICK_MS);
    return () => clearInterval(id);
  }, []);

  // Try step: practice until the model is ready; switch once no practice run is playing.
  const [tryCtl, setTryCtl] = useState<TryControls>({ phase: "idle", press: () => {} });
  const [practice, setPractice] = useState(true);
  const tryPhase = tryCtl.phase;
  useEffect(() => {
    const ready = dl.phase === "ready";
    if (step !== "try") setPractice(!ready);
    else if (ready && (tryPhase === "idle" || tryPhase === "done")) setPractice(false);
  }, [step, dl.phase, tryPhase]);

  const key = hotkeyName(settings.hotkey);
  const translateKey =
    settings.translation_mode === "hotkey" && settings.translate_hotkey
      ? hotkeyName(settings.translate_hotkey)
      : null;
  const translateHint = translateKey ? ` ${translateKey} translates.` : "";

  const copy: Record<StepId, { title: string; body: string }> = {
    welcome: {
      title: "Speak anywhere. Bifaria types it.",
      body: "Press a hotkey, talk, press it again. Your words are transcribed (or translated) and pasted right where your cursor is.",
    },
    mic: {
      title: "Allow microphone access",
      body: "Bifaria only listens while you record. Nothing is recorded in the background, and speech is turned into text on your computer.",
    },
    access: {
      title: "Let Bifaria paste for you",
      body: "Accessibility access lets Bifaria notice your hotkey in any app and drop text right where your cursor is.",
    },
    keys: {
      title: "Two hotkeys, that’s it",
      body: "One to dictate in whatever language you speak, one to translate. Click a hotkey to change it, or change both later in Settings.",
    },
    lang: {
      title: "Translate into…",
      body: "Speak any language and Bifaria detects it. Pick the language translations should come out in.",
    },
    style: {
      title: "How should your text look?",
      body: "Pick a style for the text Bifaria pastes. Change it any time in Settings → Text, where you can also fine-tune each step.",
    },
    try: {
      title: "Give it a try",
      body: practice
        ? `The speech model is still downloading, so this is a practice run. Press ${key}, say a sentence, press it again.${translateHint} Real recording unlocks when the download finishes.`
        : `Press ${key} (or the button), say a sentence, then press it again.${translateHint}`,
    },
  };

  const next = () => {
    if (last) {
      finishOnboarding();
      return;
    }
    if (step === "mic" && micState === "off") setMicState("waiting");
    go(index + 1);
  };
  const nextLabel =
    step === "welcome"
      ? "Get started"
      : step === "mic" && micState === "off"
        ? "Allow & continue"
        : last
          ? "Finish"
          : "Continue";
  const busy = tryCtl.phase === "tx" || tryCtl.phase === "translate";
  // The button records for real, so it waits for the model; the hotkey still runs the practice.
  const talkOff = busy || practice;
  // While it waits, the button shows why: the model download and how far along it is.
  const waitLabel =
    dl.phase === "downloading" || dl.phase === "starting"
      ? `Downloading model ${dl.percent}%`
      : dl.phase === "failed"
        ? "Model download failed"
        : dl.phase === "idle"
          ? "Model not downloaded"
          : "Checking model…";
  const waiting = practice && !busy;

  return (
    <div
      style={{
        width: "100vw",
        height: "100vh",
        boxSizing: "border-box",
        background: p.card,
        border: `1px solid ${p.cardBorder}`,
        borderRadius: 20,
        color: p.text,
        fontFamily: SANS,
        display: "flex",
        flexDirection: "column",
        overflow: "hidden",
        userSelect: "none",
      }}
    >
      {/* Header: also the window's drag handle */}
      <div
        data-tauri-drag-region
        style={{ display: "flex", alignItems: "center", gap: 12, padding: "20px 24px 0" }}
      >
        <div data-tauri-drag-region style={{ display: "flex", alignItems: "center", gap: 8 }}>
          <AppMark />
          <span data-tauri-drag-region style={{ fontSize: 14, fontWeight: 600 }}>
            Bifaria
          </span>
        </div>
        <div data-tauri-drag-region style={{ flex: 1, alignSelf: "stretch" }} />
        <ModelStatus dl={dl} p={p} />
        <span style={{ font: `500 11px ${MONO}`, color: p.dim }}>
          {index + 1} / {STEPS.length}
        </span>
        {!last && (
          <button
            type="button"
            onClick={() => go(STEPS.length - 1)}
            style={{
              background: "none",
              border: "none",
              color: p.dim,
              font: `500 13px ${SANS}`,
              cursor: "pointer",
              padding: "6px 4px",
            }}
          >
            Skip
          </button>
        )}
        {/* Closing counts as finishing: the guide doesn't open again on its own. */}
        <button
          type="button"
          onClick={() => finishOnboarding()}
          title="Close"
          aria-label="Close"
          style={{
            width: 26,
            height: 26,
            marginRight: -6,
            display: "flex",
            alignItems: "center",
            justifyContent: "center",
            background: "none",
            border: "none",
            borderRadius: 7,
            color: p.dim,
            cursor: "pointer",
          }}
          onMouseEnter={(e) => (e.currentTarget.style.background = p.segOff)}
          onMouseLeave={(e) => (e.currentTarget.style.background = "none")}
        >
          <svg width={12} height={12} viewBox="0 0 12 12" fill="none">
            <path
              d="M2.5 2.5l7 7M9.5 2.5l-7 7"
              stroke="currentColor"
              strokeWidth={1.6}
              strokeLinecap="round"
            />
          </svg>
        </button>
      </div>
      <div data-tauri-drag-region style={{ display: "flex", gap: 4, padding: "14px 24px 0" }}>
        {STEPS.map((id, i) => (
          <div
            key={id}
            style={{
              flex: 1,
              height: 3,
              borderRadius: 2,
              background: i <= index ? p.accent : p.segOff,
              transition: "background 200ms",
            }}
          />
        ))}
      </div>

      {/* Stage */}
      <div style={{ padding: "24px 24px 0" }}>
        <div
          style={{
            height: 200,
            borderRadius: 14,
            background: p.stage,
            border: `1px solid ${p.stageBorder}`,
            display: "flex",
            alignItems: "center",
            justifyContent: "center",
            padding: 20,
            boxSizing: "border-box",
          }}
        >
          {step === "welcome" && (
            <WelcomeStage t={t} p={p} target={langCode(settings.target_lang || "English")} />
          )}
          {step === "mic" && (
            <MicStage
              p={p}
              micState={micState}
              level={micLevel}
              onAllow={() => setMicState("waiting")}
            />
          )}
          {step === "access" && <AccessStage p={p} granted={granted} />}
          {step === "keys" && <KeysStage p={p} />}
          {step === "lang" && <LangStage p={p} />}
          {step === "style" && <StyleStage p={p} />}
          {step === "try" && <TryStage t={t} p={p} practice={practice} onState={setTryCtl} />}
        </div>
      </div>

      {/* Copy */}
      <div style={{ padding: "22px 24px 0", display: "flex", flexDirection: "column", gap: 8 }}>
        <div style={{ fontSize: 22, fontWeight: 600, letterSpacing: "-.01em" }}>
          {copy[step].title}
        </div>
        <div style={{ fontSize: 14, lineHeight: 1.5, color: p.body, minHeight: 42 }}>
          {copy[step].body}
        </div>
      </div>

      <div style={{ flex: 1 }} />

      {/* Footer */}
      <div style={{ display: "flex", alignItems: "center", gap: 8, padding: "20px 24px 24px" }}>
        {index > 0 && (
          <button
            type="button"
            onClick={() => go(index - 1)}
            style={{
              background: "none",
              border: `1px solid ${p.ghostBorder}`,
              color: p.text,
              borderRadius: 10,
              padding: "10px 16px",
              font: `500 14px ${SANS}`,
              cursor: "pointer",
            }}
          >
            Back
          </button>
        )}
        <div style={{ flex: 1 }} />
        {step === "try" && (
          <button
            type="button"
            onClick={tryCtl.press}
            disabled={talkOff}
            title={waiting ? "Press to talk works once the speech model is downloaded" : undefined}
            style={{
              display: "flex",
              alignItems: "center",
              gap: 8,
              background: waiting
                ? `linear-gradient(90deg, ${p.accentSoftBorder} ${dl.percent}%, ${p.accentSoft} ${dl.percent}%)`
                : p.accentSoft,
              border: `1px solid ${p.accentSoftBorder}`,
              color: p.accentText,
              borderRadius: 10,
              padding: "10px 14px",
              font: `500 14px ${SANS}`,
              cursor: talkOff ? "default" : "pointer",
              opacity: busy ? 0.5 : 1,
              fontVariantNumeric: "tabular-nums",
            }}
          >
            {waiting ? (
              waitLabel
            ) : (
              <>
                {tryCtl.phase === "listen" ? "Press to finish" : "Press to talk"}
                <span style={{ font: `600 11px ${MONO}`, opacity: 0.8 }}>{key}</span>
              </>
            )}
          </button>
        )}
        <button
          type="button"
          onClick={next}
          style={{
            background: p.accent,
            color: p.onAccent,
            border: "none",
            borderRadius: 10,
            padding: "10px 18px",
            font: `600 14px ${SANS}`,
            cursor: "pointer",
          }}
        >
          {nextLabel}
        </button>
      </div>
    </div>
  );
}

export function Onboarding() {
  return (
    <SettingsProvider>
      <OnboardingInner />
    </SettingsProvider>
  );
}

export default Onboarding;
