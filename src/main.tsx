import React, { Suspense, useCallback } from "react";
import ReactDOM from "react-dom/client";
import { getCurrentWebview } from "@tauri-apps/api/webview";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { ToastProvider } from "./hooks/useToast";
import { useIntegrations } from "./hooks/useIntegrations";
import "./styles/index.css";

const App = React.lazy(() => import("./App"));
const RunsWindowView = React.lazy(() => import("./windows/RunsWindowView"));
const SessionsWindowView = React.lazy(
  () => import("./windows/SessionsWindowView"),
);
const LearningWindowView = React.lazy(
  () => import("./windows/LearningWindow"),
);
const PluginsWindowView = React.lazy(
  () => import("./windows/PluginsWindowView"),
);
const RestartWindowView = React.lazy(
  () => import("./windows/RestartWindowView"),
);
const SettingsWindowView = React.lazy(
  () => import("./windows/SettingsWindowView"),
);
const ReleaseNotesWindowView = React.lazy(
  () => import("./windows/ReleaseNotesWindow"),
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

function blockedWindowTitle(currentView: string | null): string {
  switch (currentView) {
    case "runs":
      return "Run History";
    case "sessions":
      return "Session Search";
    case "learning":
      return "Learning";
    case "plugins":
      return "Plugin Manager";
    case "restart":
      return "Restart Sessions";
    case "settings":
      return "Settings";
    default:
      return "Quill";
  }
}

function blockedWindowMessage(
  currentView: string | null,
  hasDetectedProvider: boolean,
  error: string | null,
): string {
  if (error) {
    return "Quill could not load provider status. Restart the app, then enable Claude Code or Codex from the QUILL menu.";
  }
  if (hasDetectedProvider) {
    return `Enable Claude Code or Codex from the QUILL menu before opening ${blockedWindowTitle(currentView)}.`;
  }
  return "Install Claude Code or Codex, then enable it from the QUILL menu before using this window.";
}

function BlockedWindow({
  title,
  heading = "No active provider",
  message,
}: {
  title: string;
  heading?: string;
  message: string;
}) {
  const handleClose = useCallback(async () => {
    await getCurrentWindow().close();
  }, []);

  return (
    <div className="blocked-window">
      <div className="blocked-window-titlebar" data-tauri-drag-region>
        <span className="blocked-window-title" data-tauri-drag-region>
          {title}
        </span>
        <button
          className="blocked-window-close"
          onClick={handleClose}
          aria-label="Close"
        >
          &times;
        </button>
      </div>
      <div className="blocked-window-body">
        <div className="integration-empty-state integration-empty-state--window">
          <p className="integration-empty-state__eyebrow">Providers</p>
          <h2 className="integration-empty-state__title">{heading}</h2>
          <p className="integration-empty-state__description">{message}</p>
        </div>
      </div>
    </div>
  );
}

function ProviderRoutedView() {
  const integrations = useIntegrations();
  const hasDetectedProvider = integrations.statuses.some(
    (status) => status.detectedCli,
  );
  const windowTitle = blockedWindowTitle(view);

  if (view && view !== "main" && view !== "settings" && view !== "release-notes") {
    if (integrations.loading) {
      return (
        <BlockedWindow
          title={windowTitle}
          heading="Checking integrations"
          message="Quill is loading provider status for this window."
        />
      );
    }

    if (!integrations.hasEnabledProvider) {
      return (
        <BlockedWindow
          title={windowTitle}
          message={blockedWindowMessage(
            view,
            hasDetectedProvider,
            integrations.error,
          )}
        />
      );
    }
  }

  return view === "runs" ? (
    <RunsWindowView />
  ) : view === "sessions" ? (
    <SessionsWindowView />
  ) : view === "learning" ? (
    <LearningWindowView />
  ) : view === "plugins" ? (
    <PluginsWindowView />
  ) : view === "restart" ? (
    <RestartWindowView />
  ) : view === "settings" ? (
    <SettingsWindowView />
  ) : view === "release-notes" ? (
    <ReleaseNotesWindowView />
  ) : (
    <App integrations={integrations} />
  );
}

function RoutedView() {
  return <ProviderRoutedView />;
}

ReactDOM.createRoot(document.getElementById("root")!).render(
  <React.StrictMode>
    <ToastProvider>
      <Suspense fallback={<div className="loading">Loading...</div>}>
        <RoutedView />
      </Suspense>
    </ToastProvider>
  </React.StrictMode>,
);
