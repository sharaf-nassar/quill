import { useState, useEffect, useCallback } from "react";
import { getVersion } from "@tauri-apps/api/app";
import { listen, emit } from "@tauri-apps/api/event";
import { WebviewWindow } from "@tauri-apps/api/webviewWindow";
import type { UseIntegrationsResult } from "../hooks/useIntegrations";
import type { PendingUpdate } from "../types";

interface TitleBarProps {
  showLive: boolean;
  showAnalytics: boolean;
  onToggleLive: (on: boolean) => void;
  onToggleAnalytics: (on: boolean) => void;
  onClose: () => void;
  pendingUpdate: PendingUpdate | null;
  updating: boolean;
  onUpdate: () => void;
  integrations: UseIntegrationsResult;
}

const SVG_PROPS = {
  viewBox: "0 0 14 14",
  width: 14,
  height: 14,
  fill: "none",
  stroke: "currentColor",
  strokeWidth: 1.5,
  strokeLinecap: "round" as const,
  strokeLinejoin: "round" as const,
  "aria-hidden": true,
  focusable: false,
};

const ManageIcon = () => (
  <svg {...SVG_PROPS}>
    <rect x="1.8" y="2.5" width="10.4" height="9" rx="1.4" />
    <line x1="5.4" y1="2.5" x2="5.4" y2="11.5" />
    <line x1="3" y1="5" x2="4.1" y2="5" />
    <line x1="3" y1="7" x2="4.1" y2="7" />
  </svg>
);

const SettingsIcon = () => (
  <svg {...SVG_PROPS}>
    <line x1="2" y1="3.5" x2="12" y2="3.5" />
    <line x1="9" y1="2" x2="9" y2="5" />
    <line x1="2" y1="7" x2="12" y2="7" />
    <line x1="5" y1="5.5" x2="5" y2="8.5" />
    <line x1="2" y1="10.5" x2="12" y2="10.5" />
    <line x1="9" y1="9" x2="9" y2="12" />
  </svg>
);

const CloseIcon = () => (
  <svg {...SVG_PROPS}>
    <path d="M3.2 3.2L10.8 10.8M10.8 3.2L3.2 10.8" />
  </svg>
);

function TitleBar({
  showLive,
  showAnalytics,
  onToggleLive,
  onToggleAnalytics,
  onClose,
  pendingUpdate,
  updating,
  onUpdate,
  integrations,
}: TitleBarProps) {
  const [version, setVersion] = useState("");
  const [pluginUpdateCount, setPluginUpdateCount] = useState(0);

  const { hasEnabledProvider, loading: providersLoading } = integrations;

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

  const liveDisabled = !showLive && (providersLoading || !hasEnabledProvider);

  const handleOpenManage = useCallback(async (section?: string) => {
    const existing = await WebviewWindow.getByLabel("manage");
    if (existing) {
      await existing.show();
      await existing.setFocus();
      // Already open: ask it to navigate (e.g. cog -> Settings) since the
      // ?section= deep-link only applies on first creation.
      if (section) {
        await emit("manage:navigate", section);
      }
      return;
    }
    new WebviewWindow("manage", {
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
  }, []);

  const handleOpenReleaseNotes = useCallback(async () => {
    const existing = await WebviewWindow.getByLabel("release-notes");
    if (existing) {
      await existing.show();
      await existing.setFocus();
      return;
    }
    new WebviewWindow("release-notes", {
      url: "/?view=release-notes",
      title: "Release Notes",
      width: 560,
      height: 600,
      minWidth: 380,
      minHeight: 360,
      decorations: false,
      transparent: true,
      resizable: true,
    });
  }, []);

  const handleCloseWindow = useCallback(async () => {
    await onClose();
  }, [onClose]);

  return (
    <div className="titlebar" data-tauri-drag-region>
      <div className="titlebar-left" data-tauri-drag-region>
        <div className="view-toggle">
          <button
            className={`view-tab${showLive ? " active" : ""}`}
            onClick={() => onToggleLive(!showLive)}
            disabled={liveDisabled}
          >
            Live
          </button>
          <button
            className={`view-tab${showAnalytics ? " active" : ""}`}
            onClick={() => onToggleAnalytics(!showAnalytics)}
          >
            Analytics
          </button>
          <span aria-hidden="true" className="view-tab-divider" />
          <button
            className="view-tab view-tab--icon view-tab--plugins"
            onClick={() => void handleOpenManage()}
            aria-label="Open Tools workspace"
            title="Tools"
          >
            <ManageIcon />
            {pluginUpdateCount > 0 && (
              <span className="plugins-update-badge">{pluginUpdateCount}</span>
            )}
          </button>
        </div>
      </div>
      <div className="titlebar-center" data-tauri-drag-region>
        <span className="titlebar-title" data-tauri-drag-region>
          QUILL
        </span>
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
        {version && (
          <button
            type="button"
            className="titlebar-version"
            onClick={() => void handleOpenReleaseNotes()}
            aria-label={`Quill version ${version}, view release notes`}
            title="View release notes"
          >
            v{version}
          </button>
        )}
        <button
          className="titlebar-cog"
          aria-label="Open settings"
          title="Open settings"
          onClick={() => void handleOpenManage("settings")}
        >
          <SettingsIcon />
        </button>
        <span aria-hidden="true" className="titlebar-divider" />
        <button
          className="titlebar-close"
          onClick={() => void handleCloseWindow()}
          aria-label="Close window"
        >
          <CloseIcon />
        </button>
      </div>
    </div>
  );
}

export default TitleBar;
