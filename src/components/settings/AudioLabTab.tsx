import { useCallback, useEffect, useRef, useState } from "react";
import type { Tokens } from "@/lib/tokens";
import { errorMessage } from "@/lib/utils";
import { Section, Row, FieldSelect, CopyButton } from "./SettingsPrimitives";
import { SELECTABLE } from "./selectable";
import { useSettingsContext } from "./useSettingsContext";
import Toggle from "@/components/ui/Toggle";
import Segmented from "@/components/ui/Segmented";
import AppleTranslationPanel from "./AppleTranslationPanel";
import {
  fetchLabAudioFiles,
  triggerRunLabExperiment,
  triggerSaveLabCustomAudio,
  triggerLabTranslate,
  triggerLabSaveResult,
  fetchAppleTranslationInfo,
  subscribeToLabProgress,
  type LabProgress,
  type LabTranslation,
  isTauri,
  type LabAudioItem,
  type LabExperimentResult,
  NOISE_REDUCTION_OPTIONS,
  type NoiseReduction,
} from "@/lib/tauri";

/** "45 s", "12 min" or "1 h 05 min". */
function formatDuration(ms: number): string {
  const s = Math.max(0, Math.round(ms / 1000));
  if (s < 60) return `${s} s`;
  const m = Math.round(s / 60);
  if (m < 60) return `${m} min`;
  return `${Math.floor(m / 60)} h ${String(m % 60).padStart(2, "0")} min`;
}

const TRANSLATION_SERVICES = [
  { value: "LLM", label: "LLM" },
  { value: "Offline", label: "Offline" },
];
// Offered only where Apple's on-device engines work (macOS 26+).
const APPLE_SERVICES = [
  { value: "Apple", label: "Apple Native" },
  { value: "Apple Intelligence", label: "Apple Intelligence" },
];

const TARGET_LANGUAGES = [
  "English",
  "Ukrainian",
  "Spanish",
  "French",
  "German",
  "Italian",
  "Polish",
  "Japanese",
  "Chinese",
  "Russian",
].map((l) => ({ value: l, label: l }));

/** What a recording opened from Logs said (none if transcription failed), its translation if it
 * was translated, and the Logs entry a fixed result can be saved to. */
export type LabSourceTexts = {
  spoken: string | null;
  translated: string | null;
  logId?: number;
  failure?: "transcription" | "translation" | null;
  failureReason?: string | null;
};

type SaveKind = "transcription" | "translation";

export default function AudioLabTab({
  t,
  initialAudio,
  initialTexts,
}: {
  t: Tokens;
  initialAudio?: string | null;
  initialTexts?: LabSourceTexts | null;
}) {
  const { models, settings, refreshLogs } = useSettingsContext();
  const [items, setItems] = useState<LabAudioItem[]>([]);
  const [selected, setSelected] = useState<string>(initialAudio || "");
  const [model, setModel] = useState(settings.model_name || "ggml-medium-q5_0.bin");
  const [useVad, setUseVad] = useState(true);
  const [language, setLanguage] = useState<string>("auto");
  const [textNorm, setTextNorm] = useState(!!settings.text_normalization);
  const [remFillers, setRemFillers] = useState(!!settings.remove_filler_words);
  const [convNumbers, setConvNumbers] = useState(!!settings.convert_numbers);
  const [remStutters, setRemStutters] = useState(!!settings.remove_stutters);
  const [selfCorr, setSelfCorr] = useState(!!settings.apply_self_corrections);
  const [noiseMarkers, setNoiseMarkers] = useState(!!settings.remove_noise_markers);
  const [collapse, setCollapse] = useState(!!settings.collapse_redundancy);
  const [ambiguity, setAmbiguity] = useState(!!settings.annotate_ambiguity);
  const [structured, setStructured] = useState(!!settings.normalize_structured_values);
  const [noiseReduction, setNoiseReduction] = useState<NoiseReduction>(
    settings.noise_reduction ?? "off",
  );
  const [phase, setPhase] = useState<"idle" | "running" | "done" | "error">("idle");
  const [progress, setProgress] = useState<LabProgress | null>(null);
  // When the run and its transcription started; the time left is estimated from the latter.
  const runStart = useRef(0);
  const transcribeStart = useRef<number | null>(null);
  const [now, setNow] = useState(() => Date.now());

  useEffect(() => {
    if (phase !== "running") return;
    const unlisten = subscribeToLabProgress((p) => {
      if (p.stage === "Transcribing" && transcribeStart.current === null) {
        transcribeStart.current = Date.now();
      }
      setProgress(p);
    });
    const timer = setInterval(() => setNow(Date.now()), 1000);
    return () => {
      unlisten.then((fn) => fn?.());
      clearInterval(timer);
    };
  }, [phase]);
  const [result, setResult] = useState<LabExperimentResult | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [rawUploadName, setRawUploadName] = useState<string | null>(null);
  const [trService, setTrService] = useState(() => {
    const provider = (settings.translation_provider || "").toLowerCase();
    return provider === "apple"
      ? "Apple"
      : provider === "apple intelligence"
        ? "Apple Intelligence"
        : provider === "offline"
          ? "Offline"
          : "LLM";
  });
  const [appleAvailable, setAppleAvailable] = useState(false);
  useEffect(() => {
    fetchAppleTranslationInfo()
      .then((info) => setAppleAvailable(info.available))
      .catch(() => setAppleAvailable(false));
  }, []);
  const trServices = appleAvailable
    ? [...TRANSLATION_SERVICES, ...APPLE_SERVICES]
    : TRANSLATION_SERVICES;
  const [trTarget, setTrTarget] = useState(settings.target_lang || "English");
  const [trFrom, setTrFrom] = useState<"spoken" | "result">("spoken");
  const [trPhase, setTrPhase] = useState<"idle" | "running" | "done" | "error">("idle");
  const [trResult, setTrResult] = useState<LabTranslation | null>(null);
  const [trError, setTrError] = useState<string | null>(null);
  // Text last saved to the Logs entry, per kind, so the button shows "Saved" until it changes.
  const [saved, setSaved] = useState<Partial<Record<SaveKind, string>>>({});
  const [saving, setSaving] = useState<SaveKind | null>(null);
  const [saveError, setSaveError] = useState<{ kind: SaveKind; message: string } | null>(null);
  const fileRef = useRef<HTMLInputElement>(null);

  const availableModels =
    models.length > 0
      ? models.map((m) => ({ value: m.filename, label: m.name }))
      : [
          { value: "ggml-base.bin", label: "Whisper Base (local)" },
          { value: "ggml-medium-q5_0.bin", label: "Whisper Medium" },
          { value: "parakeet-tdt-0.6b-v3", label: "Parakeet TDT 0.6B v3" },
        ];

  const languageOptions = [
    { value: "auto", label: "Auto" },
    { value: "en", label: "English" },
    { value: "uk", label: "Ukrainian" },
    { value: "es", label: "Spanish" },
    { value: "fr", label: "French" },
    { value: "de", label: "German" },
    { value: "ja", label: "Japanese" },
    { value: "zh", label: "Chinese" },
  ];

  const refreshLab = useCallback(
    async (targetAudio?: string | null) => {
      const list = await fetchLabAudioFiles();
      setItems(list);
      setSelected((prev) => {
        const toSelect = targetAudio || initialAudio || prev;
        if (toSelect && list.some((it) => it.filename === toSelect)) return toSelect;
        if (list[0] && !prev) return list[0].filename;
        return prev;
      });
    },
    [initialAudio],
  );

  useEffect(() => {
    refreshLab(initialAudio);
  }, [initialAudio, refreshLab]);

  useEffect(() => {
    setTextNorm(!!settings.text_normalization);
    setRemFillers(!!settings.remove_filler_words);
    setConvNumbers(!!settings.convert_numbers);
    setRemStutters(!!settings.remove_stutters);
    setSelfCorr(!!settings.apply_self_corrections);
    setNoiseMarkers(!!settings.remove_noise_markers);
    setCollapse(!!settings.collapse_redundancy);
    setAmbiguity(!!settings.annotate_ambiguity);
    setStructured(!!settings.normalize_structured_values);
    setModel((prev) => settings.model_name || prev);
  }, [settings]);

  async function handleFile(e: React.ChangeEvent<HTMLInputElement>) {
    const f = e.target.files?.[0];
    if (!f) return;
    if (!isTauri()) {
      setError("Upload works only in the desktop app.");
      return;
    }
    const buf = await f.arrayBuffer();
    const b64 = arrayBufferToBase64(buf);
    try {
      const saved = await triggerSaveLabCustomAudio(b64, f.name);
      setRawUploadName(f.name);
      await refreshLab();
      if (saved) setSelected(saved);
      setError(null);
    } catch (err) {
      setError(errorMessage(err));
    } finally {
      if (fileRef.current) fileRef.current.value = "";
    }
  }

  function handleDrop(e: React.DragEvent) {
    e.preventDefault();
    const f = e.dataTransfer.files?.[0];
    if (f && fileRef.current) {
      const dt = new DataTransfer();
      dt.items.add(f);
      fileRef.current.files = dt.files;
      fileRef.current.dispatchEvent(new Event("change", { bubbles: true }));
    }
  }

  async function run() {
    if (!selected) {
      setError("Pick a recording first.");
      return;
    }
    runStart.current = Date.now();
    transcribeStart.current = null;
    setProgress(null);
    setNow(Date.now());
    setPhase("running");
    setError(null);
    setResult(null);
    try {
      const res = await triggerRunLabExperiment({
        audio_filename: selected,
        model_name: model,
        use_vad: useVad,
        language: language === "auto" ? null : language,
        text_normalization: textNorm,
        remove_filler_words: remFillers,
        convert_numbers: convNumbers,
        remove_stutters: remStutters,
        apply_self_corrections: selfCorr,
        remove_noise_markers: noiseMarkers,
        collapse_redundancy: collapse,
        annotate_ambiguity: ambiguity,
        normalize_structured_values: structured,
        noise_reduction: noiseReduction,
        chunking: !!settings.audio_chunking,
      });
      if (res) {
        setResult(res);
        setPhase("done");
      } else {
        setError("Empty result.");
        setPhase("error");
      }
    } catch (err) {
      setError(errorMessage(err));
      setPhase("error");
    }
  }

  function reset() {
    setPhase("idle");
    setResult(null);
    setError(null);
  }
  const selItem = items.find((x) => x.filename === selected);
  // The texts belong to the recording Second Try opened, not to one picked afterwards.
  const sourceTexts = initialTexts && selected === initialAudio ? initialTexts : null;

  // Translate Again works on what was said (from Second Try) or on this run's transcript.
  const spokenText = sourceTexts?.spoken ?? null;
  const sourceRows: ["spoken" | "translated", string, string][] = [];
  if (sourceTexts?.spoken) sourceRows.push(["spoken", "As spoken", sourceTexts.spoken]);
  if (sourceTexts?.translated)
    sourceRows.push(["translated", "Translated", sourceTexts.translated]);
  const logId = sourceTexts?.logId ?? null;
  const resultText = phase === "done" ? result?.clean_text?.trim() || null : null;
  const trSource = trFrom === "result" && resultText ? resultText : (spokenText ?? resultText);

  /** Saves `text` to the Logs entry this recording was opened from, and to its history row. */
  async function saveResult(kind: SaveKind, text: string, source?: string | null) {
    if (logId === null) return;
    setSaving(kind);
    setSaveError(null);
    try {
      await triggerLabSaveResult(logId, kind, text, source);
      setSaved((prev) => ({ ...prev, [kind]: text }));
      await refreshLogs();
    } catch (err) {
      setSaveError({ kind, message: errorMessage(err) });
    } finally {
      setSaving(null);
    }
  }

  /** "Save to history" for a result of the recording opened from Logs. */
  function saveButton(kind: SaveKind, text: string, source?: string | null) {
    const done = saved[kind] === text;
    const busy = saving === kind;
    return (
      <button
        type="button"
        onClick={() => void saveResult(kind, text, source)}
        disabled={done || busy}
        title={`Replace this recording's ${kind === "translation" ? "translation" : "text"} in Logs and History`}
        style={{
          padding: "3px 9px",
          borderRadius: 6,
          border: `1px solid ${t.accent}40`,
          background: done ? "transparent" : `${t.accent}14`,
          color: done ? t.textDim : t.accent,
          fontSize: 11,
          fontWeight: 600,
          fontFamily: "Inter, sans-serif",
          cursor: done || busy ? "default" : "pointer",
        }}
      >
        {done ? "Saved" : busy ? "Saving…" : "Save to history"}
      </button>
    );
  }

  const saveErrorText = (kind: SaveKind) =>
    saveError?.kind === kind && (
      <p style={{ margin: "0 0 8px", fontSize: 12, color: t.errorColor, lineHeight: 1.5 }}>
        {saveError.message}
      </p>
    );

  async function translateAgain() {
    if (!trSource) return;
    setTrPhase("running");
    setTrError(null);
    setTrResult(null);
    try {
      setTrResult(await triggerLabTranslate(trSource, trService, trTarget));
      setTrPhase("done");
    } catch (err) {
      setTrError(errorMessage(err));
      setTrPhase("error");
    }
  }

  // Under "This Recording" after Second Try; otherwise below this run's results.
  const translateAgainSection = (
    <Section title="Translate Again" t={t}>
      {spokenText && resultText && (
        <Row label="Text" hint="What you said, or this run's transcript" t={t}>
          <Segmented
            options={[
              { value: "spoken", label: "As spoken" },
              { value: "result", label: "This run" },
            ]}
            value={trFrom}
            onChange={(v) => setTrFrom(v as "spoken" | "result")}
            t={t}
          />
        </Row>
      )}
      <Row label="Service" hint="Uses the keys and prompt saved in Translations" t={t}>
        <Segmented options={trServices} value={trService} onChange={setTrService} t={t} />
      </Row>
      <Row label="Translate to" t={t} last>
        <FieldSelect value={trTarget} onChange={setTrTarget} options={TARGET_LANGUAGES} t={t} />
      </Row>
      {(trService === "Apple" || trService === "Offline") && (
        <div style={{ padding: "12px 12px 0" }}>
          <AppleTranslationPanel
            t={t}
            offline={trService === "Offline"}
            targetLang={trTarget}
            packsOnly
          />
        </div>
      )}
      <div style={{ padding: 12, display: "flex", flexDirection: "column", gap: 10 }}>
        <button
          type="button"
          onClick={() => void translateAgain()}
          disabled={!trSource || trPhase === "running"}
          style={{
            padding: "8px 14px",
            borderRadius: 8,
            border: "none",
            background: trPhase === "running" ? t.border : t.accent,
            color: trPhase === "running" ? t.textDim : "#fff",
            fontSize: 12,
            fontWeight: 600,
            fontFamily: "Inter, sans-serif",
            cursor: trPhase === "running" ? "not-allowed" : "pointer",
          }}
        >
          {trPhase === "running" ? "Translating…" : `Translate with ${trService}`}
        </button>
        {!trSource && (
          <p style={{ margin: 0, fontSize: 12, color: t.textMuted, lineHeight: 1.5 }}>
            Nothing to translate yet. Open a recording with Second Try in Logs, or run the
            experiment above.
          </p>
        )}
        {trError && (
          <p style={{ margin: 0, fontSize: 12, color: t.errorColor, lineHeight: 1.5 }}>{trError}</p>
        )}
        {saveErrorText("translation")}
        {trResult && (
          <div
            style={{
              padding: "10px 12px",
              borderRadius: 8,
              background: t.inputBg,
              border: `1px solid ${t.accent}55`,
            }}
          >
            <div
              style={{
                display: "flex",
                alignItems: "center",
                justifyContent: "space-between",
                marginBottom: 6,
              }}
            >
              <span
                style={{
                  fontSize: 10,
                  fontWeight: 600,
                  letterSpacing: "0.08em",
                  textTransform: "uppercase",
                  color: t.accent,
                  fontFamily: "JetBrains Mono, monospace",
                }}
              >
                {trTarget} · {trResult.ms} ms
              </span>
              <div style={{ display: "flex", alignItems: "center", gap: 6 }}>
                {logId !== null && saveButton("translation", trResult.text, trSource)}
                <CopyButton text={trResult.text} t={t} title="Copy translation" />
              </div>
            </div>
            <p
              style={{
                margin: 0,
                fontSize: 12,
                color: t.text,
                fontFamily: "Inter, sans-serif",
                lineHeight: 1.5,
                whiteSpace: "pre-wrap",
                ...SELECTABLE,
              }}
            >
              {trResult.text}
            </p>
            <p
              style={{
                margin: "6px 0 0",
                fontSize: 10,
                color: t.textDim,
                fontFamily: "JetBrains Mono, monospace",
                wordBreak: "break-all",
              }}
            >
              {trResult.service}
            </p>
          </div>
        )}
      </div>
    </Section>
  );

  const metrics = result
    ? [
        { label: "Inference", value: result.inference_time_ms + " ms" },
        { label: "VAD cut", value: result.vad_cut_sec.toFixed(2) + "s" },
        { label: "Speed", value: result.speed_factor.toFixed(1) + "\u00d7" },
        {
          label: "Duration",
          value:
            result.original_duration_sec.toFixed(1) +
            "s \u2192 " +
            result.processed_duration_sec.toFixed(1) +
            "s",
        },
        { label: "Segments", value: String(result.segments.length) },
        { label: "Needs review", value: result.requires_clarification ? "Yes" : "No" },
      ]
    : [];

  return (
    <div>
      {sourceTexts && (
        <Section title="This Recording" t={t}>
          <div style={{ padding: 12, display: "flex", flexDirection: "column", gap: 10 }}>
            {sourceTexts.failure && (
              <div
                style={{
                  padding: "10px 12px",
                  borderRadius: 8,
                  background: `${t.errorColor}10`,
                  border: `1px solid ${t.errorColor}40`,
                  fontSize: 12,
                  lineHeight: 1.5,
                  color: t.text,
                }}
              >
                <b style={{ color: t.errorColor }}>
                  {sourceTexts.failure === "transcription"
                    ? "Transcription failed"
                    : "Translation failed"}
                </b>
                {sourceTexts.failureReason && (
                  <span style={{ color: t.textDim, ...SELECTABLE }}>
                    {" "}
                    · {sourceTexts.failureReason}
                  </span>
                )}
                <p style={{ margin: "6px 0 0", color: t.textMuted }}>
                  {sourceTexts.failure === "transcription"
                    ? "Run the experiment below on this recording, then save the transcript to history."
                    : "Translate it again below, then save the translation to history."}
                </p>
              </div>
            )}
            {sourceRows.map(([which, label, text]) => (
              <div
                key={which}
                style={{
                  padding: "10px 12px",
                  borderRadius: 8,
                  background: t.inputBg,
                  border: `1px solid ${which === "spoken" ? t.accent + "55" : t.border}`,
                }}
              >
                <div
                  style={{
                    display: "flex",
                    alignItems: "center",
                    justifyContent: "space-between",
                    marginBottom: 6,
                  }}
                >
                  <span
                    style={{
                      fontSize: 10,
                      fontWeight: 600,
                      letterSpacing: "0.08em",
                      textTransform: "uppercase",
                      color: which === "spoken" ? t.accent : t.textDim,
                      fontFamily: "JetBrains Mono, monospace",
                    }}
                  >
                    {label}
                  </span>
                  <CopyButton text={text} t={t} title={`Copy ${label.toLowerCase()}`} />
                </div>
                <p
                  style={{
                    margin: 0,
                    fontSize: 12,
                    color: t.text,
                    fontFamily: "Inter, sans-serif",
                    lineHeight: 1.5,
                    whiteSpace: "pre-wrap",
                    ...SELECTABLE,
                  }}
                >
                  {text}
                </p>
              </div>
            ))}
          </div>
        </Section>
      )}

      {spokenText && translateAgainSection}

      <Section title="Source Recording (Real Audio)" t={t}>
        <div style={{ padding: 12 }}>
          {items.length === 0 ? (
            <p style={{ fontSize: 12, color: t.textMuted, margin: 0, lineHeight: 1.5 }}>
              No recordings yet — record with your hotkey or drop a .wav/.mp3 below.
            </p>
          ) : (
            <FieldSelect
              value={selected}
              onChange={setSelected}
              options={items.map((it) => ({
                value: it.filename,
                label:
                  it.name +
                  " \u2014 " +
                  it.duration_sec.toFixed(1) +
                  "s" +
                  (it.is_vad_trimmed ? " \u00b7 VAD" : ""),
              }))}
              t={t}
            />
          )}
          {selItem && (
            <p
              style={{
                fontSize: 11,
                color: t.textDim,
                margin: "8px 0 0",
                fontFamily: "JetBrains Mono, monospace",
              }}
            >
              {selItem.filename} · {(selItem.size_bytes / 1024).toFixed(0)} KB
            </p>
          )}
          <div
            onDragOver={(e) => e.preventDefault()}
            onDrop={handleDrop}
            onClick={() => fileRef.current?.click()}
            style={{
              marginTop: 12,
              border: "2px dashed " + (rawUploadName ? t.accent : t.border),
              borderRadius: 10,
              padding: "16px 14px",
              textAlign: "center",
              cursor: "pointer",
              background: rawUploadName ? t.accent + "08" : t.inputBg,
            }}
          >
            <input
              ref={fileRef}
              type="file"
              accept=".wav,.mp3,.m4a,.mp4,.mov,.aac,.flac,.ogg,.aiff,.aif"
              onChange={handleFile}
              style={{ display: "none" }}
            />
            <div style={{ fontSize: 18, marginBottom: 4 }}>🎙</div>
            <p
              style={{
                fontSize: 12,
                color: rawUploadName ? t.accent : t.textMuted,
                margin: 0,
                fontWeight: rawUploadName ? 600 : 400,
              }}
            >
              {rawUploadName
                ? "Uploaded: " + rawUploadName
                : "Drop an audio file (WAV, MP3, M4A, FLAC, Ogg, AIFF) or click to upload"}
            </p>
            <p style={{ fontSize: 11, color: t.textDim, margin: "4px 0 0" }}>
              Listed as "Opened File"; long files are transcribed in chunks when Audio chunking is
              on
            </p>
          </div>
        </div>
      </Section>

      <Section title="Experiment (Runs on Real Audio)" t={t}>
        <Row label="Model" hint="Whisper GGML or Parakeet — real inference" t={t}>
          <FieldSelect value={model} onChange={setModel} options={availableModels} t={t} />
        </Row>
        <Row label="Language hint" hint="null = auto" t={t}>
          <FieldSelect value={language} onChange={setLanguage} options={languageOptions} t={t} />
        </Row>
        <Row
          label="Noise reduction"
          hint="RNNoise before the model; saved as lab_denoised.wav"
          t={t}
        >
          <Segmented
            options={NOISE_REDUCTION_OPTIONS}
            value={noiseReduction}
            onChange={setNoiseReduction}
            t={t}
          />
        </Row>
        <Row label="VAD trimming" hint="Silero VAD cut + 400ms padding" t={t}>
          <Toggle value={useVad} onChange={setUseVad} accent={t.accent} />
        </Row>
        <Row label="Text normalization master" t={t} last={!textNorm}>
          <Toggle value={textNorm} onChange={setTextNorm} accent={t.accent} />
        </Row>
        {textNorm && (
          <>
            <Row label="Remove filler words" hint="um, uh" t={t}>
              <Toggle value={remFillers} onChange={setRemFillers} accent={t.accent} />
            </Row>
            <Row label="Numbers to digits" hint="twenty four → 24" t={t}>
              <Toggle value={convNumbers} onChange={setConvNumbers} accent={t.accent} />
            </Row>
            <Row label="Remove stutters" hint="the the → the" t={t}>
              <Toggle value={remStutters} onChange={setRemStutters} accent={t.accent} />
            </Row>
          </>
        )}
        <Row label="Self-corrections" hint="sorry, I mean X" t={t}>
          <Toggle value={selfCorr} onChange={setSelfCorr} accent={t.accent} />
        </Row>
        <Row label="Strip noise markers" hint="[noise], (cough)" t={t}>
          <Toggle value={noiseMarkers} onChange={setNoiseMarkers} accent={t.accent} />
        </Row>
        <Row label="Collapse redundancy" hint="merge immediate duplicate phrase" t={t}>
          <Toggle value={collapse} onChange={setCollapse} accent={t.accent} />
        </Row>
        <Row label="Annotate ambiguity" hint="[?]" t={t}>
          <Toggle value={ambiguity} onChange={setAmbiguity} accent={t.accent} />
        </Row>
        <Row label="Structured values" hint="dates, currencies, %" t={t} last>
          <Toggle value={structured} onChange={setStructured} accent={t.accent} />
        </Row>
      </Section>

      <div style={{ display: "flex", gap: 8, marginBottom: 16 }}>
        <button
          type="button"
          onClick={run}
          disabled={!selected || phase === "running"}
          style={{
            flex: 1,
            padding: 10,
            borderRadius: 8,
            border: "none",
            background:
              !selected || phase === "running"
                ? t.surface
                : "linear-gradient(135deg, " + t.accent + " 0%, " + t.accent + "cc 100%)",
            color: !selected || phase === "running" ? t.textDim : "#fff",
            fontSize: 13,
            fontWeight: 600,
            cursor: !selected || phase === "running" ? "not-allowed" : "pointer",
          }}
        >
          {phase === "running"
            ? "\u23f3 Running on real audio\u2026"
            : "\u25b6  Run on real recording"}
        </button>
        {(phase !== "idle" || result) && (
          <button
            type="button"
            onClick={reset}
            style={{
              padding: "10px 14px",
              borderRadius: 8,
              background: t.surface,
              border: "1px solid " + t.border,
              color: t.textMuted,
              fontSize: 12,
              cursor: "pointer",
            }}
          >
            Reset
          </button>
        )}
      </div>

      {error && (
        <div
          style={{
            padding: "10px 12px",
            borderRadius: 8,
            background: t.errorColor + "14",
            border: "1px solid " + t.errorColor + "33",
            color: t.errorColor,
            fontSize: 12,
            marginBottom: 12,
          }}
        >
          {error}
        </div>
      )}

      {phase === "running" && (
        <div style={{ marginBottom: 16 }}>
          <p
            style={{
              fontSize: 11,
              color: t.textMuted,
              fontFamily: "JetBrains Mono, monospace",
              margin: "0 0 6px",
            }}
          >
            Running {model} {useVad ? "+ VAD" : ""} on {selected}\u2026
          </p>
          <div style={{ height: 4, borderRadius: 2, background: t.stepTrack, overflow: "hidden" }}>
            <div
              style={{
                height: "100%",
                // A full bar while the stage can't say how far it is.
                width: `${progress?.fraction != null ? Math.round(progress.fraction * 100) : 100}%`,
                background: "linear-gradient(90deg, " + t.accent + ", #00d4ff)",
                transition: "width 0.4s",
              }}
            />
          </div>
          <p style={{ fontSize: 12, color: t.textMuted, margin: "6px 0 0" }}>
            {(() => {
              const parts = [progress?.stage ?? "Starting"];
              const f = progress?.fraction;
              if (f != null) parts.push(`${Math.floor(f * 100)}%`);
              parts.push(`${formatDuration(now - runStart.current)} elapsed`);
              // Too early to guess before 2%: the first minute includes loading the model.
              if (f != null && f >= 0.02 && f < 1 && transcribeStart.current !== null) {
                const spent = now - transcribeStart.current;
                parts.push(`about ${formatDuration((spent * (1 - f)) / f)} left`);
              }
              return parts.join(" \u00b7 ");
            })()}
          </p>
        </div>
      )}

      {result && phase === "done" && (
        <div>
          <Section
            title="Raw Whisper"
            t={t}
            action={<CopyButton text={result.raw_text || ""} t={t} title="Copy raw transcript" />}
          >
            <div style={{ padding: 12 }}>
              <p
                style={{
                  fontSize: 11,
                  color: t.textDim,
                  fontFamily: "JetBrains Mono, monospace",
                  margin: "0 0 6px",
                  wordBreak: "break-all",
                  ...SELECTABLE,
                }}
              >
                {result.raw_output}
              </p>
              <p
                style={{
                  fontSize: 13,
                  color: t.text,
                  lineHeight: 1.6,
                  margin: 0,
                  whiteSpace: "pre-wrap",
                  borderTop: "1px solid " + t.border,
                  paddingTop: 8,
                  ...SELECTABLE,
                }}
              >
                {result.raw_text || "(empty)"}
              </p>
            </div>
          </Section>
          <Section
            title="Clean (Post-Processed)"
            t={t}
            action={
              <div style={{ display: "flex", alignItems: "center", gap: 6 }}>
                {logId !== null &&
                  sourceTexts?.failure === "transcription" &&
                  resultText &&
                  saveButton("transcription", resultText)}
                <CopyButton text={result.clean_text || ""} t={t} title="Copy clean text" />
              </div>
            }
          >
            <div style={{ padding: 12 }}>
              {saveErrorText("transcription")}
              <p
                style={{
                  fontSize: 13,
                  color: t.text,
                  lineHeight: 1.7,
                  margin: 0,
                  whiteSpace: "pre-wrap",
                  ...SELECTABLE,
                }}
              >
                {result.clean_text || "(post blanked it)"}
              </p>
              {(result.uncertain_spans?.length ?? 0) > 0 && (
                <div
                  style={{
                    marginTop: 10,
                    padding: 8,
                    borderRadius: 6,
                    background: t.warnColor + "14",
                    border: "1px solid " + t.warnColor + "33",
                  }}
                >
                  <p
                    style={{ fontSize: 11, color: t.warnColor, margin: "0 0 4px", fontWeight: 600 }}
                  >
                    \u26a0 Uncertain spans
                  </p>
                  {result.uncertain_spans.map((u: string, i: number) => (
                    <p
                      key={i}
                      style={{
                        fontSize: 11,
                        color: t.warnColor,
                        margin: "2px 0",
                        fontFamily: "JetBrains Mono, monospace",
                        ...SELECTABLE,
                      }}
                    >
                      · {u}
                    </p>
                  ))}
                </div>
              )}
              <p
                style={{
                  fontSize: 11,
                  color: t.textDim,
                  margin: "8px 0 0",
                  fontFamily: "JetBrains Mono, monospace",
                }}
              >
                Post: {(result.post_applied || []).join(", ") || "none"} · lang:{" "}
                {result.detected_lang} · {result.hardware_engine}
              </p>
            </div>
          </Section>
          <Section
            title="Metrics"
            t={t}
            action={
              <CopyButton
                text={metrics.map((m) => `${m.label}: ${m.value}`).join("\n")}
                t={t}
                title="Copy metrics"
              />
            }
          >
            <div style={{ display: "grid", gridTemplateColumns: "repeat(3,1fr)" }}>
              {metrics.map((m, i) => (
                <div
                  key={m.label}
                  style={{
                    padding: "10px 12px",
                    textAlign: "center",
                    borderRight: (i + 1) % 3 !== 0 ? "1px solid " + t.border : "none",
                    borderBottom: i < 3 ? "1px solid " + t.border : "none",
                  }}
                >
                  <p
                    style={{
                      fontSize: 14,
                      fontWeight: 700,
                      color: t.accent,
                      margin: "0 0 2px",
                      ...SELECTABLE,
                    }}
                  >
                    {m.value}
                  </p>
                  <p
                    style={{
                      fontSize: 9,
                      color: t.textDim,
                      fontFamily: "JetBrains Mono, monospace",
                      letterSpacing: "0.06em",
                      textTransform: "uppercase",
                      margin: 0,
                    }}
                  >
                    {m.label}
                  </p>
                </div>
              ))}
            </div>
          </Section>
          {result.segments?.length > 0 && (
            <Section
              title="Segments"
              t={t}
              action={
                <CopyButton
                  text={result.segments
                    .map(
                      (s) =>
                        `[${(s.start_ms / 1000).toFixed(2)}–${(s.end_ms / 1000).toFixed(2)}s] ${s.text.trim()}`,
                    )
                    .join("\n")}
                  t={t}
                  title="Copy all segments"
                />
              }
            >
              <div style={{ padding: 8, maxHeight: 160, overflow: "auto" }}>
                {result.segments.slice(0, 20).map((s, i) => (
                  <p
                    key={i}
                    style={{
                      fontSize: 11,
                      color: t.textMuted,
                      fontFamily: "JetBrains Mono, monospace",
                      margin: "4px 0",
                      ...SELECTABLE,
                    }}
                  >
                    [{(s.start_ms / 1000).toFixed(2)}–{(s.end_ms / 1000).toFixed(2)}s ·{" "}
                    {(s.no_speech_prob * 100).toFixed(0)}%] {s.text}
                  </p>
                ))}
              </div>
            </Section>
          )}
        </div>
      )}

      {!spokenText && translateAgainSection}
    </div>
  );
}

function arrayBufferToBase64(buf: ArrayBuffer): string {
  const bytes = new Uint8Array(buf);
  let binary = "";
  for (let i = 0; i < bytes.length; i++) binary += String.fromCharCode(bytes[i]!);
  return btoa(binary);
}
