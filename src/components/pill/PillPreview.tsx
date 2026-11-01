import { useEffect, useState } from "react";
import type { AudioLevels } from "@/lib/tauri";
import type { Tokens } from "@/lib/tokens";
import { PillBody } from "./styles";
import type { PillState, PillStyleId, PillView } from "./view";

/** The demo recording: how long each stage stays on screen, in ms. */
const STAGES: { state: PillState; ms: number }[] = [
  { state: "listening", ms: 3600 },
  { state: "transcribing", ms: 1500 },
  { state: "translating", ms: 1500 },
  { state: "done", ms: 1600 },
];
const CYCLE_MS = STAGES.reduce((sum, s) => sum + s.ms, 0);
/** The demo recording starts at 0:07, so the timer ticks over visibly. */
const DEMO_START_SEC = 7;

function stageAt(ms: number): { state: PillState; into: number } {
  let rest = ms % CYCLE_MS;
  for (const s of STAGES) {
    if (rest < s.ms) return { state: s.state, into: rest };
    rest -= s.ms;
  }
  return { state: "listening", into: 0 };
}

/** Made-up speech for the bars: a few syllables a second, louder in the middle bands. */
function fakeLevels(ms: number): AudioLevels {
  const syllable = Math.abs(Math.sin(ms / 170)) * Math.abs(Math.cos(ms / 610));
  const volume = 0.12 + 0.6 * syllable;
  const bands = [0, 1, 2, 3, 4, 5, 6].map((i) => {
    const centre = 1 - Math.abs(i - 3) / 4;
    return Math.min(1, centre * (0.6 + 0.4 * Math.abs(Math.sin(ms / 230 + i * 1.3))));
  });
  return { volume, bands };
}

/** One style playing a whole recording on a loop: listening, transcribing, translating, done. */
export function PillPreview({ style, t }: { style: PillStyleId; t: Tokens }) {
  const [now, setNow] = useState(0);
  useEffect(() => {
    const start = performance.now();
    const id = setInterval(() => setNow(performance.now() - start), 60);
    return () => clearInterval(id);
  }, []);

  const { state, into } = stageAt(now);
  const accent =
    state === "transcribing"
      ? "#00d4ff"
      : state === "translating"
        ? "#e58bdb"
        : state === "done"
          ? "#00e5a0"
          : t.accent;
  const view: PillView = {
    state,
    accent,
    title: {
      listening: "Listening",
      transcribing: "Transcribing",
      translating: "Translating…",
      done: "Copied to clipboard",
      error: "Error",
    }[state],
    subtitle: {
      listening: "Translates if not English",
      transcribing: "Converting speech to text",
      translating: "Ukrainian → English (Gemini)",
      done: "Text in clipboard",
      error: "",
    }[state],
    elapsedSec: DEMO_START_SEC + Math.floor(into / 1000),
    heardSec: DEMO_START_SEC + Math.floor((STAGES[0]?.ms ?? 0) / 1000),
    run: null,
    targetLang: "English",
    sourceLang: state === "translating" ? "Ukrainian" : null,
    micWarning: false,
    long: false,
    retry: false,
  };
  return (
    <PillBody
      style={style}
      view={view}
      t={t}
      levels={state === "listening" ? fakeLevels(now) : undefined}
    />
  );
}
