import { useCallback, useEffect, useState } from "react";
import { emit, listen } from "@tauri-apps/api/event";
import type { LayoutMode, TimeMode } from "../types";

// Tauri event used for cross-window UI-pref sync. localStorage is shared
// across Tauri webviews, but state held in `useState` is not — so the
// settings window emits this event after writing localStorage and other
// windows re-read on receipt.
export const UI_PREFS_EVENT = "ui-prefs-updated";

export interface UiPrefs {
  layoutMode: LayoutMode;
  timeMode: TimeMode;
  showLive: boolean;
  showAnalytics: boolean;
}

const STORAGE_KEYS = {
  layoutMode: "quill-layout-mode",
  timeMode: "quill-time-mode",
  showLive: "quill-show-live",
  showAnalytics: "quill-show-analytics",
} as const;

const DEFAULTS: UiPrefs = {
  layoutMode: "stacked",
  timeMode: "marker",
  showLive: true,
  showAnalytics: false,
};

function readBool(key: string, fallback: boolean): boolean {
  try {
    const stored = localStorage.getItem(key);
    if (stored === "true") return true;
    if (stored === "false") return false;
  } catch {
    /* ignore */
  }
  return fallback;
}

function readLayoutMode(): LayoutMode {
  try {
    const stored = localStorage.getItem(STORAGE_KEYS.layoutMode);
    if (stored === "stacked" || stored === "side-by-side") return stored;
  } catch {
    /* ignore */
  }
  return DEFAULTS.layoutMode;
}

function readTimeMode(): TimeMode {
  try {
    const stored = localStorage.getItem(STORAGE_KEYS.timeMode);
    if (stored === "marker" || stored === "dual" || stored === "background") return stored;
  } catch {
    /* ignore */
  }
  return DEFAULTS.timeMode;
}

export function readUiPrefs(): UiPrefs {
  return {
    layoutMode: readLayoutMode(),
    timeMode: readTimeMode(),
    showLive: readBool(STORAGE_KEYS.showLive, DEFAULTS.showLive),
    showAnalytics: readBool(STORAGE_KEYS.showAnalytics, DEFAULTS.showAnalytics),
  };
}

export function useUiPrefs() {
  const [prefs, setPrefs] = useState<UiPrefs>(readUiPrefs);

  useEffect(() => {
    const unlisten = listen<UiPrefs>(UI_PREFS_EVENT, (event) => {
      setPrefs(event.payload);
    });
    return () => {
      unlisten.then((fn) => fn());
    };
  }, []);

  const update = useCallback(async (patch: Partial<UiPrefs>) => {
    const next = { ...readUiPrefs(), ...patch };
    try {
      localStorage.setItem(STORAGE_KEYS.layoutMode, next.layoutMode);
      localStorage.setItem(STORAGE_KEYS.timeMode, next.timeMode);
      localStorage.setItem(STORAGE_KEYS.showLive, String(next.showLive));
      localStorage.setItem(STORAGE_KEYS.showAnalytics, String(next.showAnalytics));
    } catch {
      /* ignore */
    }
    setPrefs(next);
    try {
      await emit(UI_PREFS_EVENT, next);
    } catch {
      /* ignore — emit failures should not block the user */
    }
  }, []);

  return { prefs, update };
}
