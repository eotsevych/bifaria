import React, { useEffect, useState } from "react";
import { subscribeToSettingsTab, takeSettingsTab } from "@/lib/tauri";
import { tok, type AccentColor, type Theme } from "@/lib/tokens";
import { useResolvedTheme } from "@/lib/theme";
import { SettingsProvider } from "@/components/settings/SettingsContext";
import { useSettingsContext } from "@/components/settings/useSettingsContext";
import GeneralTab from "@/components/settings/GeneralTab";
import TranscriptionTab from "@/components/settings/TranscriptionTab";
import TextTab from "@/components/settings/TextTab";
import ThemeTab from "@/components/settings/ThemeTab";
import TranslationsTab from "@/components/settings/TranslationsTab";
import StorageTab from "@/components/settings/StorageTab";
import LogsTab from "@/components/settings/LogsTab";
import AudioLabTab, { type LabSourceTexts } from "@/components/settings/AudioLabTab";

type TabId =
  "general" | "transcription" | "text" | "translations" | "theme" | "history" | "audio-labs";

interface TabItem {
  id: TabId;
  label: string;
  icon: React.ReactNode;
}

const TABS: TabItem[] = [
  {
    id: "general",
    label: "General",
    icon: (
      <svg width="15" height="15" viewBox="0 0 16 16" fill="none">
        <path
          d="M6.5 1h3l.5 2 1.5.87 2-.5L15 5.5l-1.25 1.75v1.5L15 10.5l-1.5 2.13-2-.5L10 12.93l-.5 2h-3l-.5-2L4.5 12.07l-2 .5L1 10.43l1.25-1.75v-1.5L1 5.43 2.5 3.3l2 .5L6 2.87z"
          stroke="currentColor"
          strokeWidth="1.2"
          strokeLinejoin="round"
        />
        <circle cx="8" cy="8" r="2" stroke="currentColor" strokeWidth="1.2" />
      </svg>
    ),
  },
  {
    id: "transcription",
    label: "Transcription",
    icon: (
      <svg width="15" height="15" viewBox="0 0 16 16" fill="none">
        <path
          d="M2 7v2M5 4.5v7M8 2v12M11 4.5v7M14 7v2"
          stroke="currentColor"
          strokeWidth="1.2"
          strokeLinecap="round"
        />
      </svg>
    ),
  },
  {
    id: "text",
    label: "Text",
    icon: (
      <svg width="15" height="15" viewBox="0 0 16 16" fill="none">
        <path
          d="M3 3.5h10M8 3.5V13M6 13h4"
          stroke="currentColor"
          strokeWidth="1.2"
          strokeLinecap="round"
        />
      </svg>
    ),
  },
  {
    id: "translations",
    label: "Translations",
    icon: (
      <svg width="15" height="15" viewBox="0 0 16 16" fill="none">
        <path
          d="M1 3h9M5 1v2M3 3c0 3 2 5 5 5M7 3c0 2-1 4-3 5"
          stroke="currentColor"
          strokeWidth="1.2"
          strokeLinecap="round"
        />
        <path
          d="M9 10l2-5 2 5M10 8.5h2"
          stroke="currentColor"
          strokeWidth="1.2"
          strokeLinecap="round"
          strokeLinejoin="round"
        />
      </svg>
    ),
  },
  {
    id: "theme",
    label: "Theme",
    icon: (
      <svg width="15" height="15" viewBox="0 0 16 16" fill="none">
        <circle cx="8" cy="8" r="3" stroke="currentColor" strokeWidth="1.2" />
        <path
          d="M8 1v2M8 13v2M1 8h2M13 8h2M3.05 3.05l1.41 1.41M11.54 11.54l1.41 1.41M3.05 12.95l1.41-1.41M11.54 4.46l1.41-1.41"
          stroke="currentColor"
          strokeWidth="1.2"
          strokeLinecap="round"
        />
      </svg>
    ),
  },
  {
    id: "history",
    label: "History",
    icon: (
      <svg width="15" height="15" viewBox="0 0 16 16" fill="none">
        <circle cx="8" cy="8" r="6" stroke="currentColor" strokeWidth="1.2" />
        <path
          d="M8 4.5V8l2.5 1.5"
          stroke="currentColor"
          strokeWidth="1.2"
          strokeLinecap="round"
          strokeLinejoin="round"
        />
      </svg>
    ),
  },
  {
    id: "audio-labs",
    label: "Audio Labs",
    icon: (
      <svg width="15" height="15" viewBox="0 0 16 16" fill="none">
        <path
          d="M8 1a2 2 0 0 1 2 2v5a2 2 0 0 1-4 0V3a2 2 0 0 1 2-2z"
          stroke="currentColor"
          strokeWidth="1.2"
        />
        <path
          d="M4 8a4 4 0 0 0 8 0M8 12v3"
          stroke="currentColor"
          strokeWidth="1.2"
          strokeLinecap="round"
        />
      </svg>
    ),
  },
];

const TAB_DESCRIPTIONS: Record<TabId, string> = {
  general: "Permissions, updates, hotkeys, and audio devices",
  transcription: "Speech model, memory use, and audio chunking",
  text: "Cleanup of transcribed text and masking of confidential words",
  translations: "Configure your translation provider and credentials",
  theme: "Visual style, accent color, and interface density",
  history: "Recent activity, saved recordings, and retention",
  "audio-labs": "Test transcription accuracy and latency on sample audio",
};

function SettingsInner() {
  const { settings, updateSettings, closeWindow } = useSettingsContext();
  const [activeTab, setActiveTab] = useState<TabId>("general");
  const [labSelectedAudio, setLabSelectedAudio] = useState<string | null>(null);
  const [labTexts, setLabTexts] = useState<LabSourceTexts | null>(null);
  const [savedSuccess, setSavedSuccess] = useState(false);

  // What's New can ask for a tab, both when it opens this window and when it's already open.
  useEffect(() => {
    const show = (tab: string | null) => {
      const found = TABS.find((tb) => tb.id === tab);
      if (found) setActiveTab(found.id);
    };
    takeSettingsTab().then(show);
    const unlisten = subscribeToSettingsTab((tab) => {
      show(tab);
      void takeSettingsTab();
    });
    return () => {
      unlisten.then((fn) => fn?.());
    };
  }, []);

  const handleOpenInAudioLab = (audioFilename: string, texts?: LabSourceTexts) => {
    setLabSelectedAudio(audioFilename);
    setLabTexts(texts ?? null);
    setActiveTab("audio-labs");
  };

  const [accent, setAccent] = useState<AccentColor>(() => {
    return (localStorage.getItem("bifaria_accent") as AccentColor) || "violet";
  });

  const handleAccentChange = (newAccent: AccentColor) => {
    setAccent(newAccent);
    localStorage.setItem("bifaria_accent", newAccent);
    window.dispatchEvent(new Event("storage"));
  };

  const currentTheme = useResolvedTheme(settings.theme);
  const t = tok(currentTheme, accent);

  const handleSave = async () => {
    await updateSettings(settings);
    setSavedSuccess(true);
    setTimeout(() => setSavedSuccess(false), 2000);
  };

  return (
    <div
      style={{
        width: "100vw",
        height: "100vh",
        display: "flex",
        flexDirection: "column",
        background: t.modalBg,
        color: t.text,
        overflow: "hidden",
        userSelect: "none",
      }}
    >
      {/* Native-Integrated Unified Title Bar */}
      <div
        data-tauri-drag-region
        style={
          {
            height: 48,
            flexShrink: 0,
            display: "flex",
            alignItems: "center",
            padding: "0 16px 0 78px",
            background: currentTheme === "dark" ? "rgba(255,255,255,0.03)" : "rgba(0,0,0,0.025)",
            borderBottom: `1px solid ${t.sidebarBorder}`,
            userSelect: "none",
            cursor: "default",
            position: "relative",
            WebkitAppRegion: "drag",
          } as React.CSSProperties
        }
      >
        <span
          style={{
            position: "absolute",
            left: "50%",
            transform: "translateX(-50%)",
            fontSize: 13,
            fontWeight: 600,
            color: t.text,
            fontFamily: "Inter, sans-serif",
            letterSpacing: "-0.01em",
            pointerEvents: "none",
          }}
        >
          Settings
        </span>
      </div>

      {/* Body: Sidebar + Content */}
      <div style={{ display: "flex", flex: 1, overflow: "hidden" }}>
        {/* Sidebar */}
        <aside
          style={{
            width: 185,
            flexShrink: 0,
            background: currentTheme === "dark" ? "rgba(255,255,255,0.02)" : "rgba(0,0,0,0.02)",
            borderRight: `1px solid ${t.sidebarBorder}`,
            padding: "12px 8px",
            display: "flex",
            flexDirection: "column",
            gap: 2,
            overflowY: "auto",
          }}
        >
          {TABS.map((tab) => {
            const active = activeTab === tab.id;
            return (
              <button
                key={tab.id}
                type="button"
                onClick={() => setActiveTab(tab.id)}
                style={{
                  display: "flex",
                  alignItems: "center",
                  gap: 9,
                  padding: "8px 10px",
                  borderRadius: 7,
                  border: "none",
                  background: active ? t.activeSidebar : "transparent",
                  color: active ? t.accent : t.textMuted,
                  fontSize: 14,
                  fontFamily: "Inter, sans-serif",
                  cursor: "pointer",
                  textAlign: "left",
                  width: "100%",
                  transition: "all 0.15s",
                  fontWeight: active ? 500 : 400,
                }}
                onMouseEnter={(e) => {
                  if (!active) (e.currentTarget as HTMLElement).style.background = t.surface;
                }}
                onMouseLeave={(e) => {
                  if (!active) (e.currentTarget as HTMLElement).style.background = "transparent";
                }}
              >
                <span style={{ opacity: active ? 1 : 0.55, flexShrink: 0, lineHeight: 0 }}>
                  {tab.icon}
                </span>
                {tab.label}
              </button>
            );
          })}
        </aside>

        {/* Content Area */}
        <div style={{ flex: 1, display: "flex", flexDirection: "column", overflow: "hidden" }}>
          {/* Header */}
          <div
            style={{
              padding: "16px 22px 14px",
              borderBottom: `1px solid ${t.sidebarBorder}`,
              flexShrink: 0,
            }}
          >
            <h2
              style={{
                fontSize: 16,
                fontWeight: 600,
                color: t.text,
                margin: "0 0 2px",
                fontFamily: "Inter, sans-serif",
              }}
            >
              {TABS.find((tb) => tb.id === activeTab)?.label}
            </h2>
            <p
              style={{
                fontSize: 12,
                color: t.textMuted,
                margin: 0,
                fontFamily: "Inter, sans-serif",
              }}
            >
              {TAB_DESCRIPTIONS[activeTab]}
            </p>
          </div>

          {/* Scrollable Tab Body */}
          <div
            style={{ flex: 1, overflowY: "auto", padding: "18px 22px" }}
            className="fade-in"
            key={activeTab}
          >
            {activeTab === "general" && <GeneralTab t={t} />}
            {activeTab === "transcription" && <TranscriptionTab t={t} />}
            {activeTab === "text" && <TextTab t={t} />}
            {activeTab === "theme" && (
              <ThemeTab t={t} accent={accent} onAccentChange={handleAccentChange} />
            )}
            {activeTab === "translations" && <TranslationsTab t={t} />}
            {activeTab === "history" && (
              <>
                <LogsTab t={t} onOpenInAudioLab={handleOpenInAudioLab} />
                <div style={{ marginTop: 24 }}>
                  <StorageTab t={t} />
                </div>
              </>
            )}
            {activeTab === "audio-labs" && (
              <AudioLabTab t={t} initialAudio={labSelectedAudio} initialTexts={labTexts} />
            )}
          </div>

          {/* Footer */}
          <div
            style={{
              display: "flex",
              alignItems: "center",
              justifyContent: "flex-end",
              gap: 8,
              padding: "10px 22px",
              borderTop: `1px solid ${t.sidebarBorder}`,
              background: currentTheme === "dark" ? "rgba(255,255,255,0.02)" : "rgba(0,0,0,0.02)",
              flexShrink: 0,
            }}
          >
            <button
              type="button"
              onClick={closeWindow}
              style={{
                padding: "6px 16px",
                borderRadius: 7,
                background: "transparent",
                border: `1px solid ${t.border}`,
                color: t.textMuted,
                fontSize: 13,
                fontFamily: "Inter, sans-serif",
                cursor: "pointer",
                transition: "all 0.15s",
              }}
            >
              Cancel
            </button>
            <button
              type="button"
              onClick={handleSave}
              style={{
                padding: "6px 18px",
                borderRadius: 7,
                border: "none",
                background: savedSuccess
                  ? t.successColor
                  : `linear-gradient(135deg, ${t.accent} 0%, ${t.accent}cc 100%)`,
                color: "#fff",
                fontSize: 13,
                fontWeight: 500,
                fontFamily: "Inter, sans-serif",
                cursor: "pointer",
                boxShadow: `0 2px 8px ${t.accent}40`,
                transition: "all 0.2s",
              }}
            >
              {savedSuccess ? "✓ Saved" : "Save Changes"}
            </button>
          </div>
        </div>
      </div>
    </div>
  );
}

export function SettingsScreen() {
  return (
    <SettingsProvider>
      <SettingsInner />
    </SettingsProvider>
  );
}

export default SettingsScreen;
