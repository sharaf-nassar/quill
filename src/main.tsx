import React, { Suspense } from "react";
import ReactDOM from "react-dom/client";
import { ToastProvider } from "./hooks/useToast";
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

// Zoom with Ctrl+Plus / Ctrl+Minus / Ctrl+0 (per-window, persisted)
{
  const ZOOM_KEY = `quill-zoom-${new URLSearchParams(window.location.search).get("view") ?? "main"}`;
  const STEP = 0.1;
  const MIN = 0.5;
  const MAX = 2.0;

  const saved = localStorage.getItem(ZOOM_KEY);
  if (saved) document.documentElement.style.zoom = saved;

  document.addEventListener("keydown", (e) => {
    if (!e.ctrlKey && !e.metaKey) return;

    // Block webview find-in-page (no search UI exists)
    if (e.key === "f") {
      e.preventDefault();
      return;
    }

    const current = parseFloat(document.documentElement.style.zoom || "1");
    let next: number | null = null;

    if (e.key === "=" || e.key === "+") {
      next = Math.min(current + STEP, MAX);
    } else if (e.key === "-") {
      next = Math.max(current - STEP, MIN);
    } else if (e.key === "0") {
      next = 1;
    }

    if (next !== null) {
      e.preventDefault();
      const rounded = Math.round(next * 10) / 10;
      document.documentElement.style.zoom = String(rounded);
      localStorage.setItem(ZOOM_KEY, String(rounded));
    }
  });
}

const params = new URLSearchParams(window.location.search);
const view = params.get("view");

ReactDOM.createRoot(document.getElementById("root")!).render(
  <React.StrictMode>
    <ToastProvider>
      <Suspense fallback={<div className="loading">Loading…</div>}>
        {view === "runs" ? (
          <RunsWindowView />
        ) : view === "sessions" ? (
          <SessionsWindowView />
        ) : view === "learning" ? (
          <LearningWindowView />
        ) : view === "plugins" ? (
          <PluginsWindowView />
        ) : view === "restart" ? (
          <RestartWindowView />
        ) : (
          <App />
        )}
      </Suspense>
    </ToastProvider>
  </React.StrictMode>,
);
