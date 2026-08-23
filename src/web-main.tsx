import React from "react";
import ReactDOM from "react-dom/client";
import { installHttpTransport } from "./web/httpTransport";
import "./styles/index.css";

// This must run before the monitor shell begins importing shared widget code.
installHttpTransport();
const { default: WebShell } = await import("./web/WebShell");

const root = document.getElementById("root");
if (root === null) throw new Error("Web UI root is missing.");

ReactDOM.createRoot(root).render(
  <React.StrictMode>
    <WebShell />
  </React.StrictMode>,
);
