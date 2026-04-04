import { useState, useEffect, useCallback, useMemo } from "react";
import { getVersion } from "@tauri-apps/api/app";
import { listen } from "@tauri-apps/api/event";
import { WebviewWindow } from "@tauri-apps/api/webviewWindow";
import ConfirmDialog from "./ConfirmDialog";
import ProviderMenu from "./integrations/ProviderMenu";
import type { UseIntegrationsResult } from "../hooks/useIntegrations";
import { useToast } from "../hooks/useToast";
import type { IntegrationProvider, PendingUpdate } from "../types";

interface PendingProviderAction {
  provider: IntegrationProvider;
  nextEnabled: boolean;
}

function providerLabel(provider: IntegrationProvider): string {
  return provider === "claude" ? "Claude Code" : "Codex";
}

function providerActionCopy(action: PendingProviderAction) {
  const label = providerLabel(action.provider);
  if (action.nextEnabled) {
    return {
      title: `Enable ${label}?`,
      description:
        `Quill will install its ${label} integration assets, including hooks, commands, MCP configuration, and managed instruction blocks.`,
      confirmLabel: `Enable ${label}`,
      destructive: false,
    };
  }

  return {
    title: `Disable ${label}?`,
    description:
      `Quill will remove every ${label} integration asset it installed, including hooks, commands, MCP entries, and managed instruction blocks. Historical Quill data stays in the app.`,
    confirmLabel: `Disable ${label}`,
    destructive: true,
  };
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
  integrations: UseIntegrationsResult;
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
  integrations,
}: TitleBarProps) {
  const { toast } = useToast();
  const [version, setVersion] = useState("");
  const [pluginUpdateCount, setPluginUpdateCount] = useState(0);
  const [menuOpen, setMenuOpen] = useState(false);
  const [pendingProviderAction, setPendingProviderAction] =
    useState<PendingProviderAction | null>(null);

  const {
    statuses,
    loading: providersLoading,
    error: providerError,
    hasEnabledProvider,
    inFlightProviders,
    enableProvider,
    disableProvider,
  } = integrations;

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

  useEffect(() => {
    if (!menuOpen) return;
    const handleKeyDown = (event: KeyboardEvent) => {
      if (event.key === "Escape" && !pendingProviderAction) {
        setMenuOpen(false);
      }
    };
    document.addEventListener("keydown", handleKeyDown);
    return () => document.removeEventListener("keydown", handleKeyDown);
  }, [menuOpen, pendingProviderAction]);

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

  const handleToggleMenu = useCallback(() => {
    setMenuOpen((prev) => {
      if (prev) {
        setPendingProviderAction(null);
      }
      return !prev;
    });
  }, []);

  const handleRequestToggle = useCallback(
    (provider: IntegrationProvider, nextEnabled: boolean) => {
      setPendingProviderAction({ provider, nextEnabled });
    },
    [],
  );

  const handleConfirmProviderAction = useCallback(async () => {
    if (!pendingProviderAction) return;

    const { provider, nextEnabled } = pendingProviderAction;
    const label = providerLabel(provider);

    try {
      if (nextEnabled) {
        await enableProvider(provider);
      } else {
        await disableProvider(provider);
      }
      setPendingProviderAction(null);
      setMenuOpen(false);
    } catch (err) {
      toast(
        "error",
        `${nextEnabled ? "Enable" : "Disable"} failed for ${label}: ${String(err)}`,
      );
    }
  }, [disableProvider, enableProvider, pendingProviderAction, toast]);

  const confirmCopy = useMemo(
    () =>
      pendingProviderAction ? providerActionCopy(pendingProviderAction) : null,
    [pendingProviderAction],
  );

  const busyConfirmProvider = pendingProviderAction
    ? inFlightProviders.has(pendingProviderAction.provider)
    : false;

  const handleCloseWindow = useCallback(async () => {
    setMenuOpen(false);
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
            aria-haspopup="menu"
            aria-expanded={menuOpen}
            onClick={handleToggleMenu}
          >
            QUILL
          </button>
          {menuOpen && (
            <>
              <div
                className="provider-menu-backdrop"
                onMouseDown={() => {
                  setPendingProviderAction(null);
                  setMenuOpen(false);
                }}
              />
              <ProviderMenu
                statuses={statuses}
                loading={providersLoading}
                error={providerError}
                inFlightProviders={inFlightProviders}
                onRequestToggle={handleRequestToggle}
              />
            </>
          )}
        </div>
      </div>
      {pendingProviderAction && confirmCopy && (
        <ConfirmDialog
          open
          title={confirmCopy.title}
          description={confirmCopy.description}
          confirmLabel={confirmCopy.confirmLabel}
          destructive={confirmCopy.destructive}
          busy={busyConfirmProvider}
          onCancel={() => setPendingProviderAction(null)}
          onConfirm={handleConfirmProviderAction}
        />
      )}
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
