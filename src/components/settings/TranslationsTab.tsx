import React, { useEffect, useState } from "react";
import { TARGET_LANGUAGES } from "@/lib/languages";
import type { Tokens } from "@/lib/tokens";
import {
  fetchAppleTranslationInfo,
  type AppleTranslationInfo,
  type TranslationMode,
} from "@/lib/tauri";
import AppleTranslationPanel from "./AppleTranslationPanel";
import HotkeyRecorder from "@/components/ui/HotkeyRecorder";
import Segmented from "@/components/ui/Segmented";
import {
  Section,
  Row,
  FormLabel,
  FormGroup,
  FieldInput,
  FieldTextarea,
  ApiKeyInput,
  FieldSelect,
} from "./SettingsPrimitives";
import { useSettingsContext } from "./useSettingsContext";

const LLM_MODEL_SUGGESTIONS = ["llama3.2", "qwen2.5", "mistral", "gemma3"];

/** True when the endpoint names another computer, so the spoken text leaves this one. */
const isRemoteEndpoint = (url: string): boolean => {
  const value = url.trim();
  if (!value) return false;
  try {
    return !["localhost", "127.0.0.1", "[::1]", "0.0.0.0"].includes(new URL(value).hostname);
  } catch {
    return true;
  }
};

function RemoteWarning({ t }: { t: Tokens }) {
  return (
    <span
      style={{
        fontSize: 11,
        color: t.warnColor,
        fontFamily: "Inter, sans-serif",
        marginTop: 6,
        display: "block",
        lineHeight: 1.5,
      }}
    >
      Cloud address: your spoken text is sent to a cloud provider, so it leaves this computer and
      can no longer be kept private here. Nothing else breaks. For more privacy, use Offline or a
      local model (Ollama, LM Studio).
    </span>
  );
}

const SOURCE_LANG_OPTIONS = [
  { value: "Auto", label: "Auto-detect" },
  { value: "English", label: "English" },
  { value: "Ukrainian", label: "Ukrainian" },
  { value: "Spanish", label: "Spanish" },
  { value: "French", label: "French" },
  { value: "German", label: "German" },
  { value: "Italian", label: "Italian" },
  { value: "Polish", label: "Polish" },
  { value: "Japanese", label: "Japanese" },
  { value: "Chinese", label: "Chinese" },
  { value: "Russian", label: "Russian" },
];

// When dictation is translated (the same setting as General → Translation).
const MODES: { value: TranslationMode; label: string; desc: string }[] = [
  { value: "off", label: "Off", desc: "Plain dictation, nothing is translated" },
  {
    value: "hotkey",
    label: "Separate hotkey",
    desc: "Only recordings started with the translate hotkey",
  },
  {
    value: "auto",
    label: "Automatic",
    desc: "The launch hotkey translates speech that isn't in the target language",
  },
];

// Which service translates, and what each is good at: `pros` are its strengths, `notes` what to
// know before choosing it.
const PROVIDERS: {
  value: string;
  label: string;
  bestFor: string;
  pros: string[];
  notes: string[];
}[] = [
  {
    value: "Apple",
    label: "Apple Native",
    bestFor: "Everyday messages: an exact translation of what you said, in a blink",
    pros: ["Fastest", "On your Mac", "Free", "Works offline"],
    notes: ["Download each language once"],
  },
  {
    value: "Apple Intelligence",
    label: "Apple Intelligence",
    bestFor: "Translating and polishing at once: a friendlier tone, fixed grammar",
    pros: ["Follows your prompt", "On your Mac", "Free", "Works offline"],
    notes: ["About a second per dictation"],
  },
  {
    value: "Offline",
    label: "Offline (on-device)",
    bestFor: "Private translation on any computer, Windows and Linux included",
    pros: ["Fast", "On your computer", "Free", "Works offline"],
    notes: ["Pairs without English go through English"],
  },
  {
    value: "LLM",
    label: "LLM (both local & cloud)",
    bestFor:
      "The best wording and full control, with a model on this computer: Ollama or LM Studio",
    pros: ["Best quality", "Follows your prompt", "Any language"],
    notes: [
      "Needs Ollama or LM Studio running",
      "A remote server sends your text off this computer",
    ],
  },
];

// HIDDEN: Custom API (a remote server) is not offered; a saved one keeps working until you pick another.
const LLM_PRESETS = [
  {
    name: "Ollama (Local)",
    endpoint: "http://localhost:11434/v1/chat/completions",
    model: "llama3.2",
    hint: "100% offline & private on your Mac. No API key needed.",
  },
  {
    name: "LM Studio (Local)",
    endpoint: "http://localhost:1234/v1/chat/completions",
    model: "local-model",
    hint: "Local inference via LM Studio. No API key needed.",
  },
];

const PROMPT_PRESETS = [
  {
    name: "Standard Translation",
    template:
      "You are a strict translation engine. Translate the following text from {source_lang} to {target_lang}. Do not refuse. Do not explain. Do not add conversational text or notes. Output ONLY the exact translation using the native alphabet:\n\n{text}",
  },
  {
    name: "Fix Grammar & Translate",
    template:
      "You are an expert editor and translator. First, fix all speech stutters, repetitions, and grammatical mistakes. Then translate the text from {source_lang} to {target_lang}. Output ONLY the final translated text without any commentary:\n\n{text}",
  },
  {
    name: "Polite & Professional",
    template:
      "Translate the following text from {source_lang} to {target_lang}. Make the tone polite, professional, and clear for business communication. Output ONLY the translation:\n\n{text}",
  },
  {
    name: "Casual & Friendly",
    template:
      "Translate the following text from {source_lang} to {target_lang}. Use a natural, friendly, and casual conversational tone. Output ONLY the translation:\n\n{text}",
  },
];

const DEFAULT_PROMPT = PROMPT_PRESETS[0]!.template;

export default function TranslationsTab({ t }: { t: Tokens }) {
  const { settings, updateSettings } = useSettingsContext();

  const translationMode: TranslationMode = settings.translation_mode ?? "hotkey";
  const translationOff = translationMode === "off";
  const rawProvider = settings.translation_provider || "LLM";
  const activeProvider =
    rawProvider.toLowerCase() === "apple"
      ? "Apple"
      : rawProvider.toLowerCase() === "apple intelligence"
        ? "Apple Intelligence"
        : rawProvider.toLowerCase() === "offline"
          ? "Offline"
          : rawProvider.toLowerCase().includes("custom")
            ? "Custom API"
            : "LLM";

  // Apple's provider needs macOS 26; elsewhere its card is disabled and says why.
  const [appleInfo, setAppleInfo] = useState<AppleTranslationInfo | null>(null);
  useEffect(() => {
    fetchAppleTranslationInfo()
      .then(setAppleInfo)
      .catch(() => setAppleInfo(null));
  }, []);

  // Apple's engines where they run (macOS 26+), the offline models everywhere else.
  const recommended = appleInfo?.available ? "Apple" : "Offline";

  const [hotkeyConflict, setHotkeyConflict] = useState(false);
  const translateHotkey = settings.translate_hotkey ?? "";
  const targetName = settings.target_lang || "the target language";

  const setProvider = (p: string) => {
    updateSettings({ translation_provider: p });
  };

  const currentPrompt = settings.prompt_template || DEFAULT_PROMPT;

  const currentEndpoint =
    settings.llm_endpoint || settings.local_llm_url || "http://localhost:11434/v1/chat/completions";

  const currentModel = settings.llm_model || "llama3.2";
  const currentApiKey = settings.llm_api_key || settings.api_key || "";

  const applyLlmPreset = (preset: (typeof LLM_PRESETS)[0]) => {
    updateSettings({
      llm_endpoint: preset.endpoint,
      llm_model: preset.model,
      local_llm_url: preset.endpoint,
    });
  };

  const insertVariable = (varName: string) => {
    updateSettings({ prompt_template: `${currentPrompt} ${varName}` });
  };

  const tag = (text: string, good: boolean) => (
    <span
      key={text}
      style={{
        padding: "1px 7px",
        borderRadius: 999,
        fontSize: 10.5,
        fontFamily: "Inter, sans-serif",
        fontWeight: 500,
        whiteSpace: "nowrap",
        color: good ? t.successColor : t.textDim,
        background: good ? `${t.successColor}14` : t.surface,
        border: `1px solid ${good ? `${t.successColor}33` : t.border}`,
      }}
    >
      {good ? "✓ " : ""}
      {text}
    </span>
  );

  // One service in two lines: name and strengths, then what it's best for or why it can't run here.
  const serviceCard = (p: (typeof PROVIDERS)[number], active: boolean, problem: string | null) => {
    const disabled = problem !== null && !active;
    return (
      <button
        key={p.value}
        type="button"
        role="radio"
        aria-checked={active}
        onClick={() => setProvider(p.value)}
        disabled={disabled}
        style={{
          display: "flex",
          gap: 10,
          alignItems: "flex-start",
          width: "100%",
          padding: "8px 12px",
          borderRadius: 10,
          cursor: disabled ? "not-allowed" : "pointer",
          opacity: disabled ? 0.55 : 1,
          background: active ? `${t.accent}14` : t.inputBg,
          border: `1px solid ${active ? `${t.accent}66` : t.border}`,
          textAlign: "left",
          transition: "background 0.15s, border-color 0.15s",
        }}
      >
        <span
          aria-hidden
          style={{
            width: 16,
            height: 16,
            marginTop: 1,
            flexShrink: 0,
            borderRadius: "50%",
            border: `1.5px solid ${active ? t.accent : t.inputBorder}`,
            display: "flex",
            alignItems: "center",
            justifyContent: "center",
          }}
        >
          {active && (
            <span style={{ width: 8, height: 8, borderRadius: "50%", background: t.accent }} />
          )}
        </span>
        <span style={{ flex: 1, minWidth: 0, display: "flex", flexDirection: "column", gap: 3 }}>
          <span style={{ display: "flex", alignItems: "center", gap: 6, flexWrap: "wrap" }}>
            <span
              style={{
                fontSize: 13,
                fontWeight: 600,
                marginRight: 2,
                color: active ? t.accent : t.text,
                fontFamily: "Inter, sans-serif",
              }}
            >
              {p.label}
            </span>
            {p.value === recommended && (
              <span
                style={{
                  padding: "1px 7px",
                  borderRadius: 999,
                  fontSize: 10,
                  fontWeight: 600,
                  letterSpacing: "0.04em",
                  color: t.accent,
                  border: `1px solid ${t.accent}55`,
                  fontFamily: "Inter, sans-serif",
                }}
              >
                RECOMMENDED
              </span>
            )}
            {!problem && p.pros.map((x) => tag(x, true))}
            {!problem && p.notes.map((x) => tag(x, false))}
          </span>
          <span
            style={{
              fontSize: 12,
              lineHeight: 1.4,
              color: problem ? t.warnColor : t.textMuted,
              fontFamily: "Inter, sans-serif",
            }}
          >
            {problem || p.bestFor}
          </span>
        </span>
      </button>
    );
  };

  return (
    <div style={{ display: "flex", flexDirection: "column", gap: 14 }}>
      {/* ── When to translate ── */}
      <Section title="When to Translate" t={t}>
        <Row
          label="Translate"
          hint={MODES.find((m) => m.value === translationMode)?.desc ?? ""}
          t={t}
          last={translationMode !== "hotkey"}
        >
          <Segmented
            options={MODES}
            value={translationMode}
            onChange={(v) => updateSettings({ translation_mode: v })}
            t={t}
          />
        </Row>
        {translationMode === "hotkey" && (
          <Row
            label="Translate hotkey"
            hint={
              hotkeyConflict
                ? "That's already the launch hotkey — pick a different key"
                : translateHotkey
                  ? `Records and translates to ${targetName}`
                  : `Set a key to record and translate to ${targetName}`
            }
            t={t}
            last
          >
            <HotkeyRecorder
              value={translateHotkey}
              clearable
              onChange={(v) => {
                if (v && v === settings.hotkey) {
                  setHotkeyConflict(true);
                  return;
                }
                setHotkeyConflict(false);
                updateSettings({ translate_hotkey: v });
              }}
              t={t}
            />
          </Row>
        )}
      </Section>

      {translationOff && (
        <div
          style={{
            padding: 16,
            borderRadius: 8,
            background: t.inputBg,
            border: `1px solid ${t.border}`,
            color: t.textMuted,
            fontSize: 12,
            fontFamily: "Inter, sans-serif",
            lineHeight: 1.6,
          }}
        >
          <strong style={{ color: t.text, display: "block", marginBottom: 4 }}>
            Voice-to-text only
          </strong>
          Spoken audio is transcribed locally and pasted in the language you speak. No web requests
          or translation steps. Choose Separate hotkey or Automatic to set up translation.
        </div>
      )}

      {/* ── Translation service ── */}
      {!translationOff && (
        <Section title="Translation Service" t={t}>
          <p
            style={{
              margin: "12px 12px 0",
              fontSize: 11.5,
              color: t.textDim,
              fontFamily: "Inter, sans-serif",
              lineHeight: 1.5,
            }}
          >
            Apple Native, Apple Intelligence and Offline translate on this computer and send nothing
            anywhere. An LLM does too, if its address is on this computer.
          </p>
          <div
            role="radiogroup"
            aria-label="Translation service"
            style={{ padding: 12, display: "flex", flexDirection: "column", gap: 8 }}
          >
            {PROVIDERS.map((p) => {
              // Apple's engines only where Apple Translation runs (macOS 26+). The chosen one
              // always stays visible.
              const native = p.value === "Apple" || p.value === "Apple Intelligence";
              const fits = !native || appleInfo?.available === true;
              if (!fits && activeProvider !== p.value) return null;
              const reason =
                p.value === "Apple"
                  ? appleInfo?.available
                    ? null
                    : (appleInfo?.reason ?? "")
                  : p.value === "Apple Intelligence"
                    ? appleInfo
                      ? appleInfo.intelligence_reason
                      : ""
                    : null;
              // "" while Apple's support is still being checked: not a problem yet.
              return serviceCard(p, activeProvider === p.value, reason || null);
            })}
          </div>
        </Section>
      )}

      {/* ── Provider Configuration Section ── */}
      {!translationOff && (
        <Section
          title={
            activeProvider === "LLM"
              ? "LLM Configuration"
              : activeProvider === "Apple"
                ? "Apple Native Translation"
                : activeProvider === "Apple Intelligence"
                  ? "Apple Intelligence"
                  : activeProvider === "Offline"
                    ? "Offline Translation"
                    : "Custom API Configuration"
          }
          t={t}
        >
          <div style={{ padding: 16 }}>
            {activeProvider === "Apple" &&
              (appleInfo && !appleInfo.available ? (
                <p
                  style={{
                    fontSize: 12,
                    color: t.warnColor,
                    fontFamily: "Inter, sans-serif",
                    lineHeight: 1.5,
                    margin: 0,
                  }}
                >
                  {appleInfo.reason} Recordings that need translation won't start until you choose
                  another service.
                </p>
              ) : (
                <AppleTranslationPanel t={t} />
              ))}

            {activeProvider === "Offline" && <AppleTranslationPanel t={t} offline />}

            {activeProvider === "Apple Intelligence" && (
              <p
                style={{
                  fontSize: 12,
                  color: appleInfo?.intelligence_reason ? t.warnColor : t.textMuted,
                  fontFamily: "Inter, sans-serif",
                  lineHeight: 1.5,
                  margin: 0,
                }}
              >
                {appleInfo?.intelligence_reason
                  ? `${appleInfo.intelligence_reason} Recordings that need translation fail until it's available or you choose another service.`
                  : "Translates on this Mac with Apple's on-device language model: no API key, no cost, nothing leaves your Mac. It follows the prompt template below, so it can also fix grammar or change the tone. Slower than Apple Native (about a second or more per dictation) and weaker than large cloud models."}
              </p>
            )}

            {/* Combined LLM Mode (Both Local & Cloud) */}
            {activeProvider === "LLM" && (
              <div style={{ display: "flex", flexDirection: "column", gap: 14 }}>
                {/* Quick Presets */}
                <div>
                  <FormLabel t={t}>Quick Presets</FormLabel>
                  <div style={{ display: "flex", flexWrap: "wrap", gap: 8, marginTop: 6 }}>
                    {LLM_PRESETS.map((pre) => {
                      const isSelected =
                        currentEndpoint.trim() === pre.endpoint.trim() &&
                        currentModel.trim() === pre.model.trim();
                      return (
                        <button
                          key={pre.name}
                          type="button"
                          onClick={() => applyLlmPreset(pre)}
                          title={pre.hint}
                          style={{
                            padding: "6px 12px",
                            borderRadius: 8,
                            background: isSelected ? `${t.accent}22` : t.inputBg,
                            border: `1px solid ${isSelected ? `${t.accent}66` : t.border}`,
                            color: isSelected ? t.accent : t.textMuted,
                            fontSize: 11,
                            fontFamily: "Inter, sans-serif",
                            fontWeight: isSelected ? 600 : 400,
                            cursor: "pointer",
                            transition: "all 0.15s",
                          }}
                        >
                          {pre.name}
                        </button>
                      );
                    })}
                  </div>
                </div>

                {/* Endpoint URL */}
                <FormGroup>
                  <FormLabel t={t}>Endpoint URL (chat completions)</FormLabel>
                  <FieldInput
                    value={currentEndpoint}
                    onChange={(v) =>
                      updateSettings({
                        llm_endpoint: v,
                        local_llm_url: v,
                      })
                    }
                    placeholder="http://localhost:11434/v1/chat/completions"
                    mono
                    t={t}
                  />
                  <span
                    style={{
                      fontSize: 11,
                      color: t.textDim,
                      fontFamily: "Inter, sans-serif",
                      marginTop: 4,
                      display: "block",
                    }}
                  >
                    Works with local Ollama (http://localhost:11434) and LM Studio
                    (http://localhost:1234), or any server that speaks the chat completions API.
                  </span>
                  {isRemoteEndpoint(currentEndpoint) && <RemoteWarning t={t} />}
                </FormGroup>

                {/* API Key & Model Name in 2 columns */}
                <div style={{ display: "grid", gridTemplateColumns: "1fr 1fr", gap: 12 }}>
                  <FormGroup>
                    <FormLabel t={t}>API Key (Optional for Local)</FormLabel>
                    <ApiKeyInput
                      value={currentApiKey}
                      onChange={(v) =>
                        updateSettings({
                          llm_api_key: v,
                          api_key: v,
                        })
                      }
                      placeholder="Leave empty for local Ollama / LM Studio"
                      t={t}
                    />
                    <span
                      style={{
                        fontSize: 10,
                        color: t.textDim,
                        fontFamily: "Inter, sans-serif",
                        marginTop: 4,
                        display: "block",
                      }}
                    >
                      Needed only for a server you run elsewhere. Leave blank for localhost.
                    </span>
                  </FormGroup>

                  <FormGroup>
                    <FormLabel t={t}>Model Name</FormLabel>
                    <FieldInput
                      value={currentModel}
                      onChange={(v) =>
                        updateSettings({
                          llm_model: v,
                          local_llm_model: v,
                        })
                      }
                      placeholder="llama3.2, qwen2.5, etc."
                      mono
                      t={t}
                    />
                    <div style={{ display: "flex", flexWrap: "wrap", gap: 6, marginTop: 7 }}>
                      {LLM_MODEL_SUGGESTIONS.map((m) => (
                        <button
                          key={m}
                          type="button"
                          onClick={() =>
                            updateSettings({
                              llm_model: m,
                              local_llm_model: m,
                            })
                          }
                          style={{
                            padding: "2px 8px",
                            borderRadius: 12,
                            background: currentModel === m ? `${t.accent}22` : t.inputBg,
                            border: `1px solid ${currentModel === m ? `${t.accent}66` : t.border}`,
                            color: currentModel === m ? t.accent : t.textMuted,
                            fontSize: 10,
                            fontFamily: "JetBrains Mono, monospace",
                            cursor: "pointer",
                          }}
                        >
                          {m}
                        </button>
                      ))}
                    </div>
                  </FormGroup>
                </div>
              </div>
            )}

            {/* HIDDEN: Custom API is not offered in the choices above. Shown only for a saved one, which still works. */}
            {activeProvider === "Custom API" && (
              <div style={{ display: "flex", flexDirection: "column", gap: 14 }}>
                <p
                  style={{
                    fontSize: 12,
                    color: t.textMuted,
                    fontFamily: "Inter, sans-serif",
                    margin: 0,
                  }}
                >
                  Custom API is no longer offered. This saved one keeps working until you pick
                  another service.
                </p>
                <FormGroup>
                  <FormLabel t={t}>Endpoint URL (REST)</FormLabel>
                  <FieldInput
                    value={settings.custom_api_url || ""}
                    onChange={(v) => updateSettings({ custom_api_url: v })}
                    placeholder="https://your-server/v1/chat/completions"
                    mono
                    t={t}
                  />
                  {isRemoteEndpoint(settings.custom_api_url || "") && <RemoteWarning t={t} />}
                </FormGroup>

                <div style={{ display: "grid", gridTemplateColumns: "1fr 1fr", gap: 12 }}>
                  <FormGroup>
                    <FormLabel t={t}>API Key</FormLabel>
                    <ApiKeyInput
                      value={settings.custom_api_key || ""}
                      onChange={(v) => updateSettings({ custom_api_key: v })}
                      placeholder="Bearer sk-…"
                      t={t}
                    />
                  </FormGroup>

                  <FormGroup>
                    <FormLabel t={t}>Model Name</FormLabel>
                    <FieldInput
                      value={settings.custom_api_model || ""}
                      onChange={(v) => updateSettings({ custom_api_model: v })}
                      placeholder="model name"
                      mono
                      t={t}
                    />
                  </FormGroup>
                </div>
              </div>
            )}
          </div>
        </Section>
      )}

      {/* ── Spoken and Target Languages ── */}
      {!translationOff && (
        <Section title="Language Configuration" t={t}>
          <div style={{ padding: 16, display: "grid", gridTemplateColumns: "1fr 1fr", gap: 12 }}>
            <FormGroup>
              <FormLabel t={t}>Spoken Language (Input)</FormLabel>
              <FieldSelect
                value={settings.source_lang || "Auto"}
                onChange={(v) => updateSettings({ source_lang: v })}
                options={SOURCE_LANG_OPTIONS}
                t={t}
              />
              <span
                style={{
                  fontSize: 11,
                  color: t.textDim,
                  fontFamily: "Inter, sans-serif",
                  marginTop: 4,
                  display: "block",
                }}
              >
                With Automatic translation, only this language is translated (e.g. Ukrainian);
                anything else is pasted as spoken. Choose Auto-detect to translate any language. The
                translate hotkey translates whatever you speak.
              </span>
            </FormGroup>

            <FormGroup>
              <FormLabel t={t}>Target Language (Output)</FormLabel>
              <FieldSelect
                value={settings.target_lang || "English"}
                onChange={(v) => updateSettings({ target_lang: v })}
                options={TARGET_LANGUAGES}
                t={t}
              />
              <span
                style={{
                  fontSize: 11,
                  color: t.textDim,
                  fontFamily: "Inter, sans-serif",
                  marginTop: 4,
                  display: "block",
                }}
              >
                The language your text will be translated into. If spoken speech is already in this
                language, translation is automatically skipped.
              </span>
            </FormGroup>
          </div>
        </Section>
      )}

      {/* ── Prompt Template Configuration (Apple's and the offline engines take no prompt) ── */}
      {!translationOff && activeProvider !== "Apple" && activeProvider !== "Offline" && (
        <Section title="Prompt Template (LLM Instructions)" t={t}>
          <div style={{ padding: 16, display: "flex", flexDirection: "column", gap: 14 }}>
            <div>
              <FormLabel t={t}>Prompt Presets</FormLabel>
              <div style={{ display: "flex", flexWrap: "wrap", gap: 8, marginTop: 6 }}>
                {PROMPT_PRESETS.map((p) => {
                  const isCurrent = currentPrompt.trim() === p.template.trim();
                  return (
                    <button
                      key={p.name}
                      type="button"
                      onClick={() => updateSettings({ prompt_template: p.template })}
                      style={{
                        padding: "6px 12px",
                        borderRadius: 8,
                        background: isCurrent ? `${t.accent}22` : t.inputBg,
                        border: `1px solid ${isCurrent ? `${t.accent}66` : t.border}`,
                        color: isCurrent ? t.accent : t.textMuted,
                        fontSize: 11,
                        fontFamily: "Inter, sans-serif",
                        fontWeight: isCurrent ? 600 : 400,
                        cursor: "pointer",
                        transition: "all 0.15s",
                      }}
                    >
                      {p.name}
                    </button>
                  );
                })}
              </div>
            </div>

            <FormGroup>
              <div
                style={{
                  display: "flex",
                  justifyContent: "space-between",
                  alignItems: "center",
                  marginBottom: 6,
                }}
              >
                <FormLabel t={t}>Custom Prompt Template</FormLabel>
                <button
                  type="button"
                  onClick={() => updateSettings({ prompt_template: DEFAULT_PROMPT })}
                  style={{
                    background: "none",
                    border: "none",
                    color: t.accent,
                    fontSize: 11,
                    cursor: "pointer",
                    padding: 0,
                    textDecoration: "underline",
                  }}
                >
                  Reset to default
                </button>
              </div>

              <FieldTextarea
                value={currentPrompt}
                onChange={(v) => updateSettings({ prompt_template: v })}
                placeholder="Enter prompt instructions for translation..."
                rows={5}
                t={t}
              />

              <div
                style={{
                  display: "flex",
                  alignItems: "center",
                  gap: 6,
                  marginTop: 8,
                  flexWrap: "wrap",
                }}
              >
                <span style={{ fontSize: 11, color: t.textDim, fontFamily: "Inter, sans-serif" }}>
                  Insert placeholder:
                </span>
                {["{text}", "{source_lang}", "{target_lang}"].map((tag) => (
                  <button
                    key={tag}
                    type="button"
                    onClick={() => insertVariable(tag)}
                    style={{
                      padding: "2px 8px",
                      borderRadius: 4,
                      background: t.inputBg,
                      border: `1px solid ${t.border}`,
                      color: t.accent,
                      fontFamily: "JetBrains Mono, monospace",
                      fontSize: 10,
                      cursor: "pointer",
                    }}
                  >
                    + {tag}
                  </button>
                ))}
              </div>
            </FormGroup>
          </div>
        </Section>
      )}
    </div>
  );
}
