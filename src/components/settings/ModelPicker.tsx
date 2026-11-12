import { useEffect, useLayoutEffect, useMemo, useRef, useState } from "react";
import { createPortal } from "react-dom";
import type { Tokens } from "@/lib/tokens";
import type { ModelCatalogItem } from "@/lib/tauri";

/** Display names of the model families, in the order the list shows them. */
const FAMILIES: [string, string][] = [
  ["whisper", "Whisper"],
  ["parakeet", "Parakeet"],
  ["canary", "Canary"],
  ["canary_qwen", "Canary-Qwen"],
  ["moonshine", "Moonshine"],
  ["moonshine_streaming", "Moonshine Streaming"],
  ["qwen3_asr", "Qwen3-ASR"],
  ["cohere_asr", "Cohere Transcribe"],
  ["voxtral", "Voxtral"],
  ["voxtral_realtime", "Voxtral Realtime"],
  ["granite_speech", "Granite Speech"],
  ["granite_speech_nar", "Granite Speech"],
  ["granite5_ctc", "Granite Speech 5"],
  ["gigaam", "GigaAM"],
  ["funasr_nano", "Fun-ASR"],
  ["sensevoice", "SenseVoice"],
  ["medasr", "MedASR"],
  ["moss", "MOSS"],
  ["custom", "Your models"],
];

function familyName(family: string): string {
  const known = FAMILIES.find(([id]) => id === family)?.[1];
  return known ?? family.charAt(0).toUpperCase() + family.slice(1).replace(/_/g, " ");
}

function familyOrder(family: string): number {
  const i = FAMILIES.findIndex(([id]) => id === family);
  return i < 0 ? FAMILIES.length - 1 : i;
}

const languageNames = (() => {
  try {
    return new Intl.DisplayNames(["en"], { type: "language" });
  } catch {
    return null;
  }
})();

function languageName(code: string): string {
  try {
    return languageNames?.of(code) ?? code;
  } catch {
    return code;
  }
}

/** "English", "Ukrainian, Russian" or "25 languages". */
function languagesLabel(codes: string[]): string {
  if (codes.length === 0) return "";
  if (codes.length <= 2) return codes.map(languageName).join(", ");
  return `${codes.length} languages`;
}

/** Every word of the query appears in the model's name, family, languages or labels. */
function matches(m: ModelCatalogItem, query: string): boolean {
  const haystack = [
    m.name,
    familyName(m.family),
    m.speed_desc,
    ...m.tags,
    ...m.languages,
    ...m.languages.map(languageName),
  ]
    .join(" ")
    .toLowerCase();
  return query
    .toLowerCase()
    .split(/\s+/)
    .filter(Boolean)
    .every((word) => haystack.includes(word));
}

/**
 * The speech model picker: every model this computer can run, grouped by family, with what each
 * one transcribes, its size, where it runs and whether it's downloaded. Less common models wait
 * behind "Show all models"; search looks through all of them.
 */
export default function ModelPicker({
  models,
  value,
  onChange,
  t,
}: {
  models: ModelCatalogItem[];
  value: string;
  onChange: (filename: string) => void;
  t: Tokens;
}) {
  const [open, setOpen] = useState(false);
  const [query, setQuery] = useState("");
  const [showAll, setShowAll] = useState(false);
  const [active, setActive] = useState(0);
  const rootRef = useRef<HTMLDivElement>(null);
  const popRef = useRef<HTMLDivElement>(null);
  const searchRef = useRef<HTMLInputElement>(null);
  const listRef = useRef<HTMLDivElement>(null);
  const [place, setPlace] = useState<{
    left: number;
    width: number;
    top?: number;
    bottom?: number;
    maxHeight: number;
  } | null>(null);

  const selected = models.find((m) => m.filename === value);
  // A label every model has (e.g. "GPU") tells nothing apart: rows leave it out.
  const commonTags = useMemo(
    () =>
      models.length > 1
        ? (models[0]?.tags ?? []).filter((tag) => models.every((m) => m.tags.includes(tag)))
        : [],
    [models],
  );

  // The list floats over the window, below the button or above it, whichever has more room, so
  // the settings card around the button can't cut it off.
  useLayoutEffect(() => {
    if (!open) return;
    const placeIt = () => {
      const r = rootRef.current?.getBoundingClientRect();
      if (!r) return;
      const margin = 12;
      const width = Math.min(460, window.innerWidth - 2 * margin);
      const left = Math.max(margin, Math.min(r.right - width, window.innerWidth - width - margin));
      const below = window.innerHeight - r.bottom - margin - 6;
      const above = r.top - margin - 6;
      setPlace(
        below >= 360 || below >= above
          ? { left, width, top: r.bottom + 6, maxHeight: Math.min(520, below) }
          : {
              left,
              width,
              bottom: window.innerHeight - r.top + 6,
              maxHeight: Math.min(520, above),
            },
      );
    };
    placeIt();
    window.addEventListener("resize", placeIt);
    window.addEventListener("scroll", placeIt, true);
    return () => {
      window.removeEventListener("resize", placeIt);
      window.removeEventListener("scroll", placeIt, true);
    };
  }, [open]);

  // Recommended, downloaded and the selected model show by default; a search covers every model.
  const visible = useMemo(() => {
    const shown = models.filter((m) =>
      query ? matches(m, query) : showAll || m.curated || m.downloaded || m.filename === value,
    );
    return [...shown].sort((a, b) => familyOrder(a.family) - familyOrder(b.family));
  }, [models, query, showAll, value]);
  const hidden =
    models.length - models.filter((m) => m.curated || m.downloaded || m.filename === value).length;

  useEffect(() => {
    if (!open) return;
    setActive(
      Math.max(
        0,
        visible.findIndex((m) => m.filename === value),
      ),
    );
    searchRef.current?.focus();
    const close = (e: MouseEvent) => {
      const target = e.target as Node;
      if (!rootRef.current?.contains(target) && !popRef.current?.contains(target)) setOpen(false);
    };
    document.addEventListener("mousedown", close);
    return () => document.removeEventListener("mousedown", close);
    // Only when it opens: typing keeps the highlight where the keyboard put it.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [open]);

  useEffect(() => {
    listRef.current
      ?.querySelector(`[data-index="${active}"]`)
      ?.scrollIntoView({ block: "nearest" });
  }, [active]);

  const choose = (m: ModelCatalogItem) => {
    onChange(m.filename);
    setOpen(false);
    setQuery("");
  };

  const onKeyDown = (e: React.KeyboardEvent) => {
    if (e.key === "ArrowDown") {
      e.preventDefault();
      setActive((i) => Math.min(i + 1, visible.length - 1));
    } else if (e.key === "ArrowUp") {
      e.preventDefault();
      setActive((i) => Math.max(i - 1, 0));
    } else if (e.key === "Enter") {
      e.preventDefault();
      const m = visible[active];
      if (m) choose(m);
    } else if (e.key === "Escape") {
      e.preventDefault();
      setOpen(false);
    }
  };

  const chip = (label: string, color?: string) => (
    <span
      key={label}
      style={{
        fontSize: 10,
        fontWeight: 500,
        padding: "1px 6px",
        borderRadius: 4,
        background: color ? `${color}22` : t.surfaceHover,
        color: color ?? t.textMuted,
        whiteSpace: "nowrap",
      }}
    >
      {label}
    </span>
  );

  let lastFamily = "";
  return (
    <div ref={rootRef} style={{ position: "relative", width: 340, maxWidth: "100%" }}>
      <button
        type="button"
        aria-haspopup="listbox"
        aria-expanded={open}
        onClick={() => setOpen((o) => !o)}
        style={{
          width: "100%",
          display: "flex",
          alignItems: "center",
          justifyContent: "space-between",
          gap: 8,
          background: t.inputBg,
          border: `1px solid ${open ? t.accent : t.inputBorder}`,
          borderRadius: 8,
          padding: "7px 10px 7px 12px",
          color: t.inputText,
          fontSize: 13,
          fontWeight: 450,
          fontFamily: "Inter, -apple-system, sans-serif",
          cursor: "pointer",
          boxShadow: open ? `0 0 0 2px ${t.accent}22` : "none",
          textAlign: "left",
        }}
      >
        <span style={{ overflow: "hidden", textOverflow: "ellipsis", whiteSpace: "nowrap" }}>
          {selected ? selected.name : models.length ? "Choose a model" : "Loading models…"}
        </span>
        <span aria-hidden style={{ color: t.textDim, fontSize: 10 }}>
          ▼
        </span>
      </button>

      {open &&
        place &&
        createPortal(
          <div
            ref={popRef}
            onKeyDown={onKeyDown}
            style={{
              position: "fixed",
              left: place.left,
              width: place.width,
              top: place.top,
              bottom: place.bottom,
              maxHeight: place.maxHeight,
              zIndex: 1000,
              display: "flex",
              flexDirection: "column",
              // A step lighter than the window, so it reads as a layer above it.
              backgroundColor: t.modalBg,
              backgroundImage: `linear-gradient(${t.surfaceActive}, ${t.surfaceActive})`,
              border: `1px solid ${t.inputBorder}`,
              borderRadius: 12,
              boxShadow: "0 18px 48px rgba(0,0,0,0.45), 0 2px 8px rgba(0,0,0,0.25)",
              overflow: "hidden",
              fontFamily: "Inter, -apple-system, sans-serif",
            }}
          >
            <div style={{ padding: 8, borderBottom: `1px solid ${t.border}`, flexShrink: 0 }}>
              <input
                ref={searchRef}
                value={query}
                onChange={(e) => {
                  setQuery(e.target.value);
                  setActive(0);
                }}
                placeholder="Search models, languages, GPU…"
                aria-label="Search models"
                style={{
                  width: "100%",
                  boxSizing: "border-box",
                  background: t.inputBg,
                  border: `1px solid ${t.inputBorder}`,
                  borderRadius: 6,
                  padding: "6px 10px",
                  color: t.inputText,
                  fontSize: 12,
                  outline: "none",
                }}
              />
            </div>
            <div
              ref={listRef}
              role="listbox"
              aria-label="Speech models"
              style={{ flex: 1, minHeight: 0, overflowY: "auto", padding: 4 }}
            >
              {visible.length === 0 && (
                <div style={{ padding: 16, fontSize: 12, color: t.textDim, textAlign: "center" }}>
                  No model matches “{query}”
                </div>
              )}
              {visible.map((m, i) => {
                const header = m.family !== lastFamily;
                lastFamily = m.family;
                const isSelected = m.filename === value;
                const languages = languagesLabel(m.languages);
                return (
                  <div key={m.filename}>
                    {header && (
                      <div
                        style={{
                          padding: "8px 8px 4px",
                          fontSize: 10,
                          fontWeight: 600,
                          letterSpacing: 0.4,
                          textTransform: "uppercase",
                          color: t.textDim,
                        }}
                      >
                        {familyName(m.family)}
                      </div>
                    )}
                    <div
                      role="option"
                      aria-selected={isSelected}
                      data-index={i}
                      onMouseEnter={() => setActive(i)}
                      onClick={() => choose(m)}
                      title={
                        m.languages.length > 2
                          ? m.languages.map(languageName).join(", ")
                          : undefined
                      }
                      style={{
                        display: "flex",
                        alignItems: "center",
                        gap: 8,
                        padding: "7px 8px",
                        borderRadius: 6,
                        cursor: "pointer",
                        background:
                          i === active
                            ? t.surfaceHover
                            : isSelected
                              ? `${t.accent}14`
                              : "transparent",
                      }}
                    >
                      <span aria-hidden style={{ width: 12, color: t.accent, fontSize: 12 }}>
                        {isSelected ? "✓" : ""}
                      </span>
                      <div style={{ flex: 1, minWidth: 0 }}>
                        <div
                          style={{
                            fontSize: 13,
                            color: t.text,
                            overflow: "hidden",
                            textOverflow: "ellipsis",
                            whiteSpace: "nowrap",
                          }}
                        >
                          {m.name}
                        </div>
                        <div
                          style={{
                            fontSize: 11,
                            color: t.textDim,
                            overflow: "hidden",
                            textOverflow: "ellipsis",
                            whiteSpace: "nowrap",
                          }}
                        >
                          {[languages, m.size_desc, m.speed_desc].filter(Boolean).join(" · ")}
                        </div>
                      </div>
                      <div style={{ display: "flex", gap: 4, flexShrink: 0, alignItems: "center" }}>
                        {m.tags.filter((tag) => !commonTags.includes(tag)).map((tag) => chip(tag))}
                        {m.downloaded ? chip("Downloaded", t.successColor) : null}
                      </div>
                    </div>
                  </div>
                );
              })}
            </div>
            {!query && hidden > 0 && (
              <button
                type="button"
                onClick={() => setShowAll((s) => !s)}
                style={{
                  width: "100%",
                  flexShrink: 0,
                  padding: "8px 12px",
                  border: "none",
                  borderTop: `1px solid ${t.border}`,
                  background: "transparent",
                  color: t.accent,
                  fontSize: 12,
                  cursor: "pointer",
                  textAlign: "left",
                }}
              >
                {showAll ? "Show recommended models only" : `Show all ${models.length} models`}
              </button>
            )}
          </div>,
          document.body,
        )}
    </div>
  );
}
