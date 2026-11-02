import React, { useCallback, useEffect, useRef, useState } from "react";
import { useResolvedTheme } from "@/lib/theme";
import { createHotkeyCapture, formatDisplay } from "@/lib/hotkey";
import { SettingsProvider } from "@/components/settings/SettingsContext";
import { useSettingsContext } from "@/components/settings/useSettingsContext";
import { PRESETS, STYLES, presetOf, type Preset } from "@/lib/textStyles";
import { TARGET_LANGUAGES } from "@/lib/languages";
import {
  checkAccessibility,
  downloadAppleLanguagePack,
  fetchAppleLanguagePacks,
  subscribeToOfflinePackProgress,
  fetchAppleTranslationInfo,
  finishOnboarding,
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
  type AppleLanguagePack,
  type AppleTranslationInfo,
  type TranslationMode,
} from "@/lib/tauri";

/** Hotkeys and pasting need the Accessibility permission only on macOS. */
const IS_MAC = typeof navigator !== "undefined" && /Mac/i.test(navigator.userAgent);

type StepId = "welcome" | "perms" | "model" | "keys" | "translation" | "style" | "try";

/** Welcome and Try it bookend the flow; the steps between are counted in the header. */
const STEPS: StepId[] = ["welcome", "perms", "model", "keys", "translation", "style", "try"];
const STEP_NAMES: Partial<Record<StepId, string>> = {
  perms: "Permissions",
  model: "Speech model",
  keys: "Languages & hotkeys",
  translation: "Translation",
  style: "Text style",
};

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
  rowBorder: "rgba(255,255,255,.14)",
  key: "#22223a",
  keyBorder: "rgba(255,255,255,.1)",
  text: "#e8e8f0",
  body: "#c2c2d4",
  dim: "#a6a6bc",
  faint: "#8c8ca4",
  divider: "rgba(255,255,255,.14)",
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
  body: "#4a4a5e",
  dim: "#62627a",
  faint: "#86869c",
  divider: "rgba(0,0,0,.1)",
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

/** The accessibility figure (a person in a circle), as macOS shows for Accessibility. */
function AccessIcon({ color }: { color: string }) {
  return (
    <svg
      width={28}
      height={28}
      viewBox="0 0 24 24"
      fill="none"
      stroke={color}
      strokeWidth={1.7}
      strokeLinecap="round"
      strokeLinejoin="round"
    >
      <circle cx={12} cy={12} r={10} />
      <circle cx={12} cy={7} r={1.2} fill={color} />
      <path d="M7 9.5l5 1.2 5-1.2M12 10.7v3.3M12 14l-2.2 4.2M12 14l2.2 4.2" />
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
        padding: "14px 16px",
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

// ── Building blocks of a step: eyebrow, title, lede, labelled sections ─────

function Eyebrow({ p, children }: { p: Palette; children: React.ReactNode }) {
  return (
    <div style={{ font: `600 12px ${MONO}`, letterSpacing: ".1em", color: p.accent }}>
      {children}
    </div>
  );
}

function Title({ children }: { children: React.ReactNode }) {
  return (
    <div style={{ fontSize: 30, fontWeight: 600, letterSpacing: "-.015em", lineHeight: 1.2 }}>
      {children}
    </div>
  );
}

function Lede({ p, children }: { p: Palette; children: React.ReactNode }) {
  return <div style={{ fontSize: 16, lineHeight: 1.55, color: p.body }}>{children}</div>;
}

/** Short notes, one sentence per line, so they read at a glance instead of as a wall of text. */
function Lines({ p, lines, size = 13 }: { p: Palette; lines: string[]; size?: number }) {
  return (
    <div style={{ display: "flex", flexDirection: "column", gap: 4 }}>
      {lines.map((l) => (
        <div key={l} style={{ fontSize: size, lineHeight: 1.5, color: p.dim }}>
          {l}
        </div>
      ))}
    </div>
  );
}

function Fine({ p, children }: { p: Palette; children: React.ReactNode }) {
  return <div style={{ fontSize: 13, lineHeight: 1.55, color: p.dim }}>{children}</div>;
}

function SectionLabel({ p, children }: { p: Palette; children: React.ReactNode }) {
  return (
    <div style={{ font: `600 11.5px ${MONO}`, letterSpacing: ".08em", color: p.dim }}>
      {children}
    </div>
  );
}

/** A dropdown in the guide's palette. */
function selectStyle(p: Palette): React.CSSProperties {
  return {
    height: CONTROL_H,
    padding: "0 10px",
    borderRadius: 8,
    background: p.row,
    color: p.text,
    border: `1px solid ${p.rowBorder}`,
    font: `500 14px ${SANS}`,
    cursor: "pointer",
  };
}

/** A thin line between sections, so blocks read apart (above all on the dark theme). */
function Divider({ p }: { p: Palette }) {
  return <div style={{ height: 1, background: p.divider, margin: "6px 0" }} />;
}

function PrimaryButton({
  p,
  onClick,
  children,
}: {
  p: Palette;
  onClick: () => void;
  children: React.ReactNode;
}) {
  return (
    <button
      type="button"
      onClick={onClick}
      style={{
        background: p.accent,
        color: p.onAccent,
        border: "none",
        borderRadius: 8,
        padding: "8px 14px",
        font: `600 13px ${SANS}`,
        cursor: "pointer",
        whiteSpace: "nowrap",
      }}
    >
      {children}
    </button>
  );
}

function Done({ p, children }: { p: Palette; children: React.ReactNode }) {
  return <span style={{ font: `600 12px ${MONO}`, color: p.green }}>✓ {children}</span>;
}

/** Size of the hotkeys step's controls, so the hotkeys, switch and dropdown line up. */
const CONTROL_W = 340;
const CONTROL_H = 38;

/** A segmented control in the guide's palette; with `width`, its options share it equally. */
function Segments<T extends string>({
  p,
  value,
  options,
  onChange,
  width,
}: {
  p: Palette;
  value: T;
  options: { value: T; label: string }[];
  onChange: (v: T) => void;
  width?: number;
}) {
  return (
    <div
      role="radiogroup"
      style={{
        display: "inline-flex",
        width,
        height: CONTROL_H,
        boxSizing: "border-box",
        padding: 3,
        gap: 2,
        borderRadius: 9,
        background: p.stage,
        border: `1px solid ${p.rowBorder}`,
      }}
    >
      {options.map((o) => {
        const on = o.value === value;
        return (
          <button
            key={o.value}
            type="button"
            role="radio"
            aria-checked={on}
            onClick={() => onChange(o.value)}
            style={{
              flex: width ? 1 : undefined,
              padding: "0 14px",
              borderRadius: 7,
              border: "none",
              background: on ? p.accent : "transparent",
              color: on ? p.onAccent : p.text,
              font: `${on ? 600 : 500} 14px ${SANS}`,
              cursor: "pointer",
              whiteSpace: "nowrap",
            }}
          >
            {o.label}
          </button>
        );
      })}
    </div>
  );
}

// ── Welcome ─────────────────────────────────────────────────────────────────

/** You speak → Transcription → Text (the dictation path, in the accent colour), with an optional
 * branch through Translation (orange). */
function FlowDiagram({ p, hotkey }: { p: Palette; hotkey: string }) {
  const node = (x: number, y: number, color: string, filled = false) => (
    <circle cx={x} cy={y} r={6} fill={filled ? color : p.card} stroke={color} strokeWidth={2} />
  );
  const label = (x: number, y: number, text: string, color: string, sub?: string) => (
    <>
      <text x={x} y={y} fill={color} style={{ font: `600 14px ${SANS}` }}>
        {text}
      </text>
      {sub && (
        <text x={x} y={y + 17} fill={p.dim} style={{ font: `400 12px ${SANS}` }}>
          {sub}
        </text>
      )}
    </>
  );
  return (
    <svg viewBox="0 0 560 124" width="100%" style={{ maxWidth: 560 }} aria-hidden>
      <path
        d="M12 54 H230 C275 54 275 22 320 22 H440"
        stroke={p.accent}
        strokeWidth={2}
        fill="none"
      />
      <path d="M230 54 C275 54 275 88 320 88 H440" stroke={p.orange} strokeWidth={2} fill="none" />
      {node(12, 54, p.accent)}
      {node(128, 54, p.accent)}
      {node(332, 88, p.orange)}
      {node(440, 22, p.accent, true)}
      {node(440, 88, p.orange, true)}
      {label(0, 82, "You speak", p.text, hotkey)}
      {label(106, 82, "Transcription", p.text)}
      {label(298, 116, "Translation", p.orange)}
      {label(454, 27, "Text", p.accentText, "at your cursor")}
      {label(454, 93, "Translated", p.orange, "optional")}
    </svg>
  );
}

function WelcomeStep({ p, hotkey, place }: { p: Palette; hotkey: string; place: string }) {
  return (
    <>
      <Eyebrow p={p}>WELCOME</Eyebrow>
      <Title>Speak anywhere. Bifaria types it.</Title>
      <Lede p={p}>
        Press a hotkey, speak, press it again. Bifaria writes down what you said, or translates it,
        and pastes it where your cursor is.
      </Lede>
      <div style={{ padding: "8px 0" }}>
        <FlowDiagram p={p} hotkey={hotkey} />
      </div>
      <Row p={p}>
        <div style={{ display: "flex", flexDirection: "column", gap: 4 }}>
          <span style={{ fontSize: 16, fontWeight: 600 }}>Your voice stays on {place}</span>
          <Lines
            p={p}
            size={14}
            lines={[
              "Speech is turned into text right here. No audio is sent anywhere.",
              "Translation is optional. Dictation works on its own.",
            ]}
          />
        </div>
      </Row>
      <Lines
        p={p}
        lines={[
          "Setup takes about two minutes.",
          "The speech model downloads once while you set up, so it's ready by the end.",
        ]}
      />
    </>
  );
}

// ── Permissions ─────────────────────────────────────────────────────────────

type MicState = "off" | "waiting" | "allowed";

function Permission({
  p,
  icon,
  name,
  why,
  children,
}: {
  p: Palette;
  icon: React.ReactNode;
  name: string;
  why: string;
  children: React.ReactNode;
}) {
  return (
    <div style={{ display: "flex", gap: 12 }}>
      <div style={{ width: 32, flexShrink: 0, paddingTop: 2 }}>{icon}</div>
      <div style={{ display: "flex", flexDirection: "column", gap: 8, flex: 1 }}>
        <span style={{ fontSize: 17, fontWeight: 600 }}>{name}</span>
        <span style={{ fontSize: 14, lineHeight: 1.55, color: p.body }}>{why}</span>
        <div style={{ display: "flex", alignItems: "center", gap: 10, flexWrap: "wrap" }}>
          {children}
        </div>
      </div>
    </div>
  );
}

function PermsStep({
  p,
  micState,
  micLevel,
  onAllowMic,
  granted,
  place,
}: {
  p: Palette;
  micState: MicState;
  micLevel: number;
  onAllowMic: () => void;
  granted: boolean | null;
  place: string;
}) {
  const { settings, updateSettings, inputDevices } = useSettingsContext();
  const ring = micState === "off" ? 0 : Math.min(1, micLevel * 3);
  return (
    <>
      <Title>{IS_MAC ? "Two permissions from macOS" : "Microphone access"}</Title>
      <Lede p={p}>Bifaria asks only for what it needs, and uses each permission for one job.</Lede>
      <Divider p={p} />
      <div style={{ display: "flex", flexDirection: "column", gap: 14 }}>
        <Permission
          p={p}
          icon={
            <div
              style={{
                borderRadius: 14,
                boxShadow: `0 0 0 ${Math.round(ring * 6)}px ${p.accentSoft}`,
                transition: "box-shadow 80ms linear",
                display: "inline-flex",
              }}
            >
              <MicIcon color={p.accent} />
            </div>
          }
          name="Microphone"
          why={`To hear you. Only while you're recording, from one press of the hotkey to the next. Speech is processed on ${place}.`}
        >
          {micState === "allowed" ? (
            <Done p={p}>ALLOWED</Done>
          ) : micState === "waiting" ? (
            <span style={{ font: `600 11px ${MONO}`, color: p.dim }}>SAY SOMETHING…</span>
          ) : (
            <>
              <PrimaryButton p={p} onClick={onAllowMic}>
                Allow microphone
              </PrimaryButton>
              {IS_MAC && <Fine p={p}>macOS will ask you to confirm.</Fine>}
            </>
          )}
          <select
            value={settings.input_device ?? ""}
            onChange={(e) => updateSettings({ input_device: e.target.value || null })}
            style={{
              maxWidth: 220,
              background: "none",
              border: "none",
              color: p.dim,
              font: `500 11px ${MONO}`,
              cursor: "pointer",
            }}
          >
            <option value="">INPUT: SYSTEM DEFAULT</option>
            {inputDevices.map((d) => (
              <option key={d} value={d}>
                INPUT: {d.toUpperCase()}
              </option>
            ))}
          </select>
        </Permission>
        {IS_MAC && <Divider p={p} />}
        {IS_MAC && (
          <Permission
            p={p}
            icon={<AccessIcon color={p.accent} />}
            name="Accessibility"
            why="To notice the hotkey in any app and paste the text where your cursor is. Nothing else."
          >
            {granted ? (
              <Done p={p}>ALLOWED</Done>
            ) : (
              <>
                <PrimaryButton p={p} onClick={() => requestAccessibility()}>
                  Open System Settings
                </PrimaryButton>
                <Fine p={p}>Turn on Bifaria, then come back. This page updates by itself.</Fine>
              </>
            )}
          </Permission>
        )}
      </div>
      <Divider p={p} />
      <Fine p={p}>You can carry on and allow it later. Dictation won't work until you do.</Fine>
    </>
  );
}

// ── Speech model ────────────────────────────────────────────────────────────

function ModelStep({ p, dl, place }: { p: Palette; dl: ModelDownload; place: string }) {
  const { settings, models } = useSettingsContext();
  const model = models.find((m) => m.filename === settings.model_name);
  const size = model?.file_size_bytes ? formatBytes(model.file_size_bytes) : null;
  const percent = dl.phase === "ready" ? 100 : dl.percent;
  const status =
    dl.phase === "ready"
      ? "Ready"
      : dl.phase === "downloading" && dl.total > 0
        ? `${dl.percent}% · ${formatBytes(dl.downloaded)} of ${formatBytes(dl.total)}`
        : dl.phase === "failed"
          ? "Download failed"
          : dl.phase === "idle"
            ? "Not downloaded"
            : "Starting…";
  const col = (title: string, body: string) => (
    <div style={{ flex: 1, display: "flex", flexDirection: "column", gap: 4 }}>
      <span style={{ fontSize: 15, fontWeight: 600 }}>{title}</span>
      <span style={{ fontSize: 14, lineHeight: 1.55, color: p.body }}>{body}</span>
    </div>
  );
  return (
    <>
      <Title>Speech recognition runs on {place}</Title>
      <Lede p={p}>
        Bifaria turns speech into text with a model that lives on {place}. It downloads once
        {size ? `, about ${size},` : ""} then works offline.
      </Lede>
      <Divider p={p} />
      <div style={{ display: "flex", flexDirection: "column", gap: 8 }}>
        <div style={{ display: "flex", alignItems: "baseline", gap: 10 }}>
          <span style={{ fontSize: 17, fontWeight: 600, flex: 1 }}>Speech model</span>
          <span
            style={{
              font: `500 13px ${MONO}`,
              color: dl.phase === "ready" ? p.green : dl.phase === "failed" ? p.red : p.dim,
            }}
          >
            {status}
          </span>
          {(dl.phase === "failed" || dl.phase === "idle") && (
            <PrimaryButton p={p} onClick={dl.start}>
              {dl.phase === "failed" ? "Retry" : "Download"}
            </PrimaryButton>
          )}
        </div>
        <div style={{ height: 6, borderRadius: 3, background: p.segOff, overflow: "hidden" }}>
          <div
            style={{
              height: "100%",
              width: `${percent}%`,
              background: dl.phase === "ready" ? p.green : p.accent,
              transition: "width 400ms",
            }}
          />
        </div>
        {model && <Fine p={p}>{model.name}</Fine>}
      </div>
      <Divider p={p} />
      <div style={{ display: "flex", gap: 28 }}>
        {col(
          "While it downloads",
          "Keep going with setup. Dictation is a practice run, so you can learn the hotkey.",
        )}
        {col(
          "When it's ready",
          "Real dictation unlocks. Bifaria tells you, even if this window is closed.",
        )}
      </div>
      <Divider p={p} />
      <Lines
        p={p}
        lines={[
          `History, logs and (only if you choose) saved audio stay on ${place}.`,
          "None of it leaves. See or clear it in Settings › Storage.",
        ]}
      />
    </>
  );
}

// ── Languages and hotkeys ───────────────────────────────────────────────────

const MODE_OPTIONS: { value: TranslationMode; label: string }[] = [
  { value: "off", label: "Off" },
  { value: "hotkey", label: "Own hotkey" },
  { value: "auto", label: "Every dictation" },
];

/** A hotkey with a Change link; while changing, the next key press is the new hotkey. */
function HotkeyField({
  p,
  which,
  editing,
  setEditing,
}: {
  p: Palette;
  which: "main" | "translate";
  editing: "main" | "translate" | null;
  setEditing: (e: "main" | "translate" | null) => void;
}) {
  const { settings, updateSettings } = useSettingsContext();
  const [conflict, setConflict] = useState(false);
  const active = editing === which;
  const value = which === "main" ? settings.hotkey : (settings.translate_hotkey ?? "");

  const save = (v: string) => {
    if (which === "main") {
      // A translate hotkey on the same key would never fire; turn it off.
      updateSettings(
        v && v === settings.translate_hotkey ? { hotkey: v, translate_hotkey: "" } : { hotkey: v },
      );
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
    if (!active) return;
    setPracticeMode(true);
    const capture = createHotkeyCapture((hotkey) => saveRef.current(hotkey));
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
  }, [active, setEditing]);

  const box: React.CSSProperties = {
    display: "flex",
    alignItems: "center",
    gap: 4,
    width: CONTROL_W,
    height: CONTROL_H,
    boxSizing: "border-box",
    padding: "0 6px 0 8px",
    borderRadius: 8,
    background: p.row,
  };
  if (active) {
    return (
      <div style={{ display: "flex", flexDirection: "column", gap: 6 }}>
        <div
          style={{
            ...box,
            border: `1px dashed ${conflict ? p.red : p.accent}`,
            font: `600 12px ${MONO}`,
            color: conflict ? p.red : p.accent,
          }}
        >
          {conflict ? "USED BY DICTATE" : "PRESS KEYS…  (ESC CANCELS)"}
        </div>
        <div style={{ display: "flex", gap: 6, flexWrap: "wrap" }}>
          {HOTKEY_PRESETS.map((preset) => (
            <Chip
              key={preset.value}
              p={p}
              on={value === preset.value}
              onClick={() => save(preset.value)}
            >
              {preset.label}
            </Chip>
          ))}
        </div>
      </div>
    );
  }
  return (
    <div style={{ ...box, border: `1px solid ${p.rowBorder}` }}>
      {value ? (
        keycaps(value).map((key) => (
          <Keycap key={key} p={p}>
            {key}
          </Keycap>
        ))
      ) : (
        <span style={{ font: `600 12px ${MONO}`, color: p.dim }}>NOT SET</span>
      )}
      <button
        type="button"
        onClick={() => setEditing(which)}
        style={{
          marginLeft: "auto",
          background: "none",
          border: "none",
          color: p.accent,
          font: `600 13px ${SANS}`,
          cursor: "pointer",
          padding: 0,
        }}
      >
        Change
      </button>
    </div>
  );
}

/** One row of the hotkeys grid: a label column and a control with a hint under it. */
function FieldRow({
  p,
  label,
  hint,
  children,
}: {
  p: Palette;
  label: string;
  hint?: string;
  children: React.ReactNode;
}) {
  return (
    <div style={{ display: "grid", gridTemplateColumns: "150px 1fr", columnGap: 20, rowGap: 6 }}>
      <span style={{ fontSize: 15, fontWeight: 600, alignSelf: "center" }}>{label}</span>
      <div>{children}</div>
      {hint && (
        <>
          <span />
          <span style={{ fontSize: 13, color: p.dim }}>{hint}</span>
        </>
      )}
    </div>
  );
}

function KeysStep({ p }: { p: Palette }) {
  const { settings, updateSettings } = useSettingsContext();
  const [editing, setEditing] = useState<"main" | "translate" | null>(null);
  const mode = settings.translation_mode ?? "hotkey";
  const target = settings.target_lang || "English";
  const dictate = hotkeyName(settings.hotkey);
  const translate = settings.translate_hotkey ? hotkeyName(settings.translate_hotkey) : null;
  const modeHint =
    mode === "off"
      ? "Bifaria only types what you say."
      : mode === "auto"
        ? `${dictate} translates whenever you speak another language.`
        : translate
          ? `${translate} translates. ${dictate} just dictates.`
          : "Set a translate hotkey below.";
  return (
    <>
      <Title>Languages and hotkeys</Title>
      <Divider p={p} />
      <div style={{ display: "flex", flexDirection: "column", gap: 16 }}>
        <SectionLabel p={p}>DICTATION</SectionLabel>
        <FieldRow p={p} label="Dictate" hint="Types what you say.">
          <HotkeyField p={p} which="main" editing={editing} setEditing={setEditing} />
        </FieldRow>
        <Divider p={p} />
        <SectionLabel p={p}>TRANSLATION</SectionLabel>
        <FieldRow p={p} label="Translate" hint={modeHint}>
          <Segments
            p={p}
            value={mode}
            options={MODE_OPTIONS}
            width={CONTROL_W}
            onChange={(v) => updateSettings({ translation_mode: v })}
          />
        </FieldRow>
        {mode !== "off" && (
          <FieldRow p={p} label="Translate into">
            <select
              value={target}
              onChange={(e) => updateSettings({ target_lang: e.target.value })}
              style={{ ...selectStyle(p), width: CONTROL_W }}
            >
              {(TARGET_LANGUAGES.some((l) => l.value === target)
                ? TARGET_LANGUAGES
                : [...TARGET_LANGUAGES, { value: target, label: target }]
              ).map((l) => (
                <option key={l.value} value={l.value}>
                  {l.label}
                </option>
              ))}
            </select>
          </FieldRow>
        )}
        {mode === "hotkey" && (
          <FieldRow p={p} label="Translate hotkey" hint={`Types it in ${target}.`}>
            <HotkeyField p={p} which="translate" editing={editing} setEditing={setEditing} />
          </FieldRow>
        )}
        {IS_MAC && <Divider p={p} />}
        {IS_MAC && (
          <Fine p={p}>
            Right Option and Right Control work well on their own: few apps use them.
          </Fine>
        )}
      </div>
    </>
  );
}

// ── Where translation happens ───────────────────────────────────────────────

const TRANSLATORS: {
  value: string;
  label: string;
  body: string;
  sub: string;
  apple?: "native" | "ai";
}[] = [
  {
    value: "Apple",
    label: "Apple Native",
    body: "On this Mac. Free, works offline. Language packs download once.",
    sub: "Apple Translation · macOS 26 and later",
    apple: "native",
  },
  {
    value: "Apple Intelligence",
    label: "Apple Intelligence",
    body: "On this Mac. Slower, but can also polish tone and grammar.",
    sub: "macOS 26 and later",
    apple: "ai",
  },
  {
    value: "Offline",
    label: "Offline models",
    body: "On this computer. Translation models download once, then work without internet.",
    sub: "Used when the Apple options aren't available",
  },
  {
    value: "LLM",
    label: "Your own AI model",
    body: "On this computer. An AI app you run yourself, such as Ollama or LM Studio. Set it up in Settings › Translations.",
    sub: "LLM at a local address",
  },
];

/** The settings value's provider, the same way Settings → Translations reads it. */
function providerKey(raw: string | undefined): string {
  const v = (raw || "LLM").toLowerCase();
  return v === "apple"
    ? "Apple"
    : v === "apple intelligence"
      ? "Apple Intelligence"
      : v === "offline"
        ? "Offline"
        : "LLM";
}

/** Language packs into the target language for Apple Native (or the Offline models), one row per
 * pair with a Download link next to it. The same calls as Settings → Translations. */
function LanguagePacks({ p, offline }: { p: Palette; offline: boolean }) {
  const { settings } = useSettingsContext();
  const target = settings.target_lang || "English";
  const [packs, setPacks] = useState<AppleLanguagePack[] | null>(null);
  const [targetCode, setTargetCode] = useState<string | null>(null);
  const [busy, setBusy] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);

  const refresh = useCallback(async () => {
    try {
      const res = await fetchAppleLanguagePacks(target, offline);
      setPacks(res.languages);
      setTargetCode(res.target);
    } catch (e) {
      setError(String(e));
      setPacks(null);
    }
  }, [target, offline]);

  useEffect(() => {
    void refresh();
    // Packs downloaded in System Settings meanwhile show up when the window is back in front.
    const onFocus = () => void refresh();
    window.addEventListener("focus", onFocus);
    return () => window.removeEventListener("focus", onFocus);
  }, [refresh]);

  // While a pack downloads: the bytes so far (Offline reports them; Apple shows its own window)
  // and a clock for the moving bar.
  const [bytes, setBytes] = useState(0);
  const [tick, setTick] = useState(0);
  useEffect(() => {
    if (!busy) return;
    setBytes(0);
    const timer = setInterval(() => setTick((n) => n + 1), 40);
    const unlisten = offline
      ? subscribeToOfflinePackProgress((e) => {
          if (e.pair === `${busy}>${targetCode}`) setBytes(e.bytes);
        })
      : null;
    return () => {
      clearInterval(timer);
      unlisten?.then((fn) => fn?.());
    };
  }, [busy, offline, targetCode]);

  const download = async (pack: AppleLanguagePack) => {
    if (!targetCode) return;
    setBusy(pack.code);
    setError(null);
    try {
      await downloadAppleLanguagePack(pack.code, targetCode, offline);
    } catch (e) {
      if (!/cancel|closed/i.test(String(e))) setError(String(e));
    } finally {
      setBusy(null);
      await refresh();
    }
  };

  const rows = (packs ?? []).filter((x) => x.status !== "unsupported" && x.code !== targetCode);
  // The spoken language when it's set, else the first one.
  const [picked, setPicked] = useState<string | null>(null);
  const pack =
    rows.find((x) => x.code === picked) ??
    rows.find((x) => x.name === settings.source_lang) ??
    rows[0];
  const installed = rows.filter((x) => x.status === "installed");
  return (
    <div style={{ display: "flex", flexDirection: "column", gap: 8, margin: "4px 0 8px 38px" }}>
      <SectionLabel p={p}>LANGUAGE PACK</SectionLabel>
      {error && <span style={{ fontSize: 13, color: p.red }}>{error}</span>}
      {!packs && !error && <Fine p={p}>Loading…</Fine>}
      {pack && (
        <div style={{ display: "flex", alignItems: "center", gap: 10, flexWrap: "wrap" }}>
          <select
            value={pack.code}
            onChange={(e) => setPicked(e.target.value)}
            aria-label="Translate from"
            style={selectStyle(p)}
          >
            {rows.map((x) => (
              <option key={x.code} value={x.code}>
                {x.name}
                {x.status === "installed" ? " ✓" : ""}
              </option>
            ))}
          </select>
          <span style={{ fontSize: 14, color: p.body }}>→ {target}</span>
          {pack.status === "installed" ? (
            <Done p={p}>DOWNLOADED</Done>
          ) : (
            <button
              type="button"
              disabled={busy !== null}
              onClick={() => void download(pack)}
              style={{
                background: "none",
                border: "none",
                padding: 0,
                color: p.accent,
                font: `600 14px ${SANS}`,
                textDecoration: "underline",
                cursor: busy ? "default" : "pointer",
              }}
            >
              {busy === pack.code ? "Downloading…" : "Download"}
            </button>
          )}
        </div>
      )}
      {busy && (
        <div style={{ display: "flex", flexDirection: "column", gap: 6, maxWidth: CONTROL_W }}>
          <div
            style={{
              position: "relative",
              height: 6,
              borderRadius: 3,
              background: p.segOff,
              overflow: "hidden",
            }}
          >
            <div
              style={{
                position: "absolute",
                top: 0,
                bottom: 0,
                width: "30%",
                left: `${((tick * 1.5) % 130) - 30}%`,
                borderRadius: 3,
                background: p.accent,
              }}
            />
          </div>
          <span style={{ font: `500 12px ${MONO}`, color: p.dim }}>
            {offline
              ? `DOWNLOADING · ${formatBytes(bytes)}`
              : "DOWNLOADING · FOLLOW APPLE'S WINDOW"}
          </span>
        </div>
      )}
      {installed.length > 0 && (
        <details>
          <summary
            style={{ cursor: "pointer", fontSize: 13, color: p.dim, listStylePosition: "inside" }}
          >
            {installed.length} downloaded into {target}
          </summary>
          <div
            style={{
              display: "grid",
              gridTemplateColumns: "repeat(3, minmax(0, 1fr))",
              gap: "4px 16px",
              marginTop: 8,
              maxHeight: 140,
              overflowY: "auto",
              fontSize: 13,
              color: p.body,
            }}
          >
            {installed.map((x) => (
              <span key={x.code}>✓ {x.name}</span>
            ))}
          </div>
        </details>
      )}
    </div>
  );
}

function TranslationStep({ p, place }: { p: Palette; place: string }) {
  const { settings, updateSettings } = useSettingsContext();
  const [apple, setApple] = useState<AppleTranslationInfo | null>(null);
  useEffect(() => {
    fetchAppleTranslationInfo()
      .then(setApple)
      .catch(() => setApple(null));
  }, []);
  const active = providerKey(settings.translation_provider);
  const recommended = apple?.available ? "Apple" : "Offline";
  const options = TRANSLATORS.filter(
    (o) =>
      !o.apple ||
      (IS_MAC && apple?.available && (o.apple === "native" || !apple.intelligence_reason)),
  );
  return (
    <>
      <Title>Where translation happens</Title>
      <Lede p={p}>
        This is the privacy choice. By default, translation runs on {place} too, so your words stay
        here.
      </Lede>
      <div role="radiogroup" style={{ display: "flex", flexDirection: "column", gap: 8 }}>
        {options.map((o) => {
          const on = o.value === active;
          return (
            <React.Fragment key={o.value}>
              <button
                type="button"
                role="radio"
                aria-checked={on}
                onClick={() => updateSettings({ translation_provider: o.value })}
                style={{
                  display: "flex",
                  gap: 12,
                  alignItems: "flex-start",
                  padding: "12px 14px",
                  borderRadius: 10,
                  background: on ? p.accentSoft : p.row,
                  border: `1px solid ${on ? p.accentSoftBorder : p.rowBorder}`,
                  color: p.text,
                  textAlign: "left",
                  cursor: "pointer",
                  font: `400 14px ${SANS}`,
                }}
              >
                <span
                  aria-hidden
                  style={{
                    width: 14,
                    height: 14,
                    marginTop: 2,
                    flexShrink: 0,
                    borderRadius: "50%",
                    border: `1.5px solid ${on ? p.accent : p.dashed}`,
                    boxShadow: on ? `inset 0 0 0 3px ${p.card}` : undefined,
                    background: on ? p.accent : "transparent",
                  }}
                />
                <span style={{ display: "flex", flexDirection: "column", gap: 2 }}>
                  <span style={{ display: "flex", alignItems: "center", gap: 8 }}>
                    <span style={{ fontWeight: 600, fontSize: 15 }}>{o.label}</span>
                    {o.value === recommended && (
                      <span style={{ font: `600 11px ${MONO}`, color: p.accentText }}>DEFAULT</span>
                    )}
                  </span>
                  <span style={{ color: p.body, lineHeight: 1.45 }}>{o.body}</span>
                  <span style={{ fontSize: 12.5, color: p.dim }}>{o.sub}</span>
                </span>
              </button>
              {on && (o.value === "Apple" || o.value === "Offline") && (
                <LanguagePacks p={p} offline={o.value === "Offline"} />
              )}
            </React.Fragment>
          );
        })}
      </div>
      <Divider p={p} />
      <Lines
        p={p}
        lines={[
          "Cloud services aren't offered here.",
          `If you connect one later, Bifaria warns you first: text sent there leaves ${place}.`,
        ]}
      />
    </>
  );
}

// ── Text style ──────────────────────────────────────────────────────────────

const STYLE_SAY = "um so I think we can ship it on, uh, friday, no, monday if the the tests pass";
const STYLE_PASTES: Record<Preset, string> = {
  natural: "So I think we can ship it on Friday, no, Monday if the tests pass.",
  ai: "I think we can ship it on Monday if the tests pass.",
  verbatim: STYLE_SAY,
};

function StyleStep({ p }: { p: Palette }) {
  const { settings, updateSettings } = useSettingsContext();
  const current = presetOf(settings);
  const styles = STYLES.filter((s) => s.value !== "custom") as {
    value: Preset;
    label: string;
    hint: string;
  }[];
  const shown: Preset = current === "custom" ? "natural" : current;
  return (
    <>
      <Title>How should your text look?</Title>
      <Lede p={p}>Pick a style for pasted text. Change it any time in Settings › Text.</Lede>
      <Segments
        p={p}
        value={current === "custom" ? ("" as Preset) : current}
        options={styles}
        onChange={(v) => updateSettings(PRESETS[v])}
      />
      <Fine p={p}>
        {styles.find((s) => s.value === current)?.hint ?? "Your own mix, set in Settings › Text."}
      </Fine>
      <Divider p={p} />
      <div style={{ display: "flex", flexDirection: "column", gap: 8, maxWidth: 560 }}>
        <SectionLabel p={p}>YOU SAY</SectionLabel>
        <span style={{ fontSize: 16, fontStyle: "italic", color: p.body }}>“{STYLE_SAY}”</span>
        <SectionLabel p={p}>BIFARIA PASTES</SectionLabel>
        <Row p={p}>
          <span style={{ fontSize: 17 }}>{STYLE_PASTES[shown]}</span>
        </Row>
      </div>
    </>
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
  const [step, setStep] = useState<StepId>("welcome");
  const [t, setT] = useState(0);
  const dl = useModelDownload();
  const place = IS_MAC ? "this Mac" : "this computer";

  // The Translation step is there only while translation is on.
  const steps = STEPS.filter((s) => s !== "translation" || settings.translation_mode !== "off");
  const index = Math.max(0, steps.indexOf(step));
  const counted = steps.filter((s) => STEP_NAMES[s]);
  const go = (i: number) => setStep(steps[Math.max(0, Math.min(steps.length - 1, i))] ?? "welcome");

  // Permissions: opening the microphone is what makes macOS ask for access.
  const [micState, setMicState] = useState<MicState>("off");
  const [micLevel, setMicLevel] = useState(0);
  const micOn = micState !== "off";
  useEffect(() => {
    if (step !== "perms" || !micOn) return;
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

  // Accessibility: macOS doesn't announce the grant, so check while the step is open.
  const [granted, setGranted] = useState<boolean | null>(null);
  useEffect(() => {
    if (step !== "perms" || !IS_MAC) return;
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

  // Animation clock for the pill.
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

  const last = step === "try";
  const permsDone = micState === "allowed" && (!IS_MAC || granted === true);
  const nextLabel =
    step === "welcome"
      ? "Get started"
      : last
        ? "Start using Bifaria"
        : step === "perms" && !permsDone
          ? "Skip for now"
          : "Continue";
  const next = () => (last ? finishOnboarding() : go(index + 1));

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

  const ghost: React.CSSProperties = {
    background: "none",
    border: `1px solid ${p.ghostBorder}`,
    color: p.text,
    borderRadius: 10,
    padding: "10px 16px",
    font: `500 14px ${SANS}`,
    cursor: "pointer",
  };

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
      {/* Header: brand, progress through the counted steps, download status. Also the drag handle. */}
      <div
        data-tauri-drag-region
        style={{
          display: "flex",
          alignItems: "center",
          gap: 16,
          padding: "18px 36px 16px",
          borderBottom: `1px solid ${p.divider}`,
        }}
      >
        <div data-tauri-drag-region style={{ display: "flex", alignItems: "center", gap: 8 }}>
          <AppMark />
          <span data-tauri-drag-region style={{ fontSize: 15, fontWeight: 600 }}>
            Bifaria
          </span>
        </div>
        {STEP_NAMES[step] && (
          <>
            <div style={{ display: "flex", gap: 4 }}>
              {counted.map((id, i) => (
                <button
                  key={id}
                  type="button"
                  onClick={() => setStep(id)}
                  title={STEP_NAMES[id]}
                  aria-label={STEP_NAMES[id]}
                  style={{
                    width: 26,
                    height: 4,
                    padding: 0,
                    border: "none",
                    borderRadius: 2,
                    cursor: "pointer",
                    background:
                      id === step ? p.accent : i < counted.indexOf(step) ? p.text : p.segOff,
                    transition: "background 200ms",
                  }}
                />
              ))}
            </div>
            <span data-tauri-drag-region style={{ font: `500 12px ${MONO}`, color: p.dim }}>
              {counted.indexOf(step) + 1} / {counted.length} · {STEP_NAMES[step]}
            </span>
          </>
        )}
        <div data-tauri-drag-region style={{ flex: 1, alignSelf: "stretch" }} />
        <ModelStatus dl={dl} p={p} />
      </div>

      {/* Body */}
      <div
        style={{
          flex: 1,
          minHeight: 0,
          overflowY: "auto",
          padding: "30px 36px 16px",
          display: "flex",
          flexDirection: "column",
          gap: 14,
        }}
      >
        {step === "welcome" && <WelcomeStep p={p} hotkey={key} place={place} />}
        {step === "perms" && (
          <PermsStep
            p={p}
            micState={micState}
            micLevel={micLevel}
            onAllowMic={() => setMicState("waiting")}
            granted={granted}
            place={place}
          />
        )}
        {step === "model" && <ModelStep p={p} dl={dl} place={place} />}
        {step === "keys" && <KeysStep p={p} />}
        {step === "translation" && <TranslationStep p={p} place={place} />}
        {step === "style" && <StyleStep p={p} />}
        {step === "try" && (
          <>
            <Eyebrow p={p}>ONE MORE THING</Eyebrow>
            <Title>Try it</Title>
            <Lede p={p}>
              {practice
                ? `The speech model is still downloading, so this is a practice run. Press ${key}, say a sentence, press it again.${translateHint} Real recording unlocks when the download finishes.`
                : `The speech model is ready. Press ${key} (or the button), say a sentence, then press it again.${translateHint}`}
            </Lede>
            <div
              style={{
                display: "flex",
                flexDirection: "column",
                alignItems: "center",
                gap: 10,
                padding: 12,
                borderRadius: 14,
                background: p.stage,
                border: `1px solid ${p.stageBorder}`,
              }}
            >
              <button
                type="button"
                onClick={tryCtl.press}
                disabled={talkOff}
                title={
                  waiting ? "Press to talk works once the speech model is downloaded" : undefined
                }
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
                  padding: "8px 14px",
                  font: `500 13px ${SANS}`,
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
              <TryStage t={t} p={p} practice={practice} onState={setTryCtl} />
            </div>
            <Fine p={p}>
              Everything here can be changed later in Settings: General, Transcription,
              Translations, Text, Storage, Updates.
            </Fine>
          </>
        )}
      </div>

      {/* Footer */}
      <div
        style={{
          display: "flex",
          alignItems: "center",
          gap: 10,
          padding: "16px 36px 20px",
          borderTop: `1px solid ${p.divider}`,
        }}
      >
        {/* Closing counts as finishing: the guide doesn't open again on its own. */}
        <button
          type="button"
          onClick={() => finishOnboarding()}
          style={{ ...ghost, border: "none", color: p.dim, padding: "10px 4px" }}
        >
          Finish later
        </button>
        <div style={{ flex: 1 }} />
        {index > 0 && (
          <button type="button" onClick={() => go(index - 1)} style={ghost}>
            Back
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
