// Installs a mock Tauri IPC layer so the app runs in a plain browser (no Tauri
// runtime) with fixture data. Called from src/main.tsx only in dev + non-Tauri,
// which is exactly the context `/impeccable live` uses to drive the real app.
// `mockIPC` installs a fake `window.__TAURI_INTERNALS__`, so the app's existing
// `invoke()` / `listen()` calls route here without any call-site changes.

import { mockIPC, mockWindows } from "@tauri-apps/api/mocks";
import { handleInvoke } from "./ipcFixtures";

const BADGE_ID = "quill-mock-data-badge";

function addMockBadge(): void {
  if (document.getElementById(BADGE_ID)) return;
  const mount = () => {
    if (document.getElementById(BADGE_ID)) return;
    const badge = document.createElement("div");
    badge.id = BADGE_ID;
    badge.textContent = "MOCK DATA";
    badge.setAttribute("aria-hidden", "true");
    Object.assign(badge.style, {
      position: "fixed",
      right: "6px",
      bottom: "6px",
      padding: "2px 6px",
      font: "700 8px/1 ui-monospace, SFMono-Regular, Menlo, monospace",
      letterSpacing: "0.12em",
      color: "rgba(251, 191, 36, 0.85)",
      background: "rgba(251, 191, 36, 0.10)",
      border: "1px solid rgba(251, 191, 36, 0.28)",
      borderRadius: "2px",
      pointerEvents: "none",
      zIndex: "2147483647",
    } satisfies Partial<CSSStyleDeclaration>);
    document.body.appendChild(badge);
  };
  if (document.body) mount();
  else document.addEventListener("DOMContentLoaded", mount, { once: true });
}

/**
 * Route all Tauri IPC to the fixture handler and register a single mock window
 * so `getCurrentWindow()` / `getCurrentWebview()` resolve instead of throwing.
 */
export function installBrowserMock(): void {
  mockWindows("main");
  mockIPC((cmd, args) => handleInvoke(cmd, args as Record<string, unknown> | undefined));
  (window as unknown as { __QUILL_BROWSER_MOCK__?: boolean }).__QUILL_BROWSER_MOCK__ = true;
  addMockBadge();
  console.info("[quill] browser mock IPC installed — running with fixture data");
}
