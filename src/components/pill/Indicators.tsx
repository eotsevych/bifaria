import { useEffect, useRef } from "react";
import type { AudioLevels } from "../../lib/tauri";
import type { Tokens } from "../../lib/tokens";
import { OrbitDots, ScanWave } from "./Motion";
import { VOICE_CLOSE, VOICE_OPEN, getBarVoiceScale } from "./voiceLevel";

// Nine capsule towers. The centre tower follows low pitch (voice), the edges follow high pitch
// ("s", "t", breath); the backend sends seven bands already mirrored in that order, and the bars
// in between blend their neighbours (see bandAt).
const BAR_COUNT = 9;
const BAR_WIDTH = 4;
const BAR_GAP = 3;
const BAR_MIN_H = 8; // never collapse, so the capsule shape stays visible
const BAR_MAX_H = 29; // stays clear of the pill edge
const BAR_GROWTH = 20; // extra height at full energy and volume
const BAR_OVERSHOOT_MAX = 2; // spring overshoot when a loud word starts
const WAVE_BOX_H = BAR_MAX_H + BAR_OVERSHOOT_MAX + 1;
// Bars rest flat at BAR_MIN_H until a voice is detected (see VOICE_OPEN). The gate closes once
// volume stays below VOICE_CLOSE for VOICE_HOLD_MS, so pauses between words don't flicker.
const VOICE_HOLD_MS = 200;
// Falling bars keep 80% of their height per 60 fps frame; rises are instant.
const FALL_KEEP_PER_FRAME = 0.8;
const FRAME_MS = 1000 / 60;
// Loudness where the glow and colour shift start, and where they reach full strength.
const LOUD_START = 0.35;
const LOUD_FULL = 0.8;

/** Bar geometry, so the smaller pill styles can show the same live bars. */
export interface WaveSize {
  count: number;
  width: number;
  gap: number;
  min: number;
  max: number;
  growth: number;
}

const CLASSIC_WAVE: WaveSize = {
  count: BAR_COUNT,
  width: BAR_WIDTH,
  gap: BAR_GAP,
  min: BAR_MIN_H,
  max: BAR_MAX_H,
  growth: BAR_GROWTH,
};

const GRADIENT_QUIET = ["#9a8cff", "#7c6fff", "#5b4fd6"] as const;
const GRADIENT_LOUD = ["#c4b8ff", "#a593ff", "#7c6fff"] as const;

/** Energy for bar `i` of `count`, interpolated across the backend's bands. */
function bandAt(bands: number[], i: number, count: number): number {
  if (bands.length < 2) return bands[0] ?? 0;
  const pos = (i * (bands.length - 1)) / (count - 1);
  const lo = Math.floor(pos);
  const frac = pos - lo;
  return (bands[lo] ?? 0) * (1 - frac) + (bands[Math.min(lo + 1, bands.length - 1)] ?? 0) * frac;
}

function mixHex(a: string, b: string, t: number): string {
  const pa = parseInt(a.slice(1), 16);
  const pb = parseInt(b.slice(1), 16);
  const ch = (shift: number) =>
    Math.round(((pa >> shift) & 255) + (((pb >> shift) & 255) - ((pa >> shift) & 255)) * t);
  return `rgb(${ch(16)}, ${ch(8)}, ${ch(0)})`;
}

function barGradient(loud: number): string {
  const [top, mid, bottom] = GRADIENT_QUIET.map((c, i) => mixHex(c, GRADIENT_LOUD[i] ?? c, loud));
  return `linear-gradient(180deg, ${top} 0%, ${mid} 50%, ${bottom} 100%)`;
}

export function SoundWave({
  levels,
  size = CLASSIC_WAVE,
  gradient,
}: {
  levels?: AudioLevels | undefined;
  size?: WaveSize;
  /** A fixed bar colour instead of the blue that brightens with loudness. */
  gradient?: string | undefined;
}) {
  const barsRef = useRef<(HTMLDivElement | null)[]>([]);
  const levelsRef = useRef<AudioLevels | undefined>(levels);
  levelsRef.current = levels;
  const { count, width, gap, min, max, growth } = size;

  useEffect(() => {
    const heights = new Array<number>(count).fill(min);
    let voice = false;
    let quietSince = 0;
    let loud = 0;
    let paintedLoud = -1;
    let last = performance.now();

    let frame = requestAnimationFrame(function tick(now) {
      const dt = Math.min(64, now - last);
      last = now;
      const fallKeep = Math.pow(FALL_KEEP_PER_FRAME, dt / FRAME_MS);
      const rawVolume = levelsRef.current?.volume ?? 0;
      const bands = levelsRef.current?.bands ?? [];
      const scale = getBarVoiceScale();

      if (rawVolume >= VOICE_OPEN) {
        voice = true;
        quietSince = 0;
      } else if (voice && rawVolume < VOICE_CLOSE) {
        if (!quietSince) quietSince = now;
        if (now - quietSince >= VOICE_HOLD_MS) voice = false;
      } else if (!voice) {
        quietSince = 0;
      }
      const silent = !voice;

      // Volume above the gate, so the bars start from zero.
      const cleanVolume = silent ? 0 : Math.max(0, (rawVolume - VOICE_CLOSE) / (1 - VOICE_CLOSE));

      // Glow and colour shift follow loudness with the same fast-rise, slow-fall feel.
      const loudTarget = silent
        ? 0
        : Math.min(1, Math.max(0, (cleanVolume - LOUD_START) / (LOUD_FULL - LOUD_START)));
      loud = loudTarget > loud ? loudTarget : loud * fallKeep + loudTarget * (1 - fallKeep);

      for (let i = 0; i < count; i++) {
        const band = bandAt(bands, i, count);

        // The scale boosts peaks only; quiet bands stay close to the original response.
        const peakFactor = band * Math.min(1, cleanVolume * 2.5);
        const effectiveScale = 1 + (scale - 1) * peakFactor;
        const peakVolume = Math.min(1, cleanVolume * effectiveScale);

        const target = silent ? min : min + band * peakVolume * growth;
        const cur = heights[i] ?? min;
        let next: number;
        if (target > cur) {
          // Instant rise, with a small spring overshoot on the onset of a loud word.
          const jump = target - cur;
          next =
            target + (!silent && loudTarget > 0 ? Math.min(BAR_OVERSHOOT_MAX, jump * 0.15) : 0);
        } else {
          next = cur * fallKeep + target * (1 - fallKeep);
        }
        // Settle exactly at rest in silence, without a last-pixel jitter.
        if (silent && next <= min + 0.1) {
          next = min;
        }
        next = Math.min(max + BAR_OVERSHOOT_MAX, Math.max(min, next));
        heights[i] = next;
        const bar = barsRef.current[i];
        if (bar) bar.style.height = `${next.toFixed(2)}px`;
      }

      if (Math.abs(loud - paintedLoud) > 0.02) {
        paintedLoud = loud;
        const background = gradient ?? barGradient(loud);
        const glow =
          loud > 0.01 ? `0 0 10px rgba(4, 202, 255, ${(0.7 * loud).toFixed(3)})` : "none";
        for (const bar of barsRef.current) {
          if (!bar) continue;
          bar.style.background = background;
          bar.style.boxShadow = glow;
        }
      }

      frame = requestAnimationFrame(tick);
    });
    return () => cancelAnimationFrame(frame);
  }, [count, min, max, growth, gradient]);

  return (
    <div
      style={{ display: "flex", alignItems: "center", height: max + BAR_OVERSHOOT_MAX + 1, gap }}
    >
      {Array.from({ length: count }, (_, i) => (
        <div
          key={i}
          ref={(el) => {
            barsRef.current[i] = el;
          }}
          style={{
            width,
            height: min,
            borderRadius: width,
            background: gradient ?? barGradient(0),
            willChange: "height",
          }}
        />
      ))}
    </div>
  );
}

export function CopiedIndicator() {
  return (
    <svg width="26" height="26" viewBox="0 0 22 22" fill="none">
      <rect
        x="7"
        y="2"
        width="11"
        height="13"
        rx="2.5"
        stroke="rgba(0,229,160,0.4)"
        strokeWidth="1.3"
      />
      <rect
        x="4"
        y="7"
        width="11"
        height="13"
        rx="2.5"
        stroke="#00e5a0"
        strokeWidth="1.3"
        fill="rgba(0,229,160,0.08)"
      />
      <path
        className="check-path"
        d="M7 14l3 3 6-6.5"
        stroke="#00e5a0"
        strokeWidth="1.5"
        strokeLinecap="round"
        strokeLinejoin="round"
      />
    </svg>
  );
}

export function RecordingErrorIndicator({ accent }: { accent?: string | undefined }) {
  const color = accent || "#ff4d6d";
  return (
    <div
      className="flex items-center justify-center rounded-full"
      style={{
        width: 32,
        height: 32,
        background: `${color}18`,
        border: `1px solid ${color}44`,
      }}
    >
      <svg
        width="18"
        height="18"
        viewBox="0 0 24 24"
        fill="none"
        stroke={color}
        strokeWidth="2.2"
        strokeLinecap="round"
        strokeLinejoin="round"
      >
        <line x1="2" y1="2" x2="22" y2="22" />
        <path d="M18.89 13.23A7.12 7.12 0 0 0 19 12v-2" />
        <path d="M5 10v2a7 7 0 0 0 12 5" />
        <line x1="12" y1="19" x2="12" y2="22" />
      </svg>
    </div>
  );
}

export function TranscriptionErrorIndicator({ accent }: { accent?: string | undefined }) {
  const color = accent || "#ff6b6b";
  return (
    <div
      className="flex items-center justify-center rounded-full"
      style={{
        width: 32,
        height: 32,
        background: `${color}18`,
        border: `1px solid ${color}44`,
      }}
    >
      <svg
        width="18"
        height="18"
        viewBox="0 0 24 24"
        fill="none"
        stroke={color}
        strokeWidth="2.2"
        strokeLinecap="round"
        strokeLinejoin="round"
      >
        <circle cx="12" cy="12" r="10" />
        <line x1="12" y1="8" x2="12" y2="12" />
        <line x1="12" y1="16" x2="12.01" y2="16" />
      </svg>
    </div>
  );
}

export function TranslationErrorIndicator({ accent }: { accent?: string | undefined }) {
  const color = accent || "#ff9f43";
  return (
    <div
      className="flex items-center justify-center rounded-full relative"
      style={{
        width: 32,
        height: 32,
        background: `${color}18`,
        border: `1px solid ${color}44`,
      }}
    >
      <svg
        width="18"
        height="18"
        viewBox="0 0 24 24"
        fill="none"
        stroke={color}
        strokeWidth="2.2"
        strokeLinecap="round"
        strokeLinejoin="round"
      >
        <circle cx="12" cy="12" r="10" />
        <line x1="2" y1="12" x2="22" y2="12" />
        <path d="M12 2a15.3 15.3 0 0 1 4 10 15.3 15.3 0 0 1-4 10 15.3 15.3 0 0 1-4-10 15.3 15.3 0 0 1 4-10z" />
      </svg>
      <div
        style={{
          position: "absolute",
          top: -2,
          right: -2,
          width: 8,
          height: 8,
          borderRadius: "50%",
          background: color,
        }}
      />
    </div>
  );
}

export function ErrorIndicator({
  t,
  title,
  accent,
}: {
  t: Tokens;
  title?: string | undefined;
  accent?: string | undefined;
}) {
  const lower = (title || "").toLowerCase();
  if (lower.includes("translat")) return <TranslationErrorIndicator accent={accent} />;
  if (lower.includes("record")) return <RecordingErrorIndicator accent={accent} />;
  if (lower.includes("transcrib") || lower.includes("speech"))
    return <TranscriptionErrorIndicator accent={accent} />;
  return (
    <div
      className="flex items-center justify-center rounded-full"
      style={{
        width: 32,
        height: 32,
        background: t.dangerBg,
        border: `1px solid ${t.dangerBorder}`,
      }}
    >
      <span style={{ color: t.dangerText, fontSize: 14, fontWeight: 700 }}>!</span>
    </div>
  );
}

/** The microphone stopped delivering sound while listening. */
export type MicDownMode = "reconnecting" | "down";

/**
 * Shown instead of the sound wave when the microphone stops delivering sound mid-recording: the seven
 * bars lie flat and dimmed with a strike through them. While Bifaria reopens the mic they pulse one
 * after another; once it gives up they stay still.
 */
export function MicDownIndicator({ mode, accent }: { mode: MicDownMode; accent: string }) {
  const reconnecting = mode === "reconnecting";
  const width = BAR_COUNT * BAR_WIDTH + (BAR_COUNT - 1) * BAR_GAP;
  return (
    <div
      role="img"
      aria-label={reconnecting ? "Microphone reconnecting" : "Microphone not delivering sound"}
      style={{
        position: "relative",
        display: "flex",
        alignItems: "center",
        height: WAVE_BOX_H,
        gap: BAR_GAP,
      }}
    >
      {Array.from({ length: BAR_COUNT }, (_, i) => (
        <div
          key={i}
          className={reconnecting ? "mic-search-bar" : undefined}
          style={{
            width: BAR_WIDTH,
            height: BAR_MIN_H,
            borderRadius: BAR_WIDTH,
            background: "#8a90a6",
            opacity: reconnecting ? undefined : 0.35,
            animationDelay: reconnecting ? `${i * 0.12}s` : undefined,
          }}
        />
      ))}
      {/* The strike-through: a slanted line across the flattened bars */}
      <div
        style={{
          position: "absolute",
          left: -3,
          top: "50%",
          width: width + 6,
          height: 2,
          borderRadius: 2,
          background: accent,
          transform: "translateY(-50%) rotate(-24deg)",
          boxShadow: `0 0 0 1.5px ${accent}33`,
        }}
      />
    </div>
  );
}

export function StateIndicator({
  state,
  t,
  accent,
  levels,
  title,
  micDown,
}: {
  state: "idle" | "listening" | "transcribing" | "translating" | "done" | "error";
  t: Tokens;
  accent: string;
  levels?: AudioLevels | undefined;
  title?: string | undefined;
  micDown?: MicDownMode | undefined;
}) {
  if (state === "listening" && micDown) return <MicDownIndicator mode={micDown} accent={accent} />;
  if (state === "listening") return <SoundWave levels={levels} />;
  // The same scanning wave and orbit as the smaller styles, at the width of the nine bars.
  if (state === "transcribing")
    return <ScanWave color={accent} n={BAR_COUNT} w={BAR_WIDTH} g={BAR_GAP} amp={16} />;
  if (state === "translating") return <OrbitDots color={accent} r={22} d={9} />;
  if (state === "error") return <ErrorIndicator t={t} title={title} accent={accent} />;
  return <CopiedIndicator />;
}
