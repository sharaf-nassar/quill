import { useState, useEffect, useCallback } from "react";
import { getVersion } from "@tauri-apps/api/app";
import { listen } from "@tauri-apps/api/event";
import {
  currentMonitor,
  getCurrentWindow,
} from "@tauri-apps/api/window";
import { WebviewWindow } from "@tauri-apps/api/webviewWindow";
import type { PendingUpdate } from "../types";

const INTEGRATIONS_WINDOW_LABEL = "integrations";
const INTEGRATIONS_WINDOW_WIDTH = 360;
const INTEGRATIONS_WINDOW_HEIGHT = 228;
const INTEGRATIONS_WINDOW_MARGIN = 8;
const INTEGRATIONS_WINDOW_OFFSET_Y = 30;

async function calcIntegrationsWindowPosition(): Promise<{ x: number; y: number }> {
  const parent = getCurrentWindow();
  const scale = await parent.scaleFactor();
  const physPos = await parent.outerPosition();
  const physSize = await parent.outerSize();
  const monitor = await currentMonitor();

  const pos = physPos.toLogical(scale);
  const width = physSize.width / scale;

  let x = pos.x + (width - INTEGRATIONS_WINDOW_WIDTH) / 2;
  let y = pos.y + INTEGRATIONS_WINDOW_OFFSET_Y;

  if (monitor) {
    const monitorScale = monitor.scaleFactor;
    const monitorX = monitor.position.x / monitorScale;
    const monitorY = monitor.position.y / monitorScale;
    const monitorWidth = monitor.size.width / monitorScale;
    const monitorHeight = monitor.size.height / monitorScale;
    const maxX =
      monitorX + monitorWidth - INTEGRATIONS_WINDOW_WIDTH - INTEGRATIONS_WINDOW_MARGIN;
    const maxY =
      monitorY + monitorHeight - INTEGRATIONS_WINDOW_HEIGHT - INTEGRATIONS_WINDOW_MARGIN;

    x = Math.min(
      Math.max(x, monitorX + INTEGRATIONS_WINDOW_MARGIN),
      Math.max(maxX, monitorX + INTEGRATIONS_WINDOW_MARGIN),
    );
    y = Math.min(
      Math.max(y, monitorY + INTEGRATIONS_WINDOW_MARGIN),
      Math.max(maxY, monitorY + INTEGRATIONS_WINDOW_MARGIN),
    );
  }

  return { x, y };
}

interface TitleBarProps {
  showLive: boolean;
  showAnalytics: boolean;
  onToggleLive: (on: boolean) => void;
  onToggleAnalytics: (on: boolean) => void;
  onClose: () => void;
  pendingUpdate: PendingUpdate | null;
  updating: boolean;
  onUpdate: () => void;
  providersLoading: boolean;
  hasEnabledProvider: boolean;
}

function TitleBar({
  showLive,
  showAnalytics,
  onToggleLive,
  onToggleAnalytics,
  onClose,
  pendingUpdate,
  updating,
  onUpdate,
  providersLoading,
  hasEnabledProvider,
}: TitleBarProps) {
  const [version, setVersion] = useState("");
  const [pluginUpdateCount, setPluginUpdateCount] = useState(0);
  const [providerWindowOpen, setProviderWindowOpen] = useState(false);

  useEffect(() => {
    getVersion().then(setVersion).catch(() => {});
  }, []);

  useEffect(() => {
    const unlisten = listen<number>("plugin-updates-available", (event) => {
      setPluginUpdateCount(event.payload);
    });
    return () => {
      unlisten.then((fn) => fn());
    };
  }, []);

  const featuresDisabled = providersLoading || !hasEnabledProvider;

  const handleOpenSessions = useCallback(async () => {
    const existing = await WebviewWindow.getByLabel("sessions");
    if (existing) {
      await existing.show();
      await existing.setFocus();
      return;
    }
    new WebviewWindow("sessions", {
      url: "/?view=sessions",
      title: "Session Search",
      width: 1000,
      height: 650,
      minWidth: 600,
      minHeight: 400,
      decorations: false,
      transparent: true,
      resizable: true,
    });
  }, []);

  const handleOpenLearning = useCallback(async () => {
    const existing = await WebviewWindow.getByLabel("learning");
    if (existing) {
      await existing.show();
      await existing.setFocus();
      return;
    }
    new WebviewWindow("learning", {
      url: "/?view=learning",
      title: "Learning",
      width: 500,
      height: 600,
      minWidth: 400,
      minHeight: 400,
      decorations: false,
      transparent: true,
      resizable: true,
    });
  }, []);

  const handleOpenRestart = useCallback(async () => {
    const existing = await WebviewWindow.getByLabel("restart");
    if (existing) {
      await existing.show();
      await existing.setFocus();
      return;
    }
    new WebviewWindow("restart", {
      url: "/?view=restart",
      title: "Restart Sessions",
      width: 420,
      height: 400,
      minWidth: 320,
      minHeight: 250,
      decorations: false,
      transparent: true,
      resizable: true,
    });
  }, []);

  const handleOpenPlugins = useCallback(async () => {
    const existing = await WebviewWindow.getByLabel("plugins");
    if (existing) {
      await existing.show();
      await existing.setFocus();
      return;
    }
    new WebviewWindow("plugins", {
      url: "/?view=plugins",
      title: "Plugin Manager",
      width: 700,
      height: 550,
      minWidth: 500,
      minHeight: 400,
      decorations: false,
      transparent: true,
      resizable: true,
    });
  }, []);

  const handleToggleProviderWindow = useCallback(async () => {
    const existing = await WebviewWindow.getByLabel(INTEGRATIONS_WINDOW_LABEL);
    if (existing) {
      await existing.close();
      return;
    }

    const { x, y } = await calcIntegrationsWindowPosition();
    const win = new WebviewWindow(INTEGRATIONS_WINDOW_LABEL, {
      url: "/?view=integrations",
      title: "Integrations",
      width: INTEGRATIONS_WINDOW_WIDTH,
      height: INTEGRATIONS_WINDOW_HEIGHT,
      minWidth: INTEGRATIONS_WINDOW_WIDTH,
      minHeight: INTEGRATIONS_WINDOW_HEIGHT,
      maxWidth: INTEGRATIONS_WINDOW_WIDTH,
      maxHeight: INTEGRATIONS_WINDOW_HEIGHT,
      x,
      y,
      decorations: false,
      transparent: true,
      resizable: false,
      alwaysOnTop: true,
      focus: true,
      skipTaskbar: true,
      shadow: true,
    });

    setProviderWindowOpen(true);
    void win.once("tauri://destroyed", () => setProviderWindowOpen(false));
    void win.once("tauri://error", () => setProviderWindowOpen(false));
  }, []);

  const handleCloseWindow = useCallback(async () => {
    const integrationsWindow = await WebviewWindow.getByLabel(
      INTEGRATIONS_WINDOW_LABEL,
    );
    if (integrationsWindow) {
      await integrationsWindow.close();
    }
    await onClose();
  }, [onClose]);

  return (
    <div className="titlebar" data-tauri-drag-region>
      <div className="titlebar-left" data-tauri-drag-region>
        <div className="view-toggle">
          <button
            className={`view-tab${showLive ? " active" : ""}`}
            onClick={() => onToggleLive(!showLive)}
            disabled={featuresDisabled}
          >
            Live
          </button>
          <button
            className={`view-tab${showAnalytics ? " active" : ""}`}
            onClick={() => onToggleAnalytics(!showAnalytics)}
            disabled={featuresDisabled}
          >
            Analytics
          </button>
          <button
            className="view-tab view-tab--learning"
            onClick={handleOpenLearning}
            aria-label="Open learning"
            title="Learning"
            disabled={featuresDisabled}
          >
            &#x1F9E0;
          </button>
          <button
            className="view-tab view-tab--search"
            onClick={handleOpenSessions}
            aria-label="Search sessions"
            title="Search sessions"
            disabled={featuresDisabled}
          >
            &#8981;
          </button>
          <button
            className="view-tab view-tab--plugins"
            onClick={handleOpenPlugins}
            aria-label="Plugin Manager"
            title="Plugin Manager"
            disabled={featuresDisabled}
          >
            &#9881;
            {pluginUpdateCount > 0 && (
              <span className="plugins-update-badge">{pluginUpdateCount}</span>
            )}
          </button>
          <button
            className="view-tab view-tab--restart"
            onClick={handleOpenRestart}
            aria-label="Restart sessions"
            title="Restart sessions"
            disabled={featuresDisabled}
          >
            &#8635;
          </button>
        </div>
      </div>
      <div className="titlebar-center" data-tauri-drag-region>
        <div className="titlebar-menu-anchor">
          <button
            className="titlebar-title-trigger"
            aria-haspopup="dialog"
            aria-expanded={providerWindowOpen}
            onClick={() => void handleToggleProviderWindow()}
          >
            QUILL
          </button>
        </div>
      </div>
      <div className="titlebar-right">
        {pendingUpdate && (
          <button
            className="titlebar-update-btn"
            onClick={onUpdate}
            disabled={updating}
            aria-label={`Update to version ${pendingUpdate.version}`}
          >
            {updating ? "Updating..." : `Update ${pendingUpdate.version}`}
          </button>
        )}
        {version && <span className="titlebar-version">v{version}</span>}
        <button
          className="titlebar-close"
          onClick={() => void handleCloseWindow()}
          aria-label="Close window"
        >
          &times;
        </button>
      </div>
    </div>
  );
}

export default TitleBar;
