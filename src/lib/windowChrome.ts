import type { WindowOptions } from "@tauri-apps/api/window";

export function isMacOS(
  userAgent = typeof navigator === "undefined" ? "" : navigator.userAgent,
): boolean {
  return userAgent.includes("Mac");
}

export function windowChromeOptionsFor(
  userAgent = typeof navigator === "undefined" ? "" : navigator.userAgent,
): Pick<WindowOptions, "decorations" | "titleBarStyle" | "hiddenTitle"> {
  return isMacOS(userAgent)
    ? { decorations: true, titleBarStyle: "overlay", hiddenTitle: true }
    : { decorations: false };
}

export const IS_MACOS = isMacOS();
export const WINDOW_CHROME_OPTIONS = windowChromeOptionsFor();
