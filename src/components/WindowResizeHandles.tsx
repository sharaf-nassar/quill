// WindowResizeHandles — the Linux/Windows resize border.
//
// Linux and Windows paint every app window on `decorations: false`, so the
// window manager has no native frame to hit-test. macOS uses AppKit instead.
// `resizable: true` is inert on its own; these eight zones are the entire
// resize affordance. Each one hands the gesture straight to the compositor
// through `startResizeDragging`, so the drag is native — React never sees a
// mousemove and the window keeps resizing while the webview is busy.
//
// The zones are pointer-only (`aria-hidden`, no focus, no content) and the
// geometry in `.window-resize-handle*` keeps them clear of the host window's
// own chrome. That clearance is per-window, which is what `variant` selects:
// the widget's corner squares are sized to `.wg-shell`'s 12px radius, while
// Manage and release-notes put a close key much closer to the top-right than
// the widget's keycaps do and need smaller corners. Edge width is shared —
// the tightest control in the app (Manage's close key, 5px below the top)
// leaves room for exactly 5px on every window.

import { getCurrentWindow } from "@tauri-apps/api/window";
import type { MouseEventHandler } from "react";
import { IS_MACOS } from "../lib/windowChrome";

/**
 * Which window's chrome the border geometry is tuned to. `widget` is the
 * 360px main window; `roomy` is Manage and release-notes.
 */
export type WindowResizeVariant = "widget" | "roomy";

interface WindowResizeHandlesProps {
  variant?: WindowResizeVariant;
}

/** The eight directions Tauri accepts for a border drag. */
type ResizeDirection =
  | "North"
  | "NorthEast"
  | "East"
  | "SouthEast"
  | "South"
  | "SouthWest"
  | "West"
  | "NorthWest";

const HANDLES: ReadonlyArray<{
  modifier: string;
  direction: ResizeDirection;
}> = [
  { modifier: "north", direction: "North" },
  { modifier: "east", direction: "East" },
  { modifier: "south", direction: "South" },
  { modifier: "west", direction: "West" },
  // Corners paint after the edges they overlap, so the diagonal cursor wins
  // on the squares the two meet in.
  { modifier: "north-east", direction: "NorthEast" },
  { modifier: "south-east", direction: "SouthEast" },
  { modifier: "south-west", direction: "SouthWest" },
  { modifier: "north-west", direction: "NorthWest" },
];

function startResize(
  direction: ResizeDirection,
): MouseEventHandler<HTMLDivElement> {
  return (event) => {
    // Left button only — right-click belongs to the shell's Refresh/Quit menu.
    if (event.button !== 0) return;
    event.preventDefault();
    event.stopPropagation();
    // Outside a real Tauri window (browser mock) there is nothing to drag, and
    // a rejected IPC call must not surface as an unhandled rejection.
    void getCurrentWindow()
      .startResizeDragging(direction)
      .catch(() => undefined);
  };
}

function WindowResizeHandles({ variant = "widget" }: WindowResizeHandlesProps) {
  if (IS_MACOS) return null;

  // The base class carries the widget geometry, so the main window renders
  // byte-identical markup to the single-window version; only `roomy` adds a
  // modifier that retunes the custom properties the zones read.
  const overlayClass =
    variant === "roomy"
      ? "window-resize-handles window-resize-handles--roomy"
      : "window-resize-handles";

  return (
    <div className={overlayClass} aria-hidden="true">
      {HANDLES.map(({ modifier, direction }) => (
        <div
          key={direction}
          className={`window-resize-handle window-resize-handle--${modifier}`}
          onMouseDown={startResize(direction)}
        />
      ))}
    </div>
  );
}

export default WindowResizeHandles;
