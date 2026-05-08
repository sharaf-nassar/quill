import { useState, useEffect, useCallback } from "react";
import { getVersion } from "@tauri-apps/api/app";
import { listen } from "@tauri-apps/api/event";
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

const SparkIcon = () => (
  <svg {...SVG_PROPS}>
    <path d="M7 1.5L8.4 5.6 12.5 7 8.4 8.4 7 12.5 5.6 8.4 1.5 7 5.6 5.6Z" />
  </svg>
);

const SearchIcon = () => (
  <svg {...SVG_PROPS}>
    <circle cx="6" cy="6" r="3.4" />
    <line x1="8.6" y1="8.6" x2="11.6" y2="11.6" />
  </svg>
);

const PluginIcon = () => (
  <svg {...SVG_PROPS}>
    <rect x="2.5" y="2.5" width="3.6" height="3.6" rx="0.5" />
    <rect x="7.9" y="2.5" width="3.6" height="3.6" rx="0.5" />
    <rect x="2.5" y="7.9" width="3.6" height="3.6" rx="0.5" />
    <rect x="7.9" y="7.9" width="3.6" height="3.6" rx="0.5" />
  </svg>
);

const RefreshIcon = () => (
  <svg {...SVG_PROPS}>
    <path d="M11.5 6.6A4.5 4.5 0 1 0 11.7 8.6" />
    <path d="M11.5 2.5V6.6H7.4" />
  </svg>
);

const GearIcon = () => (
  <svg {...SVG_PROPS}>
    <circle cx="7" cy="7" r="2" />
    <path d="M7 1.4V3M7 11V12.6M1.4 7H3M11 7H12.6M3.05 3.05L4.18 4.18M9.82 9.82L10.95 10.95M3.05 10.95L4.18 9.82M9.82 4.18L10.95 3.05" />
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

  const handleOpenSettings = useCallback(async () => {
    const existing = await WebviewWindow.getByLabel("settings");
    if (existing) {
      await existing.show();
      await existing.setFocus();
      return;
    }
    new WebviewWindow("settings", {
      url: "/?view=settings",
      title: "Settings",
      // Default width matches minWidth so the five top tabs (General …
      // Performance) always fit on a single row without horizontal
      // scroll or flex wrap on first launch, with a small buffer past
      // the last tab.
      width: 540,
      height: 620,
      minWidth: 540,
      minHeight: 480,
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
          <span aria-hidden="true" className="view-tab-divider" />
          <button
            className="view-tab view-tab--icon"
            onClick={handleOpenLearning}
            aria-label="Open learning"
            title="Learning"
            disabled={featuresDisabled}
          >
            <SparkIcon />
          </button>
          <button
            className="view-tab view-tab--icon"
            onClick={handleOpenSessions}
            aria-label="Search sessions"
            title="Search sessions"
            disabled={featuresDisabled}
          >
            <SearchIcon />
          </button>
          <button
            className="view-tab view-tab--icon view-tab--plugins"
            onClick={handleOpenPlugins}
            aria-label="Plugin Manager"
            title="Plugin Manager"
            disabled={featuresDisabled}
          >
            <PluginIcon />
            {pluginUpdateCount > 0 && (
              <span className="plugins-update-badge">{pluginUpdateCount}</span>
            )}
          </button>
          <button
            className="view-tab view-tab--icon"
            onClick={handleOpenRestart}
            aria-label="Restart sessions"
            title="Restart sessions"
            disabled={featuresDisabled}
          >
            <RefreshIcon />
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
        <button
          className="titlebar-cog"
          aria-label="Open settings"
          title="Open settings"
          onClick={() => void handleOpenSettings()}
        >
          <GearIcon />
        </button>
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
