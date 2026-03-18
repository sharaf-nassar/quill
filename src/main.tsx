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
        ) : (
          <App />
        )}
      </Suspense>
    </ToastProvider>
  </React.StrictMode>,
);
