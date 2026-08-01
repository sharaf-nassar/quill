// WindowResizeHandles — the resize border of a decorationless window.
//
// The widget paints its own rounded surface on a `decorations: false` window,
// so the window manager has no native frame to hit-test. `resizable: true` in
// `tauri.conf.json` is inert on its own; these eight zones are the widget's
// entire resize affordance. Each one hands the gesture straight to the
// compositor through `startResizeDragging`, so the drag is native — React
// never sees a mousemove and the window keeps resizing while the webview is
// busy.
//
// The zones are pointer-only (`aria-hidden`, no focus, no content) and the
// geometry in `.window-resize-handle*` keeps them clear of the widget's own
// chrome: the titlebar drag region, the update button, the sync pill, the
// always-on-top toggle, and the settings and close keys all stay clickable.

import { getCurrentWindow } from "@tauri-apps/api/window";
import type { MouseEventHandler } from "react";

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

function WindowResizeHandles() {
  return (
    <div className="window-resize-handles" aria-hidden="true">
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
