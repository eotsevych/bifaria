import { useState, useEffect } from "react";
import { emit, listen } from "@tauri-apps/api/event";
import { isTauri } from "./tauri";

export type ThemeMode = "system" | "light" | "dark";

export function getSystemTheme(): "dark" | "light" {
  if (typeof window !== "undefined" && window.matchMedia("(prefers-color-scheme: light)").matches) {
    return "light";
  }
  return "dark";
}

export function resolveTheme(mode?: string | null): "dark" | "light" {
  if (mode === "light") return "light";
  if (mode === "dark") return "dark";
  return getSystemTheme();
}

export function applyTheme(mode: ThemeMode) {
  const root = document.documentElement;
  root.classList.remove("dark", "theme-dark", "theme-light");

  const resolved = resolveTheme(mode);
  if (resolved === "dark") {
    root.classList.add("dark", "theme-dark");
  } else {
    root.classList.add("theme-light");
  }
}

export async function broadcastTheme(mode: ThemeMode) {
  if (typeof window !== "undefined") {
    localStorage.setItem("bifaria_theme", mode);
    window.dispatchEvent(new CustomEvent("bifaria-theme-updated", { detail: mode }));
  }
  applyTheme(mode);

  if (isTauri()) {
    try {
      await emit("theme-changed", mode);
    } catch (err) {
      console.error("Failed to emit theme-changed:", err);
    }
  }
}

export function initThemeListener(onThemeChanged?: () => void) {
  if (typeof window === "undefined") return () => {};

  // Listen to OS system theme changes
  const mediaQuery = window.matchMedia("(prefers-color-scheme: dark)");
  const osListener = () => {
    const saved = (localStorage.getItem("bifaria_theme") as ThemeMode) || "system";
    if (saved === "system") {
      applyTheme("system");
      onThemeChanged?.();
      window.dispatchEvent(new CustomEvent("bifaria-theme-updated", { detail: "system" }));
    }
  };
  mediaQuery.addEventListener("change", osListener);

  // Listen to Tauri app-wide theme change events
  let unlistenTauri: (() => void) | null = null;
  if (isTauri()) {
    listen<ThemeMode>("theme-changed", (event) => {
      if (event.payload) {
        localStorage.setItem("bifaria_theme", event.payload);
        applyTheme(event.payload);
        onThemeChanged?.();
        window.dispatchEvent(new CustomEvent("bifaria-theme-updated", { detail: event.payload }));
      }
    }).then((unlisten) => {
      unlistenTauri = unlisten;
    });
  }

  return () => {
    mediaQuery.removeEventListener("change", osListener);
    unlistenTauri?.();
  };
}

export function useResolvedTheme(themeSetting?: string | null): "dark" | "light" {
  const [resolved, setResolved] = useState<"dark" | "light">(() => resolveTheme(themeSetting));

  useEffect(() => {
    setResolved(resolveTheme(themeSetting));

    if (typeof window === "undefined") return;

    const mediaQuery = window.matchMedia("(prefers-color-scheme: dark)");
    const handler = () => {
      setResolved(resolveTheme(themeSetting));
    };

    mediaQuery.addEventListener("change", handler);
    window.addEventListener("bifaria-theme-updated", handler);

    return () => {
      mediaQuery.removeEventListener("change", handler);
      window.removeEventListener("bifaria-theme-updated", handler);
    };
  }, [themeSetting]);

  return resolved;
}
