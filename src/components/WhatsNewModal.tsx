import { useState, useEffect, type ReactNode } from "react";
import { useResolvedTheme } from "@/lib/theme";
import { dismissWhatsNew, openSettingsFromWhatsNew, fetchUpdateStatus } from "@/lib/tauri";

const MONO = "'JetBrains Mono', ui-monospace, monospace";
const SANS = "Inter, system-ui, sans-serif";

const DARK = {
  card: "#14141f",
  cardBorder: "rgba(255,255,255,.08)",
  row: "#181828",
  rowBorder: "rgba(255,255,255,.07)",
  text: "#e8e8f0",
  body: "#9a9ab0",
  dim: "#8a8aa2",
  accent: "#a594ff",
  accentSoft: "rgba(165,148,255,.14)",
  accentSoftBorder: "rgba(165,148,255,.3)",
  accentText: "#c4b8ff",
  onAccent: "#0b0b12",
  green: "#34d399",
  greenSoft: "rgba(52,211,153,.12)",
  greenBorder: "rgba(52,211,153,.35)",
  cyan: "#22d3ee",
  cyanSoft: "rgba(34,211,238,.12)",
  cyanBorder: "rgba(34,211,238,.35)",
  closeBtnHover: "rgba(255,255,255,.08)",
};

const LIGHT: typeof DARK = {
  card: "#ffffff",
  cardBorder: "rgba(0,0,0,.08)",
  row: "#f8f8fc",
  rowBorder: "rgba(0,0,0,.06)",
  text: "#1a1a24",
  body: "#5c5c70",
  dim: "#6e6e84",
  accent: "#7c6af0",
  accentSoft: "rgba(124,106,240,.12)",
  accentSoftBorder: "rgba(124,106,240,.3)",
  accentText: "#5b4bd6",
  onAccent: "#ffffff",
  green: "#059669",
  greenSoft: "rgba(5,150,105,.1)",
  greenBorder: "rgba(5,150,105,.3)",
  cyan: "#0891b2",
  cyanSoft: "rgba(8,145,178,.1)",
  cyanBorder: "rgba(8,145,178,.3)",
  closeBtnHover: "rgba(0,0,0,.05)",
};

type Palette = typeof DARK;

const AppMark = () => (
  <div
    style={{
      width: 22,
      height: 22,
      borderRadius: 7,
      background: "linear-gradient(135deg,#6a5cff,#ff7aa8,#ff8a3d)",
      flexShrink: 0,
    }}
  />
);

function Icon({ children }: { children: ReactNode }) {
  return (
    <svg
      width="16"
      height="16"
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      strokeWidth="2"
      strokeLinecap="round"
      strokeLinejoin="round"
    >
      {children}
    </svg>
  );
}

/** One area of the update (translation, transcription…) and its changes. */
function Group({
  p,
  color,
  soft,
  border,
  icon,
  title,
  children,
}: {
  p: Palette;
  color: string;
  soft: string;
  border: string;
  icon: ReactNode;
  title: string;
  children: ReactNode;
}) {
  return (
    <div
      style={{
        padding: "10px 14px 12px",
        background: p.row,
        border: `1px solid ${p.rowBorder}`,
        borderRadius: 12,
      }}
    >
      <div style={{ display: "flex", alignItems: "center", gap: 8, marginBottom: 8 }}>
        <div
          style={{
            width: 26,
            height: 26,
            borderRadius: 7,
            background: soft,
            border: `1px solid ${border}`,
            display: "flex",
            alignItems: "center",
            justifyContent: "center",
            flexShrink: 0,
            color,
          }}
        >
          {icon}
        </div>
        <div
          style={{
            font: `700 11px ${MONO}`,
            letterSpacing: 0.8,
            textTransform: "uppercase",
            color,
          }}
        >
          {title}
        </div>
      </div>
      <div style={{ display: "flex", flexDirection: "column", gap: 6 }}>{children}</div>
    </div>
  );
}

function Item({ p, title, children }: { p: Palette; title: string; children: ReactNode }) {
  return (
    <div style={{ fontSize: 12, lineHeight: 1.45, color: p.body }}>
      <span style={{ fontWeight: 600, color: p.text }}>{title}.</span> {children}
    </div>
  );
}

/** Shown once after each update, and from Settings › Updates. Update the features for each release
 * that changes what users see. */
export function WhatsNewModal() {
  const theme = useResolvedTheme();
  const p = theme === "dark" ? DARK : LIGHT;
  const [loading, setLoading] = useState<"settings" | "dismiss" | null>(null);
  const [version, setVersion] = useState<string | null>(null);

  useEffect(() => {
    fetchUpdateStatus().then((s) => setVersion(s?.current_version ?? null));
  }, []);

  const run = (which: "settings" | "dismiss", action: () => Promise<void>) => async () => {
    if (loading) return;
    setLoading(which);
    try {
      await action();
    } catch (e) {
      console.error("What's New action failed:", e);
      setLoading(null);
    }
  };

  const handleSettings = run("settings", openSettingsFromWhatsNew);
  const handleDismiss = run("dismiss", dismissWhatsNew);

  useEffect(() => {
    const onKeyDown = (e: KeyboardEvent) => {
      if (e.key === "Escape") handleDismiss();
    };
    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, [handleDismiss]);

  return (
    <div
      style={{
        width: "100vw",
        height: "100vh",
        boxSizing: "border-box",
        background: p.card,
        border: `1px solid ${p.cardBorder}`,
        borderRadius: 20,
        color: p.text,
        fontFamily: SANS,
        display: "flex",
        flexDirection: "column",
        overflow: "hidden",
        userSelect: "none",
        padding: "20px 24px 22px",
      }}
    >
      {/* Header */}
      <div
        data-tauri-drag-region
        style={{
          display: "flex",
          alignItems: "center",
          justifyContent: "space-between",
          marginBottom: 16,
        }}
      >
        <div data-tauri-drag-region style={{ display: "flex", alignItems: "center", gap: 8 }}>
          <AppMark />
          <span data-tauri-drag-region style={{ fontSize: 13, fontWeight: 600 }}>
            Bifaria
          </span>
          {version && (
            <span
              style={{
                padding: "2px 7px",
                borderRadius: 5,
                border: `1px solid ${p.accentSoftBorder}`,
                background: p.accentSoft,
                color: p.accentText,
                font: `600 10px ${MONO}`,
                letterSpacing: 0.5,
                textTransform: "uppercase",
              }}
            >
              v{version}
            </span>
          )}
        </div>

        <button
          type="button"
          onClick={handleDismiss}
          disabled={loading !== null}
          title="Close"
          aria-label="Close"
          style={{
            width: 26,
            height: 26,
            display: "flex",
            alignItems: "center",
            justifyContent: "center",
            background: "none",
            border: "none",
            borderRadius: 6,
            color: p.dim,
            cursor: "pointer",
            padding: 0,
            transition: "background 0.15s",
          }}
          onMouseEnter={(e) => {
            e.currentTarget.style.background = p.closeBtnHover;
          }}
          onMouseLeave={(e) => {
            e.currentTarget.style.background = "none";
          }}
        >
          <svg width="14" height="14" viewBox="0 0 14 14" fill="none">
            <path
              d="M3 3L11 11M11 3L3 11"
              stroke="currentColor"
              strokeWidth="1.5"
              strokeLinecap="round"
            />
          </svg>
        </button>
      </div>

      {/* Title & lead */}
      <div style={{ marginBottom: 14 }}>
        <h1
          style={{
            margin: "0 0 6px",
            fontSize: 19,
            fontWeight: 700,
            letterSpacing: -0.3,
            color: p.text,
          }}
        >
          RevFly is now Bifaria
        </h1>
        <p style={{ margin: 0, fontSize: 13, lineHeight: 1.45, color: p.body }}>
          A new name and icon, the same app and your same settings. Dictation is violet and
          translation is orange, like Bifaria on iPhone. And it&apos;s the biggest update yet:
        </p>
      </div>

      {/* Feature highlights; they scroll if the window is smaller than they need. */}
      <div
        style={{
          display: "flex",
          flexDirection: "column",
          gap: 8,
          flex: 1,
          minHeight: 0,
          overflowY: "auto",
        }}
      >
        <Group
          p={p}
          color={p.green}
          soft={p.greenSoft}
          border={p.greenBorder}
          title="Translation"
          icon={
            <Icon>
              <circle cx="12" cy="12" r="10" />
              <path d="M2 12h20M12 2a15.3 15.3 0 0 1 4 10 15.3 15.3 0 0 1-4 10 15.3 15.3 0 0 1-4-10 15.3 15.3 0 0 1 4-10z" />
            </Icon>
          }
        >
          <Item p={p} title="Offline on every computer">
            Free and private on Windows, Linux and Mac.
          </Item>
          <Item p={p} title="Apple Intelligence">
            On macOS 26, translates on your Mac with your own prompt.
          </Item>
          <Item p={p} title="Easier to choose">
            Each service shows what it&apos;s best for.
          </Item>
        </Group>
        <Group
          p={p}
          color={p.accent}
          soft={p.accentSoft}
          border={p.accentSoftBorder}
          title="Transcription"
          icon={
            <Icon>
              <polygon points="13 2 3 14 12 14 11 22 21 10 12 10 13 2" />
            </Icon>
          }
        >
          <Item p={p} title="Faster on your hardware">
            Neural Engine on Mac, GPU on Windows and Linux.
          </Item>
          <Item p={p} title="Transcribe while you talk">
            Now with every model, for faster long dictations.
          </Item>
          <Item p={p} title="Better numbers">
            &ldquo;Two hundred and fifty&rdquo; → 250, &ldquo;4 30 PM&rdquo; → 4:30 PM.
          </Item>
        </Group>
        <Group
          p={p}
          color={p.cyan}
          soft={p.cyanSoft}
          border={p.cyanBorder}
          title="Privacy & text"
          icon={
            <Icon>
              <rect x="3" y="11" width="18" height="11" rx="2" />
              <path d="M7 11V7a5 5 0 0 1 10 0v4" />
            </Icon>
          }
        >
          <Item p={p} title="Don't save keeps nothing on disk">
            Saved ones are in Settings › History.
          </Item>
          <Item p={p} title="Text styles">
            Natural speech, For AI prompts or Verbatim.
          </Item>
          <Item p={p} title="Keep my clipboard">
            After pasting, Bifaria puts back what you had copied.
          </Item>
        </Group>
      </div>

      {/* Actions */}
      <div style={{ marginTop: 16, display: "flex", gap: 10 }}>
        <button
          type="button"
          onClick={handleSettings}
          disabled={loading !== null}
          style={{
            flex: 1.4,
            padding: "10px 16px",
            borderRadius: 10,
            background: p.accent,
            color: p.onAccent,
            border: "none",
            fontSize: 13,
            fontWeight: 600,
            cursor: loading ? "default" : "pointer",
            opacity: loading && loading !== "settings" ? 0.6 : 1,
          }}
        >
          {loading === "settings" ? "Opening…" : "Choose a Text Style"}
        </button>
        <button
          type="button"
          onClick={handleDismiss}
          disabled={loading !== null}
          style={{
            flex: 1,
            padding: "10px 16px",
            borderRadius: 10,
            background: "transparent",
            color: p.text,
            border: `1px solid ${p.cardBorder}`,
            fontSize: 13,
            fontWeight: 500,
            cursor: loading ? "default" : "pointer",
            opacity: loading && loading !== "dismiss" ? 0.6 : 1,
          }}
        >
          Close
        </button>
      </div>
    </div>
  );
}
