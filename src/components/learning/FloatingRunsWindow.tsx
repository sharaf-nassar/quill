import { useEffect, useRef } from "react";
import { WebviewWindow } from "@tauri-apps/api/webviewWindow";

interface FloatingRunsWindowProps {
  onClose: () => void;
}

function FloatingRunsWindow({ onClose }: FloatingRunsWindowProps) {
  const createdRef = useRef(false);

  useEffect(() => {
    if (createdRef.current) return;
    createdRef.current = true;

    let win: WebviewWindow | null = null;

    (async () => {
      const existing = await WebviewWindow.getByLabel("runs");
      if (existing) {
        await existing.show();
        await existing.setFocus();
        win = existing;
        return;
      }

      win = new WebviewWindow("runs", {
        url: "/?view=runs",
        title: "Run History",
        width: 320,
        height: 400,
        minWidth: 240,
        minHeight: 200,
        decorations: false,
        transparent: true,
        resizable: true,
        alwaysOnTop: true,
      });

      win.once("tauri://error", () => {
        onClose();
      });
    })();

    return () => {
      win?.close();
    };
  }, [onClose]);

  return null;
}

export default FloatingRunsWindow;
