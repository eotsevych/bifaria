import React, { useCallback, useEffect, useState } from "react";
import type { Tokens } from "@/lib/tokens";
import {
  downloadAppleLanguagePack,
  fetchAppleLanguagePacks,
  openAppleTranslationSettings,
  type AppleLanguagePack,
} from "@/lib/tauri";
import { FormLabel, FormGroup, FieldSelect } from "./SettingsPrimitives";
import { useSettingsContext } from "./useSettingsContext";

// Codes Apple uses for the spoken languages Bifaria offers.
const LANG_CODES: Record<string, string> = {
  English: "en",
  Ukrainian: "uk",
  Spanish: "es",
  French: "fr",
  German: "de",
  Italian: "it",
  Polish: "pl",
  Japanese: "ja",
  Chinese: "zh-Hans",
  Russian: "ru",
};

const MODELS = [
  { value: "fast", label: "Fast", desc: "Tens of milliseconds per sentence" },
  { value: "quality", label: "Best quality", desc: "Smoother wording, about half a second" },
] as const;

const FALLBACKS = [
  { value: "", label: "Paste the spoken text" },
  { value: "LLM", label: "Translate with the LLM provider" },
  { value: "Custom API", label: "Translate with the Custom API" },
  { value: "Apple Intelligence", label: "Translate with Apple Intelligence (on-device)" },
];

const STATUS_LABEL: Record<AppleLanguagePack["status"], string> = {
  installed: "Downloaded",
  supported: "Not downloaded",
  unsupported: "Not supported",
};

const text = (t: Tokens, size = 12): React.CSSProperties => ({
  fontSize: size,
  color: t.textDim,
  fontFamily: "Inter, sans-serif",
  lineHeight: 1.5,
  margin: 0,
});

/** Settings for Apple's on-device translation: which model, which languages are downloaded for the
 * target language (with a Download action), and what happens when it fails. With `offline`, the
 * same for the Offline provider (Firefox Translations models), which has one model and no System
 * Settings. With `packsOnly`, just the language packs into `targetLang` (Audio Lab). */
export default function AppleTranslationPanel({
  t,
  offline = false,
  packsOnly = false,
  targetLang: targetOverride,
}: {
  t: Tokens;
  offline?: boolean;
  packsOnly?: boolean;
  targetLang?: string;
}) {
  const { settings, updateSettings } = useSettingsContext();
  const targetLang = targetOverride || settings.target_lang || "English";
  const sourceCode = LANG_CODES[settings.source_lang || ""] ?? null;

  const [packs, setPacks] = useState<AppleLanguagePack[] | null>(null);
  const [targetCode, setTargetCode] = useState<string | null>(null);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [downloading, setDownloading] = useState<string | null>(null);
  const [notice, setNotice] = useState<string | null>(null);

  const refresh = useCallback(async () => {
    setLoading(true);
    setError(null);
    try {
      const res = await fetchAppleLanguagePacks(targetLang, offline);
      setPacks(res.languages);
      setTargetCode(res.target);
    } catch (e) {
      setError(String(e));
      setPacks(null);
    } finally {
      setLoading(false);
    }
  }, [targetLang, offline]);

  useEffect(() => {
    void refresh();
  }, [refresh]);

  // Languages downloaded in System Settings meanwhile show up when the window is back in front.
  useEffect(() => {
    const onFocus = () => void refresh();
    window.addEventListener("focus", onFocus);
    return () => window.removeEventListener("focus", onFocus);
  }, [refresh]);

  const download = async (pack: AppleLanguagePack) => {
    if (!targetCode) return;
    setDownloading(pack.code);
    setNotice(null);
    setError(null);
    try {
      await downloadAppleLanguagePack(pack.code, targetCode, offline);
      setNotice(`${pack.name} → ${targetLang} is downloaded.`);
    } catch (e) {
      const msg = String(e);
      if (!/cancel|closed/i.test(msg)) setError(msg);
    } finally {
      setDownloading(null);
      await refresh();
    }
  };

  const spoken = sourceCode ? packs?.find((p) => p.code === sourceCode) : undefined;
  const model = settings.apple_translation_model === "quality" ? "quality" : "fast";
  const groups = (["installed", "supported", "unsupported"] as const).map((status) => ({
    status,
    items: (packs ?? []).filter((p) => p.status === status),
  }));

  const button = (
    label: string,
    onClick: () => void,
    opts: { disabled?: boolean; primary?: boolean } = {},
  ) => (
    <button
      type="button"
      onClick={onClick}
      disabled={opts.disabled}
      style={{
        padding: "4px 10px",
        borderRadius: 6,
        background: opts.primary ? `${t.accent}22` : t.inputBg,
        border: `1px solid ${opts.primary ? `${t.accent}66` : t.border}`,
        color: opts.primary ? t.accent : t.textMuted,
        fontSize: 11,
        fontFamily: "Inter, sans-serif",
        cursor: opts.disabled ? "default" : "pointer",
        opacity: opts.disabled ? 0.5 : 1,
        whiteSpace: "nowrap",
      }}
    >
      {label}
    </button>
  );

  const badge = (status: AppleLanguagePack["status"]) => {
    const color =
      status === "installed" ? t.successColor : status === "supported" ? t.warnColor : t.textDim;
    return (
      <span style={{ fontSize: 10, color, fontFamily: "Inter, sans-serif", whiteSpace: "nowrap" }}>
        {STATUS_LABEL[status]}
      </span>
    );
  };

  return (
    <div style={{ display: "flex", flexDirection: "column", gap: 16 }}>
      {!packsOnly && (
        <p style={text(t)}>
          {offline
            ? "Translates with the models behind Firefox's translations. They run on this computer's CPU: no API key, no cost, works offline, and nothing leaves the computer. Each language is a 20-55 MB download, done once. Languages other than English go through English (Ukrainian → English → German), so a pair like that needs both. It translates as-is; for rephrasing or formatting, use an LLM provider."
            : "Translates with the engine behind Apple's Translate app. It runs on your Mac: no API key, no cost, works offline, and nothing leaves the computer. Each language pair is downloaded once. It translates as-is; for rephrasing or formatting, use an LLM provider."}
        </p>
      )}

      {/* ── Model ── */}
      {!offline && !packsOnly && (
        <FormGroup>
          <FormLabel t={t}>Model</FormLabel>
          <div style={{ display: "flex", gap: 8, marginTop: 6 }}>
            {MODELS.map((m) => {
              const active = model === m.value;
              return (
                <button
                  key={m.value}
                  type="button"
                  onClick={() => updateSettings({ apple_translation_model: m.value })}
                  style={{
                    flex: 1,
                    padding: "8px 12px",
                    borderRadius: 8,
                    textAlign: "left",
                    cursor: "pointer",
                    background: active ? `${t.accent}18` : t.inputBg,
                    border: `1px solid ${active ? `${t.accent}66` : t.border}`,
                  }}
                >
                  <span
                    style={{
                      display: "block",
                      fontSize: 12,
                      fontWeight: 600,
                      color: active ? t.accent : t.text,
                      fontFamily: "Inter, sans-serif",
                    }}
                  >
                    {m.label}
                  </span>
                  <span style={text(t, 11)}>{m.desc}</span>
                </button>
              );
            })}
          </div>
          <span style={{ ...text(t, 11), marginTop: 4, display: "block" }}>
            macOS 26.4 and later have both. When only one is downloaded for a language pair, Bifaria
            uses that one.
          </span>
        </FormGroup>
      )}

      {/* ── Language packs ── */}
      <FormGroup>
        <div style={{ display: "flex", alignItems: "center", gap: 8 }}>
          <FormLabel t={t}>Language packs → {targetLang}</FormLabel>
          <div style={{ flex: 1 }} />
          {button(loading ? "Checking…" : "Refresh", () => void refresh(), { disabled: loading })}
          {!offline && button("Open System Settings", () => void openAppleTranslationSettings())}
        </div>

        {spoken && (
          <div
            style={{
              marginTop: 8,
              padding: "8px 12px",
              borderRadius: 8,
              background: t.inputBg,
              border: `1px solid ${spoken.status === "installed" ? t.border : `${t.warnColor}66`}`,
              display: "flex",
              alignItems: "center",
              gap: 10,
            }}
          >
            <span style={{ fontSize: 12, color: t.text, fontFamily: "Inter, sans-serif" }}>
              Your spoken language: {spoken.name} → {targetLang}
            </span>
            {badge(spoken.status)}
            <div style={{ flex: 1 }} />
            {spoken.status === "supported" &&
              button(
                downloading === spoken.code ? "Downloading…" : "Download",
                () => void download(spoken),
                {
                  disabled: downloading !== null,
                  primary: true,
                },
              )}
          </div>
        )}

        {error && <p style={{ ...text(t), color: t.errorColor, marginTop: 8 }}>{error}</p>}
        {notice && <p style={{ ...text(t), color: t.successColor, marginTop: 8 }}>{notice}</p>}

        {packs && (
          <div style={{ display: "flex", flexDirection: "column", gap: 10, marginTop: 10 }}>
            {groups.map(({ status, items }) =>
              items.length === 0 ? null : status === "unsupported" ? (
                <p key={status} style={text(t, 11)}>
                  Not supported into {targetLang}: {items.map((p) => p.name).join(", ")}
                </p>
              ) : (
                <div key={status}>
                  <span style={{ ...text(t, 11), display: "block", marginBottom: 6 }}>
                    {status === "installed"
                      ? `Downloaded (${items.length})`
                      : `Available to download (${items.length})`}
                  </span>
                  <div style={{ display: "grid", gridTemplateColumns: "repeat(3, 1fr)", gap: 6 }}>
                    {items.map((p) => (
                      <div
                        key={p.code}
                        style={{
                          display: "flex",
                          alignItems: "center",
                          gap: 6,
                          padding: "5px 8px",
                          borderRadius: 6,
                          background: t.inputBg,
                          border: `1px solid ${p.code === sourceCode ? `${t.accent}66` : t.border}`,
                          minWidth: 0,
                        }}
                      >
                        <span
                          style={{
                            fontSize: 11,
                            color: t.text,
                            fontFamily: "Inter, sans-serif",
                            overflow: "hidden",
                            textOverflow: "ellipsis",
                            whiteSpace: "nowrap",
                            flex: 1,
                          }}
                          title={p.name}
                        >
                          {p.name}
                        </span>
                        {p.status === "supported"
                          ? button(
                              downloading === p.code ? "…" : "Download",
                              () => void download(p),
                              {
                                disabled: downloading !== null,
                              },
                            )
                          : badge(p.status)}
                      </div>
                    ))}
                  </div>
                </div>
              ),
            )}
          </div>
        )}
        {!offline && (
          <span style={{ ...text(t, 11), marginTop: 8, display: "block" }}>
            Download asks macOS to fetch the languages in a small window. You can also manage them
            in System Settings › General › Language &amp; Region › Translation Languages.
          </span>
        )}
      </FormGroup>

      {/* ── When it fails ── */}
      {!packsOnly && (
        <FormGroup>
          <FormLabel t={t}>
            If {offline ? "offline translation" : "Apple Translation"} fails
          </FormLabel>
          <FieldSelect
            value={settings.apple_fallback_provider || ""}
            onChange={(v) => updateSettings({ apple_fallback_provider: v })}
            options={
              offline ? FALLBACKS.filter((f) => f.value !== "Apple Intelligence") : FALLBACKS
            }
            t={t}
          />
          <span style={{ ...text(t, 11), marginTop: 4, display: "block" }}>
            For example, when a language pair isn't downloaded. The fallback uses the settings saved
            for that service (switch to it above to fill them in) and is used only when they're
            complete.
          </span>
        </FormGroup>
      )}
    </div>
  );
}
