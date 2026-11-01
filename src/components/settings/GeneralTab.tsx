import { useState, useEffect } from "react";
import type { Tokens } from "@/lib/tokens";
import HotkeyRecorder from "@/components/ui/HotkeyRecorder";
import Toggle from "@/components/ui/Toggle";
import Segmented from "@/components/ui/Segmented";
import { Section, Row, FieldSelect } from "./SettingsPrimitives";
import { useSettingsContext } from "./useSettingsContext";
import UpdatesSection from "./UpdatesSection";

/** Seconds without speech from the start of a recording before the pill warns; "0" = off. */
const NO_SPEECH_ALERT_OPTIONS = [
  { value: "0", label: "Off" },
  { value: "5", label: "5 s" },
  { value: "10", label: "10 s" },
  { value: "15", label: "15 s" },
];
import {
  startMicTest,
  stopMicTest,
  playTestSound,
  subscribeToMicTestLevels,
  checkAccessibility,
  getLaunchAtLogin,
  setLaunchAtLogin,
  requestAccessibility,
  openAccessibilitySettings,
  type TranslationMode,
  NOISE_REDUCTION_OPTIONS,
} from "@/lib/tauri";

/** Hotkeys and pasting need the Accessibility permission only on macOS. */
const IS_MAC = typeof navigator !== "undefined" && /Mac/i.test(navigator.userAgent);

export default function GeneralTab({ t }: { t: Tokens }) {
  const { settings, updateSettings, inputDevices, outputDevices, refreshAudioDevices } =
    useSettingsContext();

  const [isTestingMic, setIsTestingMic] = useState(false);
  // Applied straight away: it's a system login item, not part of the saved settings.
  const [launchAtLogin, setLaunchAtLoginState] = useState(false);
  const [launchAtLoginError, setLaunchAtLoginError] = useState<string | null>(null);

  useEffect(() => {
    getLaunchAtLogin().then(setLaunchAtLoginState);
  }, []);

  const toggleLaunchAtLogin = async (enabled: boolean) => {
    setLaunchAtLoginState(enabled);
    setLaunchAtLoginError(null);
    try {
      await setLaunchAtLogin(enabled);
    } catch (e) {
      setLaunchAtLoginState(!enabled);
      setLaunchAtLoginError(String(e));
    }
  };
  const translateHotkey = settings.translate_hotkey ?? "";
  const translationMode: TranslationMode = settings.translation_mode ?? "hotkey";
  const targetName = settings.target_lang || "the target language";
  const spokenOnly =
    settings.source_lang && settings.source_lang !== "Auto" ? settings.source_lang : null;
  const [micLevel, setMicLevel] = useState(0);
  const [micPeak, setMicPeak] = useState(0);
  const [noiseFloor, setNoiseFloor] = useState(0);
  const [testDuration, setTestDuration] = useState(0);
  const [isPlayingTestSound, setIsPlayingTestSound] = useState(false);
  const [isRefreshingAudio, setIsRefreshingAudio] = useState(false);
  const [isAccessibilityGranted, setIsAccessibilityGranted] = useState<boolean | null>(null);
  const [isCheckingAccessibility, setIsCheckingAccessibility] = useState(false);

  // Sync accessibility permission
  const syncAccessibility = async () => {
    setIsCheckingAccessibility(true);
    try {
      const granted = await checkAccessibility();
      setIsAccessibilityGranted(granted);
    } catch {
      setIsAccessibilityGranted(false);
    } finally {
      setTimeout(() => setIsCheckingAccessibility(false), 300);
    }
  };

  useEffect(() => {
    syncAccessibility();
    const handleFocus = () => syncAccessibility();
    window.addEventListener("focus", handleFocus);
    return () => window.removeEventListener("focus", handleFocus);
  }, []);

  // Live microphone VU level listener & quality analysis
  useEffect(() => {
    let unlisten: (() => void) | null = null;
    let isMounted = true;
    let timer: ReturnType<typeof setInterval> | null = null;

    if (isTestingMic) {
      setMicLevel(0);
      setMicPeak(0);
      setNoiseFloor(0);
      setTestDuration(0);
      const startTime = Date.now();

      timer = setInterval(() => {
        if (!isMounted) return;
        setTestDuration(Math.floor((Date.now() - startTime) / 1000));
      }, 500);

      startMicTest(settings.input_device ?? null);
      const samples: number[] = [];

      subscribeToMicTestLevels((data) => {
        if (!isMounted) return;
        const rawLvl = typeof data === "number" ? data : (data?.level ?? 0);
        const rawPeak = typeof data === "number" ? data : (data?.peak ?? rawLvl);
        const lvlPct = Math.min(100, Math.max(0, Math.round(rawLvl * 100)));
        const peakPct = Math.min(100, Math.max(0, Math.round(rawPeak * 100)));

        setMicLevel(lvlPct);
        setMicPeak((prev) => Math.max(prev, peakPct));

        if (samples.length < 35) {
          samples.push(lvlPct);
          if (samples.length >= 8) {
            const sorted = [...samples].sort((a, b) => a - b);
            const baseline = sorted[Math.floor(sorted.length * 0.25)] ?? 0;
            setNoiseFloor(baseline);
          }
        }
      }).then((un) => {
        unlisten = un;
      });
    } else {
      stopMicTest();
      setMicLevel(0);
      setMicPeak(0);
      setNoiseFloor(0);
      setTestDuration(0);
    }

    return () => {
      isMounted = false;
      if (timer) clearInterval(timer);
      if (unlisten) unlisten();
      stopMicTest();
    };
  }, [isTestingMic, settings.input_device]);

  const handleTestSound = async () => {
    try {
      setIsPlayingTestSound(true);
      await playTestSound();
      setTimeout(() => setIsPlayingTestSound(false), 800);
    } catch {
      setIsPlayingTestSound(false);
    }
  };

  const handleRefreshAudio = async () => {
    setIsRefreshingAudio(true);
    await refreshAudioDevices();
    setTimeout(() => setIsRefreshingAudio(false), 500);
  };

  const getQualityAssessment = () => {
    if (testDuration < 1) {
      return {
        text: "Listening to microphone input… Speak normally.",
        color: t.accent,
        icon: "🎤",
      };
    }
    if (micPeak > 92) {
      return {
        text: "Too loud / Clipping! Move away from the mic or reduce input volume.",
        color: t.errorColor,
        icon: "⚠️",
      };
    }
    if (noiseFloor > 18) {
      return {
        text: "High background noise detected! May degrade speech recognition.",
        color: t.warnColor,
        icon: "⚠️",
      };
    }
    if (micPeak < 10 && testDuration >= 3) {
      return {
        text: "Too quiet! Cannot hear clearly. Speak closer or increase mic volume.",
        color: t.warnColor,
        icon: "⚠️",
      };
    }
    if (micPeak >= 20 && noiseFloor <= 15) {
      return {
        text: "Optimal audio clarity! Clean signal for speech recognition.",
        color: t.successColor,
        icon: "✓",
      };
    }
    return {
      text: "Signal detected. Speak normally to measure audio clarity.",
      color: t.accent,
      icon: "🎤",
    };
  };

  const micOptions = (() => {
    const base = inputDevices.map((d) => ({ value: d, label: d }));
    const sel = (settings.input_device || "").trim();
    if (sel && sel !== "Default" && !inputDevices.includes(sel)) {
      base.unshift({
        value: sel,
        label: sel + " (not connected — the pill asks which mic to use)",
      });
    }
    return [{ value: "", label: "System Default" }, ...base];
  })();

  const outputOptions = (() => {
    const base = outputDevices.map((d) => ({ value: d, label: d }));
    const sel = (settings.output_device || "").trim();
    if (sel && sel !== "Default" && !outputDevices.includes(sel)) {
      base.unshift({ value: sel, label: sel + " (unavailable — will fallback to Default)" });
    }
    return [{ value: "", label: "System Default" }, ...base];
  })();

  const anySound = settings.sound_start || settings.sound_stop || settings.sound_error;

  const permissions = (
    <Section title="System Permissions" t={t}>
      <Row
        label="Accessibility sync"
        hint="Check or grant macOS accessibility permission for hotkeys and auto-paste"
        t={t}
        last
      >
        <div style={{ display: "flex", alignItems: "center", gap: 8 }}>
          <span
            style={{
              fontSize: 11,
              fontFamily: "JetBrains Mono, monospace",
              padding: "4px 8px",
              borderRadius: 6,
              fontWeight: 600,
              background:
                isAccessibilityGranted === true
                  ? `${t.successColor}18`
                  : isAccessibilityGranted === false
                    ? `${t.warnColor}18`
                    : t.surface,
              color:
                isAccessibilityGranted === true
                  ? t.successColor
                  : isAccessibilityGranted === false
                    ? t.warnColor
                    : t.textDim,
              border: `1px solid ${
                isAccessibilityGranted === true
                  ? `${t.successColor}33`
                  : isAccessibilityGranted === false
                    ? `${t.warnColor}33`
                    : t.border
              }`,
            }}
          >
            {isAccessibilityGranted === null
              ? "Checking…"
              : isAccessibilityGranted
                ? "✓ Granted"
                : "⚠ Not Granted"}
          </span>

          {isAccessibilityGranted === false && (
            <button
              type="button"
              onClick={() => requestAccessibility()}
              style={{
                padding: "5px 10px",
                borderRadius: 6,
                border: `1px solid ${t.warnColor}`,
                background: `${t.warnColor}20`,
                color: t.warnColor,
                fontSize: 11,
                cursor: "pointer",
                fontWeight: 500,
                whiteSpace: "nowrap",
              }}
            >
              Grant Access
            </button>
          )}

          <button
            type="button"
            onClick={syncAccessibility}
            title="Sync accessibility status"
            disabled={isCheckingAccessibility}
            style={{
              padding: "5px 10px",
              borderRadius: 6,
              border: `1px solid ${t.border}`,
              background: t.surface,
              color: t.textMuted,
              fontSize: 11,
              cursor: "pointer",
              display: "flex",
              alignItems: "center",
              gap: 4,
            }}
          >
            <span
              style={{
                display: "inline-block",
                transform: isCheckingAccessibility ? "rotate(180deg)" : "none",
                transition: "transform 0.4s",
              }}
            >
              ↻
            </span>
            <span>Sync</span>
          </button>
        </div>
      </Row>
    </Section>
  );

  return (
    <div>
      {/* ── System Permissions (macOS only: Accessibility permission) ── */}
      {IS_MAC && permissions}
      <UpdatesSection t={t} />

      {/* ── Input Section ── */}
      <Section title="Input" t={t}>
        <Row
          label="Launch hotkey"
          hint={
            translationMode === "auto"
              ? `Records and transcribes; translates to ${targetName} when you speak ${spokenOnly ?? "another language"}`
              : "Records and transcribes in the language you speak"
          }
          t={t}
        >
          <HotkeyRecorder
            value={settings.hotkey}
            onChange={(v) => {
              // A translate hotkey on the same key would never fire; turn it off.
              updateSettings(
                v && v === translateHotkey ? { hotkey: v, translate_hotkey: "" } : { hotkey: v },
              );
            }}
            t={t}
          />
        </Row>

        <Row label="Microphone input" hint="Audio capture device" t={t}>
          <div style={{ display: "flex", alignItems: "center", gap: 8 }}>
            <FieldSelect
              value={settings.input_device || ""}
              onChange={(v) => updateSettings({ input_device: v || null })}
              options={micOptions}
              t={t}
            />
            <button
              type="button"
              onClick={() => setIsTestingMic((prev) => !prev)}
              style={{
                padding: "6px 12px",
                borderRadius: 7,
                border: `1px solid ${isTestingMic ? t.accent : t.border}`,
                background: isTestingMic ? `${t.accent}20` : t.surface,
                color: isTestingMic ? t.accent : t.textMuted,
                fontSize: 11,
                fontFamily: "Inter, sans-serif",
                cursor: "pointer",
                whiteSpace: "nowrap",
                transition: "all 0.15s",
              }}
            >
              {isTestingMic ? "Stop Test" : "Test Mic"}
            </button>
            <button
              type="button"
              onClick={handleRefreshAudio}
              title="Refresh audio devices"
              style={{
                padding: "6px 8px",
                borderRadius: 7,
                border: `1px solid ${t.border}`,
                background: t.surface,
                color: t.textMuted,
                fontSize: 12,
                cursor: "pointer",
              }}
            >
              <span
                style={{
                  display: "inline-block",
                  transform: isRefreshingAudio ? "rotate(180deg)" : "none",
                  transition: "transform 0.4s",
                }}
              >
                ↻
              </span>
            </button>
          </div>
        </Row>

        {isTestingMic && (
          <div
            style={{
              padding: "12px 16px",
              background: t.inputBg,
              borderBottom: `1px solid ${t.border}`,
            }}
          >
            <div
              style={{
                display: "flex",
                justifyContent: "space-between",
                marginBottom: 6,
                fontSize: 10,
                fontFamily: "JetBrains Mono, monospace",
                color: t.textDim,
              }}
            >
              <span>MIC INPUT LEVEL</span>
              <div style={{ display: "flex", gap: 10 }}>
                <span>
                  PEAK:{" "}
                  <strong style={{ color: micPeak > 90 ? t.errorColor : t.text }}>
                    {micPeak}%
                  </strong>
                </span>
                <span>
                  NOISE:{" "}
                  <strong style={{ color: noiseFloor > 18 ? t.warnColor : t.textDim }}>
                    {noiseFloor}%
                  </strong>
                </span>
                <span style={{ color: t.accent }}>LIVE: {micLevel}%</span>
              </div>
            </div>
            <div style={{ display: "flex", gap: 3, height: 10 }}>
              {Array.from({ length: 24 }).map((_, i) => {
                const stepVal = (i + 1) * (100 / 24);
                const isActive = micLevel >= stepVal;
                const isWarn = i >= 18;
                const isCrit = i >= 21;
                const color = isCrit ? t.errorColor : isWarn ? t.warnColor : t.accent;
                return (
                  <div
                    key={i}
                    style={{
                      flex: 1,
                      borderRadius: 2,
                      background: isActive ? color : t.stepTrack,
                      transition: "background 0.08s ease",
                    }}
                  />
                );
              })}
            </div>

            {/* Live Audio Quality Guidance */}
            {(() => {
              const quality = getQualityAssessment();
              return (
                <div
                  style={{
                    marginTop: 8,
                    padding: "6px 10px",
                    borderRadius: 6,
                    fontSize: 11,
                    fontFamily: "Inter, sans-serif",
                    background: `${quality.color}14`,
                    border: `1px solid ${quality.color}28`,
                    color: quality.color,
                    display: "flex",
                    alignItems: "center",
                    gap: 6,
                  }}
                >
                  <span>{quality.icon}</span>
                  <span>{quality.text}</span>
                </div>
              );
            })()}
          </div>
        )}

        <Row
          label="Noise reduction"
          hint="Removes fans, hum and room noise from each recording (RNNoise)"
          t={t}
        >
          <Segmented
            options={NOISE_REDUCTION_OPTIONS}
            value={settings.noise_reduction ?? "off"}
            onChange={(v) => updateSettings({ noise_reduction: v })}
            t={t}
          />
        </Row>
        <Row
          label="No-speech alert"
          hint="Warn, with a sound, when a recording hears no speech this long after it starts (often a muted mic)"
          t={t}
          last
        >
          <Segmented
            options={NO_SPEECH_ALERT_OPTIONS}
            value={String(settings.no_speech_alert_sec ?? 5)}
            onChange={(v) => updateSettings({ no_speech_alert_sec: Number(v) })}
            t={t}
          />
        </Row>
      </Section>

      {/* ── Sounds ── */}
      <Section title="Sounds" t={t}>
        <Row
          label="Sounds"
          hint="Chimes when recording starts and stops, and an alert on errors"
          t={t}
          last={!anySound}
        >
          <Toggle
            value={anySound}
            onChange={(v) => updateSettings({ sound_start: v, sound_stop: v, sound_error: v })}
            accent={t.accent}
          />
        </Row>
        {anySound && (
          <>
            <Row label="Start sound" hint="When recording starts — you can talk" t={t}>
              <Toggle
                value={settings.sound_start}
                onChange={(v) => updateSettings({ sound_start: v })}
                accent={t.accent}
              />
            </Row>
            <Row label="Stop sound" hint="When recording stops" t={t}>
              <Toggle
                value={settings.sound_stop}
                onChange={(v) => updateSettings({ sound_stop: v })}
                accent={t.accent}
              />
            </Row>
            <Row
              label="Error sound"
              hint="On errors and mic warnings, so you hear them without looking"
              t={t}
            >
              <Toggle
                value={settings.sound_error}
                onChange={(v) => updateSettings({ sound_error: v })}
                accent={t.accent}
              />
            </Row>
            <Row label="Audio output" hint="Speaker for the sounds" t={t} last>
              <div style={{ display: "flex", alignItems: "center", gap: 8 }}>
                <FieldSelect
                  value={settings.output_device || ""}
                  onChange={(v) => updateSettings({ output_device: v || null })}
                  options={outputOptions}
                  t={t}
                />
                <button
                  type="button"
                  onClick={handleTestSound}
                  disabled={isPlayingTestSound}
                  style={{
                    padding: "6px 12px",
                    borderRadius: 7,
                    border: `1px solid ${t.border}`,
                    background: isPlayingTestSound ? `${t.accent}20` : t.surface,
                    color: isPlayingTestSound ? t.accent : t.textMuted,
                    fontSize: 11,
                    fontFamily: "Inter, sans-serif",
                    cursor: isPlayingTestSound ? "default" : "pointer",
                    whiteSpace: "nowrap",
                    transition: "all 0.15s",
                  }}
                >
                  {isPlayingTestSound ? "Playing…" : "Test Sound"}
                </button>
              </div>
            </Row>
          </>
        )}
      </Section>

      {/* ── Behavior Section ── */}
      <Section title="Behavior" t={t}>
        <Row
          label="Auto-paste"
          hint="Paste the text into the app you're in. Off: it's only copied"
          t={t}
        >
          <Toggle
            value={settings.auto_paste}
            onChange={(v) => updateSettings({ auto_paste: v })}
            accent={t.accent}
          />
        </Row>
        {settings.auto_paste && (
          <Row
            label="Keep my clipboard"
            hint="After pasting, put back what you had copied: text, images or files"
            t={t}
          >
            <Toggle
              value={settings.keep_clipboard ?? false}
              onChange={(v) => updateSettings({ keep_clipboard: v })}
              accent={t.accent}
            />
          </Row>
        )}
        <Row
          label="Open at login"
          hint={launchAtLoginError ?? "Start Bifaria automatically when you log in"}
          t={t}
        >
          <Toggle value={launchAtLogin} onChange={toggleLaunchAtLogin} accent={t.accent} />
        </Row>
        <Row
          label="Show hints"
          hint="Show how to finish a recording on the pill for your first few recordings, and again after you change a hotkey"
          t={t}
          last
        >
          <Toggle
            value={settings.show_hints ?? true}
            onChange={(v) => updateSettings({ show_hints: v })}
            accent={t.accent}
          />
        </Row>
      </Section>
    </div>
  );
}
