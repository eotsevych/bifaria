import { useEffect, useState, Component, type ReactNode } from "react";
import { VoicePill } from "./components/VoicePill";
import { AssistantSettings } from "./components/AssistantSettings";
import { Onboarding } from "./components/onboarding/Onboarding";
import { WhatsNewModal } from "./components/WhatsNewModal";
import { fetchSettings, isTauri } from "./lib/tauri";
import { applyTheme, initThemeListener, type ThemeMode } from "./lib/theme";

interface ErrorBoundaryProps {
  children: ReactNode;
}

interface ErrorBoundaryState {
  hasError: boolean;
  error: Error | null;
}

class ErrorBoundary extends Component<ErrorBoundaryProps, ErrorBoundaryState> {
  constructor(props: ErrorBoundaryProps) {
    super(props);
    this.state = { hasError: false, error: null };
  }

  static getDerivedStateFromError(error: Error): ErrorBoundaryState {
    return { hasError: true, error };
  }

  override componentDidCatch(error: Error, info: React.ErrorInfo) {
    console.error("App ErrorBoundary caught:", error, info);
  }

  override render() {
    if (this.state.hasError) {
      return (
        <div
          style={{
            padding: 24,
            fontFamily: "system-ui, sans-serif",
            color: "#e11d48",
            background: "#fff",
            height: "100vh",
            boxSizing: "border-box",
          }}
        >
          <h2 style={{ fontSize: 18, fontWeight: 600 }}>Settings Error</h2>
          <pre
            style={{
              fontSize: 12,
              background: "#f1f5f9",
              padding: 12,
              borderRadius: 8,
              overflow: "auto",
              marginTop: 8,
            }}
          >
            {this.state.error?.message || String(this.state.error)}
          </pre>
          <button
            onClick={() => window.location.reload()}
            style={{
              marginTop: 16,
              padding: "8px 16px",
              borderRadius: 6,
              background: "#0f172a",
              color: "#fff",
              border: "none",
              cursor: "pointer",
              fontWeight: 500,
            }}
          >
            Reload Window
          </button>
        </div>
      );
    }
    return this.props.children;
  }
}

type View = "pill" | "preferences" | "onboarding" | "whats-new";

const WINDOW_VIEWS: View[] = ["preferences", "onboarding", "whats-new"];

/** Which screen this window shows, from the URL hash or the Tauri window label. */
function detectView(): View {
  if (typeof window === "undefined") return "pill";
  const hash = window.location.hash.replace("#", "");
  const fromHash = WINDOW_VIEWS.find((v) => v === hash);
  if (fromHash) return fromHash;
  const tauriWin = window as Window & {
    __TAURI_INTERNALS__?: {
      metadata?: { currentWebview?: { label?: string }; currentWindow?: { label?: string } };
    };
  };
  const label =
    tauriWin.__TAURI_INTERNALS__?.metadata?.currentWebview?.label ||
    tauriWin.__TAURI_INTERNALS__?.metadata?.currentWindow?.label;
  return WINDOW_VIEWS.find((v) => v === label) ?? "pill";
}

export default function App() {
  const [view, setView] = useState<View>(detectView);

  useEffect(() => {
    // 1. Initial theme application
    const savedTheme = (localStorage.getItem("bifaria_theme") as ThemeMode) || "system";
    applyTheme(savedTheme);

    fetchSettings().then((settings) => {
      if (settings?.theme) {
        const t = settings.theme as ThemeMode;
        localStorage.setItem("bifaria_theme", t);
        applyTheme(t);
      }
    });

    // 2. Listen to system preference and Tauri app changes
    const cleanupTheme = initThemeListener();

    // 3. Detect view window
    const detectWindowView = async () => {
      const fromUrl = detectView();
      if (fromUrl !== "pill") {
        setView(fromUrl);
        return;
      }
      if (isTauri()) {
        try {
          const { getCurrentWebviewWindow } = await import("@tauri-apps/api/webviewWindow");
          const label = getCurrentWebviewWindow().label;
          const fromLabel = WINDOW_VIEWS.find((v) => v === label);
          if (fromLabel) setView(fromLabel);
        } catch {
          // ignore
        }
      }
    };
    detectWindowView();

    const onHashChange = () => {
      setView(detectView());
    };
    window.addEventListener("hashchange", onHashChange);

    return () => {
      cleanupTheme();
      window.removeEventListener("hashchange", onHashChange);
    };
  }, []);

  if (view === "preferences") {
    return (
      <ErrorBoundary>
        <AssistantSettings />
      </ErrorBoundary>
    );
  }

  if (view === "onboarding") {
    return (
      <ErrorBoundary>
        <Onboarding />
      </ErrorBoundary>
    );
  }

  if (view === "whats-new") {
    return (
      <ErrorBoundary>
        <WhatsNewModal />
      </ErrorBoundary>
    );
  }

  return (
    <ErrorBoundary>
      <VoicePill />
    </ErrorBoundary>
  );
}
