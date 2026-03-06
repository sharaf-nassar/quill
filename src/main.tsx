import React from "react";
import ReactDOM from "react-dom/client";
import App from "./App";
import RunsWindowView from "./windows/RunsWindowView";
import { ToastProvider } from "./hooks/useToast";
import "./styles/index.css";

const params = new URLSearchParams(window.location.search);
const view = params.get("view");

ReactDOM.createRoot(document.getElementById("root")!).render(
  <React.StrictMode>
    <ToastProvider>
      {view === "runs" ? <RunsWindowView /> : <App />}
    </ToastProvider>
  </React.StrictMode>,
);
