import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";
import { windowChromeOptionsFor } from "../src/lib/windowChrome.ts";

// @lat: [[window-chrome-tests#Window Chrome Test Specs#Platform policy]]
test("window chrome selects native macOS frames only on macOS", () => {
  assert.deepEqual(windowChromeOptionsFor("Macintosh"), {
    decorations: true,
    titleBarStyle: "overlay",
    hiddenTitle: true,
  });
  assert.deepEqual(windowChromeOptionsFor("X11; Linux x86_64"), {
    decorations: false,
  });
  assert.deepEqual(windowChromeOptionsFor("Windows NT 10.0"), {
    decorations: false,
  });

  for (const path of [
    "src/lib/manageWindow.ts",
    "src/components/settings/GeneralTab.tsx",
  ]) {
    assert.match(readFileSync(path, "utf8"), /\.\.\.WINDOW_CHROME_OPTIONS/);
  }
  assert.match(
    readFileSync("src/components/WindowResizeHandles.tsx", "utf8"),
    /if \(IS_MACOS\) return null/,
  );
});

// @lat: [[window-chrome-tests#Window Chrome Test Specs#macOS main-window override]]
test("macOS config repeats the complete main window and preserves transparency", () => {
  const base = JSON.parse(readFileSync("src-tauri/tauri.conf.json", "utf8"));
  const mac = JSON.parse(
    readFileSync("src-tauri/tauri.macos.conf.json", "utf8"),
  );
  const { decorations: _baseDecorations, ...baseWindow } = base.app.windows[0];
  const {
    decorations,
    titleBarStyle,
    hiddenTitle,
    ...macWindow
  } = mac.app.windows[0];

  assert.deepEqual(macWindow, baseWindow);
  assert.equal(decorations, true);
  assert.equal(titleBarStyle, "Overlay");
  assert.equal(hiddenTitle, true);
  assert.equal(mac.app.macOSPrivateApi, true);

  const native = readFileSync("src-tauri/src/window_chrome.rs", "utf8");
  assert.match(native, /\.on_window_ready\(hide_standard_buttons\)/);
  assert.match(native, /NSWindowButton::CloseButton/);
  assert.match(native, /NSWindowButton::MiniaturizeButton/);
  assert.match(native, /NSWindowButton::ZoomButton/);
  assert.match(
    readFileSync("src-tauri/src/lib.rs", "utf8"),
    /StateFlags::all\(\) & !StateFlags::DECORATIONS/,
  );
});
