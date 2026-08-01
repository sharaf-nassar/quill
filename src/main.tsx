import { setCrashReportingEnabled } from "./lib/crashReporting";
import React, { Suspense } from "react";
import ReactDOM from "react-dom/client";
import { reactErrorHandler } from "@sentry/react";
import { invoke } from "@tauri-apps/api/core";
import { getCurrentWebview } from "@tauri-apps/api/webview";
import { ToastProvider } from "./hooks/useToast";
import { useIntegrations } from "./hooks/useIntegrations";
import { openManageWindow } from "./lib/manageWindow";
import type { RuntimeSettings } from "./types";
import "./styles/index.css";

// In a plain browser (no Tauri runtime) during dev, install a mock IPC layer so
// the app renders with realistic fixture data. This is what lets `/impeccable live`
// drive the real app in a browser. The dynamic import + DEV guard keeps the mock
// and its fixtures out of production builds entirely.
if (import.meta.env.DEV && !("__TAURI_INTERNALS__" in window)) {
  const { installBrowserMock } = await import("./mocks/installBrowserMock");
  installBrowserMock();
}

// SDK stays uninitialized until we confirm the user has not opted out — short
// window at boot where errors aren't captured is the price of strict privacy.
void invoke<RuntimeSettings>("get_runtime_settings")
  .then((s) => setCrashReportingEnabled(s.crashReportingEnabled))
  .catch(() => {
    /* default to off when settings can't be read */
  });

const App = React.lazy(() => import("./App"));
const ReleaseNotesWindowView = React.lazy(
  () => import("./windows/ReleaseNotesWindow"),
);
const ManageWindowView = React.lazy(
  () => import("./windows/ManageWindowView"),
);

// Zoom with Ctrl+Plus / Ctrl+Minus / Ctrl+0 (per-window, persisted)
{
  const ZOOM_KEY = `quill-zoom-${new URLSearchParams(window.location.search).get("view") ?? "main"}`;
  const STEP = 0.1;
  const MIN = 0.5;
  const MAX = 2.0;

  const clampZoom = (value: number) => Math.max(MIN, Math.min(MAX, value));
  const parseZoom = (value: string | null) => {
    const parsed = value ? parseFloat(value) : NaN;
    return Number.isFinite(parsed) ? clampZoom(parsed) : 1;
  };
  const applyZoom = async (zoom: number) => {
    try {
      // Native webview zoom keeps pointer coordinates aligned with chart hover math.
      await getCurrentWebview().setZoom(zoom);
      document.documentElement.style.zoom = "";
    } catch {
      document.documentElement.style.zoom = String(zoom);
    }
  };

  let currentZoom = parseZoom(localStorage.getItem(ZOOM_KEY));
  void applyZoom(currentZoom);

  document.addEventListener("keydown", (e) => {
    if (!e.ctrlKey && !e.metaKey) return;

    if (e.key === "f") {
      e.preventDefault();
      return;
    }

    let next: number | null = null;

    if (e.key === "=" || e.key === "+") {
      next = Math.min(currentZoom + STEP, MAX);
    } else if (e.key === "-") {
      next = Math.max(currentZoom - STEP, MIN);
    } else if (e.key === "0") {
      next = 1;
    }

    if (next !== null) {
      e.preventDefault();
      const rounded = clampZoom(Math.round(next * 10) / 10);
      currentZoom = rounded;
      void applyZoom(rounded);
      localStorage.setItem(ZOOM_KEY, String(rounded));
    }
  });
}

const params = new URLSearchParams(window.location.search);
const view = params.get("view");

// The widget paints its own rounded surface on a transparent, decorationless
// window, so the document must not paint one behind it. Other routes keep the
// opaque page background.
document.documentElement.dataset.view = view ?? "main";

// ⌘M / Ctrl+M opens the Manage workspace, focusing it when it already exists.
// App-scoped on purpose: a global shortcut would collide with macOS minimize,
// and the widget is the only surface that needs the entry point.
if (view === null) {
  document.addEventListener("keydown", (e) => {
    if (!(e.metaKey || e.ctrlKey) || e.altKey || e.shiftKey) return;
    if (e.key !== "m" && e.key !== "M") return;
    e.preventDefault();
    void openManageWindow();
  });
}

function MainAppView() {
  const integrations = useIntegrations();
  return <App integrations={integrations} />;
}

// Only main / manage / release-notes routes remain after the workspace
// consolidation, and all three are reachable without an enabled provider
// (Manage gates each section inline), so the former per-window provider
// blocking is gone.
function RoutedView() {
  if (view === "manage") {
    return <ManageWindowView />;
  }
  if (view === "release-notes") {
    return <ReleaseNotesWindowView />;
  }
  return <MainAppView />;
}

ReactDOM.createRoot(document.getElementById("root")!, {
  onUncaughtError: reactErrorHandler(),
  onCaughtError: reactErrorHandler(),
  onRecoverableError: reactErrorHandler(),
}).render(
  <React.StrictMode>
    <ToastProvider>
      <Suspense fallback={<div className="loading">Loading...</div>}>
        <RoutedView />
      </Suspense>
    </ToastProvider>
  </React.StrictMode>,
);
