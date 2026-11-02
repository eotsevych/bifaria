import type { MicDownMode } from "./Indicators";

export type PillState = "listening" | "transcribing" | "translating" | "done" | "error";

/** Everything a pill style needs to draw one moment of a recording. */
export interface PillView {
  state: PillState;
  accent: string;
  /** The status line of the Classic pill. */
  title: string;
  /** The backend's own title (picks the error icon and the short error reason). */
  rawTitle?: string | undefined;
  subtitle: string;
  /** Seconds recorded so far (listening). */
  elapsedSec: number;
  /** Length of the finished recording, once processing has started. */
  heardSec: number | null;
  /** What this recording does with the speech, while it runs. */
  run: "translate" | "auto" | null;
  targetLang: string;
  /** The spoken language, when known (shown while translating). */
  sourceLang: string | null;
  micWarning: boolean;
  micDown?: MicDownMode | undefined;
  /** A message to read in full: always shown in the Classic pill, which grows to fit it. */
  long: boolean;
  /** A failed translation with a Retry button: always shown in the Classic pill. */
  retry: boolean;
  /** The chosen microphone isn't connected: these connected mics are offered instead. */
  devices?: string[] | undefined;
}

/** The pill designs offered in Settings → Theme, largest first. */
export const PILL_STYLES = [
  {
    id: "classic",
    hint: "Status, detail, timer",
    name: "Classic",
    description: "Status, what happens to your speech, and the timer. Says the most.",
  },
  {
    id: "compact",
    hint: "Two lines, icon well",
    name: "Compact",
    description: "Two short lines beside a tinted icon well; the timer sits under the status.",
  },
  {
    id: "compact_plain",
    hint: "Two lines, no well",
    name: "Compact, no well",
    description: "Compact with the visualizer straight on the pill, closer to Classic.",
  },
  {
    id: "line",
    hint: "One row",
    name: "Single line",
    description: "Status, languages and timer on one short row.",
  },
  {
    id: "capsule",
    hint: "No words",
    name: "Capsule",
    description: "Status without words: live bars morph into a scanning wave, then a check.",
  },
  {
    id: "dot",
    hint: "Smallest",
    name: "Dot",
    description: "The smallest: a record dot that swells with your voice and one short word.",
  },
] as const;

export type PillStyleId = (typeof PILL_STYLES)[number]["id"];

export function pillStyleOf(value: string | undefined): PillStyleId {
  return PILL_STYLES.find((s) => s.id === value)?.id ?? "classic";
}

/** Color of a recording started with the translate hotkey, from listening through translating: Bifaria's orange. */
export const TRANSLATE_ACCENT = "#ff8a4c";

const LANGUAGE_CODES: Record<string, string> = {
  english: "EN",
  ukrainian: "UK",
  spanish: "ES",
  french: "FR",
  german: "DE",
  italian: "IT",
  polish: "PL",
  japanese: "JA",
  chinese: "ZH",
  russian: "RU",
};

export function languageCode(name: string): string {
  return LANGUAGE_CODES[name.trim().toLowerCase()] ?? name.trim().slice(0, 2).toUpperCase();
}

export function formatTimer(secs: number): string {
  const mins = Math.floor(secs / 60);
  const s = secs % 60;
  return `${mins}:${s < 10 ? "0" : ""}${s}`;
}

/** The languages in a translating subtitle such as "Ukrainian → English (Ollama)". */
export function parseDirection(subtitle: string | null | undefined): [string, string] | null {
  const m = /^(.+?) → (.+?)(?: \(.*\))?$/.exec(subtitle ?? "");
  return m && m[1] && m[2] ? [m[1], m[2]] : null;
}

/** One or two words for an error, for the styles without room for the backend's message. */
export function errorReason(title?: string, subtitle?: string): string {
  const all = `${title ?? ""} ${subtitle ?? ""}`.toLowerCase();
  const head = (title ?? "").toLowerCase();
  if (all.includes("no speech") || all.includes("too short") || all.includes("silence"))
    return "No speech";
  if (head.includes("timed out")) return "Timed out";
  if (head.includes("download")) return "Download";
  if (head.includes("translat")) return "Translation";
  if (head.includes("record")) return "Recording";
  if (head.includes("transcri")) return "Transcription";
  return title || "Error";
}
