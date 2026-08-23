import { installHttpTransport } from "./web/httpTransport";
import "./styles/index.css";

// This must run before the monitor shell begins importing shared widget code.
installHttpTransport();

const root = document.getElementById("root");
if (root === null) throw new Error("Web UI root is missing.");

const monitor = document.createElement("main");
monitor.setAttribute("aria-label", "Quill web monitor");
monitor.textContent = "Quill";
root.replaceChildren(monitor);
