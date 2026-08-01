// Single entry point to the Manage workspace.
//
// The widget reaches Manage from three places — the titlebar settings key, the
// ⌘M / Ctrl+M accelerator, and the Usage view footer — and all three must
// focus the existing window rather than stack a second one. The `?section=`
// deep link only applies at creation, so an already-open workspace is
// navigated with the `manage:navigate` event instead.

import { emit } from "@tauri-apps/api/event";
import { WebviewWindow } from "@tauri-apps/api/webviewWindow";

const MANAGE_LABEL = "manage";

/**
 * Show and focus the Manage workspace, creating it when it does not exist.
 * `section` selects the rail section (for example `"settings"`).
 */
export async function openManageWindow(section?: string): Promise<void> {
  const existing = await WebviewWindow.getByLabel(MANAGE_LABEL);
  if (existing) {
    await existing.show();
    await existing.setFocus();
    if (section) {
      await emit("manage:navigate", section);
    }
    return;
  }
  new WebviewWindow(MANAGE_LABEL, {
    url: section ? `/?view=manage&section=${section}` : "/?view=manage",
    title: "Manage",
    width: 960,
    height: 680,
    minWidth: 720,
    minHeight: 480,
    decorations: false,
    transparent: true,
    resizable: true,
  });
}
