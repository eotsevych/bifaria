import { useEffect, useRef, useState, type CSSProperties, type ReactNode } from "react";
import type { AudioLevels } from "@/lib/tauri";
import type { Tokens } from "@/lib/tokens";
import { SoundWave, type WaveSize } from "./Indicators";
import { CopyIcon, OrbitDots, RecDot, ResultIcon, ScanWave, Spinner } from "./Motion";
import { ClassicPill } from "./ClassicPill";
import {
  TRANSLATE_ACCENT,
  errorReason,
  formatTimer,
  languageCode,
  type PillStyleId,
  type PillView,
} from "./view";

const MONO = "JetBrains Mono, monospace";
const ORANGE_BARS = "linear-gradient(180deg, #ffb98c, #ff8a4c)";

// Bars of the two-line and single-line styles (in a 36 / 30 px well) and of the capsule.
const WELL_WAVE: WaveSize = { count: 5, width: 2.5, gap: 2, min: 3, max: 18, growth: 15 };
const LINE_WAVE: WaveSize = { count: 5, width: 2.5, gap: 2, min: 3, max: 16, growth: 13 };
const CAPSULE_WAVE: WaveSize = { count: 9, width: 3, gap: 3, min: 3, max: 20, growth: 17 };

/** Accent at about 14% opacity (accents are #rrggbb). */
const soft = (c: string) => `${c}24`;

const center: CSSProperties = { display: "flex", alignItems: "center", justifyContent: "center" };

/** The pill itself: fully rounded and as wide as its content. */
function Shell({
  t,
  accent,
  height,
  padding,
  gap,
  title,
  children,
  onHover,
}: {
  t: Tokens;
  accent: string;
  height: number;
  padding: string;
  gap: number;
  title?: string | undefined;
  children: ReactNode;
  onHover?: (hovered: boolean) => void;
}) {
  return (
    <div
      data-tauri-drag-region
      title={title}
      onMouseEnter={onHover && (() => onHover(true))}
      onMouseLeave={onHover && (() => onHover(false))}
      className="select-none cursor-grab active:cursor-grabbing"
      style={{
        display: "flex",
        alignItems: "center",
        gap,
        height,
        padding,
        borderRadius: height / 2,
        background: t.pillBg,
        border: `1px solid ${t.pillBorder}`,
        boxShadow: [`0 0 0 1px ${t.pillInset} inset`, t.pillShadow, `0 0 16px ${accent}22`].join(
          ", ",
        ),
        boxSizing: "border-box",
        whiteSpace: "nowrap",
        transition: "box-shadow 0.3s ease, background 0.3s ease",
      }}
    >
      {children}
    </div>
  );
}

function statusWord(view: PillView): string {
  switch (view.state) {
    case "listening":
      return "LISTENING";
    case "transcribing":
      return "TRANSCRIBING";
    case "translating":
      return "TRANSLATING";
    case "done":
      return "COPIED";
    default:
      return "FAILED";
  }
}

const labelStyle = (accent: string): CSSProperties => ({
  font: `600 10.5px ${MONO}`,
  letterSpacing: "0.08em",
  color: accent,
  transition: "color 0.3s ease",
});

/** "UK → EN" as two small chips; the source only when it is known. */
function LangChips({ view, t, roomy }: { view: PillView; t: Tokens; roomy?: boolean }) {
  const pad = roomy ? "2px 5px" : "1px 4px";
  const source = view.state === "translating" ? view.sourceLang : null;
  return (
    <div
      style={{
        display: "flex",
        alignItems: "center",
        gap: roomy ? 4 : 3,
        font: `600 10px ${MONO}`,
        color: t.text,
      }}
    >
      {source && (
        <span style={{ padding: pad, borderRadius: 4, border: `1px solid ${t.pillBorder}` }}>
          {languageCode(source)}
        </span>
      )}
      <span style={{ color: t.textMuted }}>→</span>
      <span
        style={{ padding: pad, borderRadius: 4, background: soft(view.accent), color: view.accent }}
      >
        {languageCode(view.targetLang)}
      </span>
    </div>
  );
}

function AutoChip({ view }: { view: PillView }) {
  return (
    <span
      title={`Auto-translate to ${view.targetLang}`}
      style={{
        font: `600 9.5px ${MONO}`,
        color: TRANSLATE_ACCENT,
        border: `1px solid ${TRANSLATE_ACCENT}88`,
        padding: "0 4px",
        borderRadius: 4,
      }}
    >
      AUTO→{languageCode(view.targetLang)}
    </span>
  );
}

/** The visual of the icon-well styles. */
function WellVisual({
  view,
  levels,
  wave,
}: {
  view: PillView;
  levels?: AudioLevels | undefined;
  wave: WaveSize;
}) {
  const { accent } = view;
  const small = wave === LINE_WAVE;
  switch (view.state) {
    case "listening":
      return (
        <SoundWave
          levels={levels}
          size={wave}
          gradient={view.run === "translate" ? ORANGE_BARS : undefined}
        />
      );
    case "transcribing":
      return <ScanWave color={accent} n={5} w={2.5} g={2} amp={9} />;
    case "translating":
      return <OrbitDots color={accent} r={small ? 7 : 8} d={small ? 5 : 6} />;
    case "done":
      return <ResultIcon kind="check" color={accent} size={small ? 18 : 20} />;
    default:
      return <ResultIcon kind="x" color={accent} size={small ? 18 : 20} />;
  }
}

/** Compact: status on top, timer / languages / short detail centred underneath (design 3a and 3b). */
function TwoLinePill({
  view,
  t,
  levels,
  well,
}: {
  view: PillView;
  t: Tokens;
  levels?: AudioLevels | undefined;
  well: boolean;
}) {
  const { accent } = view;
  const text: CSSProperties = { fontSize: 12.5, color: t.text, fontFamily: "Inter, sans-serif" };
  let detail: ReactNode;
  if (view.state === "listening") {
    detail = (
      <>
        <span
          style={{ font: `500 12px ${MONO}`, color: t.text, fontVariantNumeric: "tabular-nums" }}
        >
          {formatTimer(view.elapsedSec)}
        </span>
        {view.run === "translate" && <LangChips view={view} t={t} />}
        {view.run === "auto" && <AutoChip view={view} />}
      </>
    );
  } else if (view.state === "transcribing") {
    detail = (
      <span style={text}>
        {view.heardSec != null ? `${formatTimer(view.heardSec)} of audio` : "Converting speech"}
      </span>
    );
  } else if (view.state === "translating") {
    detail = <LangChips view={view} t={t} />;
  } else if (view.state === "done") {
    detail = <span style={text}>In clipboard</span>;
  } else {
    detail = (
      <span style={{ ...text, overflow: "hidden", textOverflow: "ellipsis" }}>
        {errorReason(view.rawTitle, view.subtitle)}
      </span>
    );
  }
  return (
    <Shell
      t={t}
      accent={accent}
      height={48}
      padding={well ? "0 16px 0 6px" : "0 16px 0 12px"}
      gap={10}
      title={view.subtitle}
    >
      <div
        style={
          well
            ? {
                ...center,
                width: 36,
                height: 36,
                borderRadius: 18,
                background: soft(accent),
                flexShrink: 0,
              }
            : { ...center, width: 28, flexShrink: 0 }
        }
      >
        <WellVisual view={view} levels={levels} wave={WELL_WAVE} />
      </div>
      <div
        style={{
          display: "flex",
          flexDirection: "column",
          alignItems: "center",
          gap: 2,
          minWidth: 0,
        }}
      >
        <span style={labelStyle(accent)}>{statusWord(view)}</span>
        <div
          style={{
            display: "flex",
            alignItems: "center",
            justifyContent: "center",
            gap: 6,
            height: 16,
            minWidth: 0,
          }}
        >
          {detail}
        </div>
      </div>
    </Shell>
  );
}

/** Single line: well, status, languages or a short note, then the timer (design 2b). */
function LinePill({
  view,
  t,
  levels,
}: {
  view: PillView;
  t: Tokens;
  levels?: AudioLevels | undefined;
}) {
  const { accent } = view;
  const showLangs =
    view.state === "translating" || (view.state === "listening" && view.run === "translate");
  const note =
    view.state === "done"
      ? "In clipboard"
      : view.state === "error"
        ? errorReason(view.rawTitle, view.subtitle)
        : null;
  return (
    <Shell t={t} accent={accent} height={40} padding="0 14px 0 5px" gap={9} title={view.subtitle}>
      <div
        style={{
          ...center,
          width: 30,
          height: 30,
          borderRadius: 15,
          background: soft(accent),
          flexShrink: 0,
        }}
      >
        <WellVisual view={view} levels={levels} wave={LINE_WAVE} />
      </div>
      <span style={labelStyle(accent)}>{statusWord(view)}</span>
      {showLangs && <LangChips view={view} t={t} roomy />}
      {view.state === "listening" && view.run === "auto" && <AutoChip view={view} />}
      {note && (
        <span
          style={{
            fontSize: 12.5,
            color: t.textMuted,
            overflow: "hidden",
            textOverflow: "ellipsis",
            fontFamily: "Inter, sans-serif",
          }}
        >
          {note}
        </span>
      )}
      {view.state === "listening" && (
        <span
          style={{
            font: `500 11.5px ${MONO}`,
            color: t.textMuted,
            fontVariantNumeric: "tabular-nums",
          }}
        >
          {formatTimer(view.elapsedSec)}
        </span>
      )}
    </Shell>
  );
}

/** Capsule: one shape morphs through the stages; text only when it adds something (design 5a). */
function CapsulePill({
  view,
  t,
  levels,
}: {
  view: PillView;
  t: Tokens;
  levels?: AudioLevels | undefined;
}) {
  const [hovered, setHovered] = useState(false);
  const { accent } = view;
  let shape: ReactNode;
  let slot: string | null = null;
  switch (view.state) {
    case "listening":
      shape = (
        <SoundWave
          levels={levels}
          size={CAPSULE_WAVE}
          gradient={view.run === "translate" ? ORANGE_BARS : undefined}
        />
      );
      if (view.run === "translate") slot = `→ ${languageCode(view.targetLang)}`;
      else if (hovered || view.elapsedSec >= 10) slot = formatTimer(view.elapsedSec);
      break;
    case "transcribing":
      shape = <ScanWave color={accent} n={9} w={3} g={3} amp={11} />;
      break;
    case "translating":
      shape = <OrbitDots color={accent} r={14} d={7} />;
      slot = view.sourceLang
        ? `${languageCode(view.sourceLang)} → ${languageCode(view.targetLang)}`
        : `→ ${languageCode(view.targetLang)}`;
      break;
    case "done":
      shape = <CopyIcon color={accent} size={20} />;
      break;
    default:
      shape = <ResultIcon kind="x" color={accent} size={20} />;
  }
  return (
    <div style={{ display: "flex", flexDirection: "column", alignItems: "center", gap: 6 }}>
      <Shell
        t={t}
        accent={accent}
        height={36}
        padding="0 18px"
        gap={8}
        title={view.subtitle}
        onHover={setHovered}
      >
        <div style={{ ...center, gap: 8 }}>
          {shape}
          {slot && (
            <span
              style={{
                font: `600 11px ${MONO}`,
                color: accent,
                fontVariantNumeric: "tabular-nums",
              }}
            >
              {slot}
            </span>
          )}
        </div>
      </Shell>
      {view.state === "error" && (
        <span
          style={{
            fontSize: 11.5,
            color: accent,
            whiteSpace: "nowrap",
            fontFamily: "Inter, sans-serif",
          }}
        >
          {view.rawTitle || "Something went wrong"}
        </span>
      )}
    </div>
  );
}

/** Dot: a status mark and one short word (design 5b). */
function DotPill({
  view,
  t,
  levels,
}: {
  view: PillView;
  t: Tokens;
  levels?: AudioLevels | undefined;
}) {
  const { accent } = view;
  let mark: ReactNode;
  let word: string;
  let chip: ReactNode = null;
  switch (view.state) {
    case "listening":
      mark = <RecDot levels={levels} color={accent} size={16} />;
      word = formatTimer(view.elapsedSec);
      if (view.run === "translate")
        chip = (
          <span
            style={{
              font: `600 9.5px ${MONO}`,
              color: accent,
              background: soft(accent),
              padding: "1px 4px",
              borderRadius: 4,
            }}
          >
            {languageCode(view.targetLang)}
          </span>
        );
      else if (view.run === "auto") chip = <AutoChip view={view} />;
      break;
    case "transcribing":
      mark = <Spinner color={accent} />;
      word = view.heardSec != null ? formatTimer(view.heardSec) : "…";
      break;
    case "translating":
      mark = <OrbitDots color={accent} r={5} d={5} />;
      word = view.sourceLang
        ? `${languageCode(view.sourceLang)} → ${languageCode(view.targetLang)}`
        : `→ ${languageCode(view.targetLang)}`;
      break;
    case "done":
      mark = <CopyIcon color={accent} size={15} />;
      word = "Copied";
      break;
    default:
      mark = <ResultIcon kind="x" color={accent} size={14} />;
      word = errorReason(view.rawTitle, view.subtitle);
  }
  return (
    <Shell t={t} accent={accent} height={28} padding="0 12px 0 8px" gap={8} title={view.subtitle}>
      <div style={{ ...center, width: 16, height: 16, flexShrink: 0 }}>{mark}</div>
      <span
        style={{ font: `500 11.5px ${MONO}`, color: t.text, fontVariantNumeric: "tabular-nums" }}
      >
        {word}
      </span>
      {chip}
    </Shell>
  );
}

/** One moment of the pill, drawn in the chosen style. */
export function PillBody({
  style,
  view,
  t,
  levels,
}: {
  style: PillStyleId;
  view: PillView;
  t: Tokens;
  levels?: AudioLevels | undefined;
}) {
  // Anything that has to be read or clicked gets the full pill, whatever the style.
  if (style === "classic" || view.long || view.micWarning || view.retry || view.devices)
    return <ClassicPill view={view} t={t} levels={levels} />;
  switch (style) {
    case "compact":
      return <TwoLinePill view={view} t={t} levels={levels} well />;
    case "compact_plain":
      return <TwoLinePill view={view} t={t} levels={levels} well={false} />;
    case "line":
      return <LinePill view={view} t={t} levels={levels} />;
    case "capsule":
      return <CapsulePill view={view} t={t} levels={levels} />;
    case "dot":
      return <DotPill view={view} t={t} levels={levels} />;
  }
}
