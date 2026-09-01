import { setCrashReportingEnabled } from "./lib/crashReporting";
import React, { Suspense, useEffect, useRef, useState } from "react";
import ReactDOM from "react-dom/client";
import { reactErrorHandler } from "@sentry/react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWebview } from "@tauri-apps/api/webview";
import { ToastProvider } from "./hooks/useToast";
import { useIntegrations } from "./hooks/useIntegrations";
import WindowResizeHandles from "./components/WindowResizeHandles";
import MigrationView from "./components/MigrationView";
import RootErrorBoundary from "./components/RootErrorBoundary";
import { openManageWindow } from "./lib/manageWindow";
import type { RuntimeSettings, StartupStatus } from "./types";
import "./styles/index.css";

// In a plain browser (no Tauri runtime) during dev, install a mock IPC layer so
// the app renders with realistic fixture data. This is what lets `/impeccable live`
// drive the real app in a browser. The dynamic import + DEV guard keeps the mock
// and its fixtures out of production builds entirely.
if (import.meta.env.DEV && !("__TAURI_INTERNALS__" in window)) {
  const { installBrowserMock } = await import("./mocks/installBrowserMock");
  installBrowserMock();
}

// Dev-only stylesheet watchdog. A Vite restart mid-session (config edit,
// dependency re-optimization) can hand an already-open window a module graph
// whose CSS request lost the optimizer race: React keeps rendering while
// every rule is silently gone — the "unstyled widget" in
// docs/solutions/environment/second-vite-server-strips-dev-css.md. The
// design tokens are the
// oracle: when `--surface` is absent after load, reload once to fetch the
// repaired graph. The sessionStorage latch stops a loop when styles are
// genuinely broken, and clears on success so a later drop can heal again.
// ponytail: load-time check only; a mid-session style drop without a reload
// would need a MutationObserver on the style tags if it ever shows up.
if (import.meta.env.DEV) {
  const RETRY_KEY = "quill-dev-css-reload";
  window.addEventListener("load", () => {
    setTimeout(() => {
      const styled =
        getComputedStyle(document.documentElement)
          .getPropertyValue("--surface")
          .trim() !== "";
      if (styled) {
        sessionStorage.removeItem(RETRY_KEY);
        return;
      }
      if (sessionStorage.getItem(RETRY_KEY)) {
        console.error(
          "[quill] dev stylesheet still missing after a reload — check the vite server",
        );
        return;
      }
      sessionStorage.setItem(RETRY_KEY, "1");
      console.warn("[quill] dev stylesheet missing — reloading once to recover");
      location.reload();
    }, 300);
  });
}

// SDK stays uninitialized until the stored opt-in says otherwise — short
// window at boot where errors aren't captured is the price of strict privacy.
function syncCrashReportingPreference(): void {
  void invoke<RuntimeSettings>("get_runtime_settings")
    .then((s) => setCrashReportingEnabled(s.crashReportingEnabled))
    .catch(() => {
      /* stay off when settings can't be read */
    });
}

syncCrashReportingPreference();

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
const requestedView = params.get("view");
const view = requestedView ?? (getCurrentWebview().label === "migration" ? "migration" : null);

// The widget paints its own rounded surface on a transparent window, so the
// document must not paint one behind it. Other routes keep an opaque page.
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

// Linux and Windows need the custom resize border; it suppresses itself on
// macOS where AppKit owns frame hit-testing. Each route retains its fallback
// geometry because its zones must clear that window's custom chrome.
function MainAppView() {
  const integrations = useIntegrations();
  return (
    <>
      <App integrations={integrations} />
      <WindowResizeHandles />
    </>
  );
}

// Normal routes remain main / manage / release-notes after workspace
// consolidation; the transient migration label is gated above. All three are
// reachable without an enabled provider
// (Manage gates each section inline), so the former per-window provider
// blocking is gone.
function RoutedView() {
  if (view === "manage") {
    return (
      <>
        <ManageWindowView />
        <WindowResizeHandles variant="roomy" />
      </>
    );
  }
  if (view === "release-notes") {
    return (
      <>
        <ReleaseNotesWindowView />
        <WindowResizeHandles variant="roomy" />
      </>
    );
  }
  return <MainAppView />;
}

const INITIAL_STARTUP_STATUS: StartupStatus = {
  state: "starting",
  stage: "Starting Quill",
  detail: "Opening local services",
  completedBytes: null,
  totalBytes: null,
};

// @lat: [[frontend#Frontend#Entry Point]]
function StartupGate() {
  const [status, setStatus] = useState(INITIAL_STARTUP_STATUS);
  const sawStatusEvent = useRef(false);

  useEffect(() => {
    let active = true;
    let stopListening: (() => void) | undefined;
    const applyStatus = (next: StartupStatus) => {
      if (!active) return;
      setStatus((current) => (current.state === "ready" ? current : next));
    };

    void listen<StartupStatus>("startup-status", ({ payload }) => {
      sawStatusEvent.current = true;
      applyStatus(payload);
    }).then((unlisten) => {
      if (active) stopListening = unlisten;
      else unlisten();
    });

    void invoke<StartupStatus>("get_startup_status")
      .then((snapshot) => {
        if (!sawStatusEvent.current) applyStatus(snapshot);
      })
      .catch((error) => {
        if (!sawStatusEvent.current) {
          applyStatus({
            state: "error",
            stage: "Quill could not read startup status",
            detail: String(error),
            completedBytes: null,
            totalBytes: null,
          });
        }
      });

    return () => {
      active = false;
      stopListening?.();
    };
  }, []);

  useEffect(() => {
    if (status.state === "ready") syncCrashReportingPreference();
  }, [status.state]);

  if (status.state !== "ready") return <MigrationView status={status} />;
  return view === "migration" ? null : <RoutedView />;
}

ReactDOM.createRoot(document.getElementById("root")!, {
  onUncaughtError: reactErrorHandler(),
  onCaughtError: reactErrorHandler(),
  onRecoverableError: reactErrorHandler(),
}).render(
  <React.StrictMode>
    <ToastProvider>
      <RootErrorBoundary>
        <Suspense fallback={<div className="loading">Loading...</div>}>
          <StartupGate />
        </Suspense>
      </RootErrorBoundary>
    </ToastProvider>
  </React.StrictMode>,
);
