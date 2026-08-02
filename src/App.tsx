// App — the widget shell.
//
// The main window *is* the widget now: an always-on-top instrument made of a
// titlebar and a single content column. The split-pane layout, the draggable
// divider, the fit-to-height CSS scaling system, arrow-key resize and every
// stored layout/size/split preference that fed them are gone — the window
// manager owns the geometry, the user drags it, and the shell scrolls into
// whatever size it is given. What survives here is the app-lifecycle work that
// has no other home: usage polling, the four-hour update check, the
// right-click Refresh/Quit menu, and close-to-tray.
//
// See specs/018-widget-ui-redesign/plan.md#Affected Components.

import { useState, useEffect, useCallback, useRef } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { check } from "@tauri-apps/plugin-updater";
import LimitsSection from "./components/widget/LimitsSection";
import ViewRegion from "./components/widget/ViewRegion";
import WidgetTitleBar from "./components/widget/WidgetTitleBar";
import type { UseIntegrationsResult } from "./hooks/useIntegrations";
import { useToast } from "./hooks/useToast";
import type { CpaConnectionStatus, UsageData, PendingUpdate } from "./types";

const USAGE_REFRESH_MS = 3 * 60_000;
const UPDATE_CHECK_MS = 4 * 60 * 60_000;

interface AppProps {
  integrations: UseIntegrationsResult;
}

function App({ integrations }: AppProps) {
  const { toast } = useToast();
  const [usageData, setUsageData] = useState<UsageData | null>(null);
  const [lastSyncAt, setLastSyncAt] = useState<number | null>(null);
  const [cpaConfigured, setCpaConfigured] = useState<boolean | null>(null);
  const [showMenu, setShowMenu] = useState(false);
  const [menuPos, setMenuPos] = useState({ x: 0, y: 0 });
  const [pendingUpdate, setPendingUpdate] = useState<PendingUpdate | null>(null);
  const [updating, setUpdating] = useState(false);
  const usageRequestRef = useRef<Promise<UsageData> | null>(null);
  const appliedUsageRequestRef = useRef<Promise<UsageData> | null>(null);
  const cpaStatusRequestRef = useRef<Promise<CpaConnectionStatus> | null>(null);

  const {
    statuses,
    loading: providersLoading,
    error: providersError,
    hasEnabledProvider,
    refresh: refreshIntegrations,
  } = integrations;
  const hasDetectedProvider = statuses.some((status) => status.detectedCli);
  const liveProviderKey = statuses
    .filter((status) => status.enabled)
    .map((status) => status.provider)
    .join(",");
  const hasUsageSource = hasEnabledProvider || cpaConfigured === true;
  const usageSourcesLoading = providersLoading || cpaConfigured === null;
  const cpaPoolProviders = new Set(
    (usageData?.cpa_pools ?? []).map((pool) => pool.provider),
  );
  const titlebarProviderErrors =
    usageData === null
      ? null
      : usageData.provider_errors.filter(
          (error) =>
            (error.source ?? "direct") !== "direct" ||
            !cpaPoolProviders.has(error.provider),
        );

  const requestUsageData = useCallback(() => {
    if (usageRequestRef.current) return usageRequestRef.current;

    const request = invoke<UsageData>("fetch_usage_data");
    usageRequestRef.current = request;
    const clearRequest = () => {
      if (usageRequestRef.current === request) usageRequestRef.current = null;
    };
    void request.then(clearRequest, clearRequest);
    return request;
  }, []);

  const refresh = useCallback(async (isActive: () => boolean = () => true) => {
    const request = requestUsageData();
    try {
      const data = await request;
      if (!isActive() || appliedUsageRequestRef.current === request) return;
      appliedUsageRequestRef.current = request;
      setUsageData(data);
      setLastSyncAt(Date.now());
    } catch (e) {
      if (!isActive() || appliedUsageRequestRef.current === request) return;
      appliedUsageRequestRef.current = request;
      toast("error", `Usage data fetch failed: ${e}`);
      setUsageData({
        buckets: [],
        provider_errors: [],
        provider_credits: [],
        cpa_accounts: [],
        cpa_pools: [],
        error: String(e),
      });
    }
  }, [requestUsageData, toast]);

  const refreshCpaConnectionStatus = useCallback(
    async (isActive: () => boolean = () => true) => {
      let request = cpaStatusRequestRef.current;
      if (!request) {
        request = invoke<CpaConnectionStatus>("get_cpa_connection_status");
        cpaStatusRequestRef.current = request;
        const clearRequest = () => {
          if (cpaStatusRequestRef.current === request) {
            cpaStatusRequestRef.current = null;
          }
        };
        void request.then(clearRequest, clearRequest);
      }

      try {
        const status = await request;
        if (isActive()) setCpaConfigured(status.configured);
      } catch (e) {
        if (!isActive()) return;
        setCpaConfigured(false);
        toast("error", `CPA connection status failed: ${e}`);
      }
    },
    [toast],
  );

  useEffect(() => {
    let active = true;
    void refreshCpaConnectionStatus(() => active);
    return () => {
      active = false;
    };
  }, [refreshCpaConnectionStatus]);

  useEffect(() => {
    let active = true;
    let unlisten: (() => void) | undefined;

    void listen<UsageData>("usage-updated", (event) => {
      if (!active) return;
      if (usageRequestRef.current) {
        appliedUsageRequestRef.current = usageRequestRef.current;
      }
      setUsageData(event.payload);
      setLastSyncAt(Date.now());
      void refreshCpaConnectionStatus(() => active);
    })
      .then((stop) => {
        if (active) unlisten = stop;
        else stop();
      })
      .catch((e) => {
        if (active) toast("error", `Usage refresh listener failed: ${e}`);
      });

    return () => {
      active = false;
      unlisten?.();
    };
  }, [refreshCpaConnectionStatus, toast]);

  // Usage polling. Cadence is unchanged from the split-pane UI — the widget
  // changed the presentation of freshness, not how often Quill reads.
  useEffect(() => {
    let active = true;
    if (usageSourcesLoading) {
      return () => {
        active = false;
      };
    }
    if (!hasUsageSource) {
      setUsageData(null);
      setLastSyncAt(null);
      return () => {
        active = false;
      };
    }
    void refresh(() => active);
    const interval = setInterval(() => void refresh(() => active), USAGE_REFRESH_MS);
    return () => {
      active = false;
      clearInterval(interval);
    };
  }, [hasUsageSource, liveProviderKey, refresh, usageSourcesLoading]);

  const checkForUpdate = useCallback(() => {
    check()
      .then((update) => {
        if (update) {
          console.log(`Update available: ${update.version}`);
          setPendingUpdate(update);
        }
      })
      .catch((e) => console.log("Update check skipped:", e));
  }, []);

  useEffect(() => {
    if (import.meta.env.DEV) return;
    checkForUpdate();
    const interval = setInterval(checkForUpdate, UPDATE_CHECK_MS);
    return () => clearInterval(interval);
  }, [checkForUpdate]);

  const handleUpdate = useCallback(async () => {
    if (!pendingUpdate || updating) return;
    setUpdating(true);
    try {
      await invoke("install_app_update");
    } catch (e) {
      toast("error", `Update failed: ${e}`);
      setUpdating(false);
    }
  }, [pendingUpdate, updating, toast]);

  // Close-to-tray: the widget is a background instrument, so both the titlebar
  // control and the window manager's close request hide it, never exit.
  const handleClose = useCallback(async () => {
    await invoke("hide_window");
  }, []);

  useEffect(() => {
    const unlistenPromise = getCurrentWindow().onCloseRequested(async (event) => {
      event.preventDefault();
      await invoke("hide_window");
    });
    return () => {
      unlistenPromise.then((fn) => fn());
    };
  }, []);

  const handleContextMenu = (e: React.MouseEvent) => {
    e.preventDefault();
    const menuWidth = 100;
    const menuHeight = 70;
    const x = Math.min(e.clientX, window.innerWidth - menuWidth);
    const y = Math.min(e.clientY, window.innerHeight - menuHeight);
    setMenuPos({ x, y });
    setShowMenu(true);
  };

  const closeMenu = () => setShowMenu(false);

  const handleQuit = async () => {
    closeMenu();
    await invoke("quit_app");
  };

  const handleRefresh = async () => {
    closeMenu();
    await refreshIntegrations();
  };

  const emptyState = (() => {
    if (providersError) {
      return {
        title: "Provider status unavailable",
        description:
          "Quill could not load integration status. Restart the app, then enable Claude Code or Codex from Manage.",
      };
    }
    if (hasDetectedProvider) {
      return {
        title: "No provider is enabled",
        description:
          "Enable Claude Code or Codex from Manage to restore Quill features.",
      };
    }
    return {
      title: "Install Claude Code or Codex",
      description:
        "Quill needs at least one supported provider installed and enabled before its features can run.",
    };
  })();

  return (
    <div className="wg-shell" onContextMenu={handleContextMenu} onClick={closeMenu}>
      <WidgetTitleBar
        providerErrors={titlebarProviderErrors}
        hasUsageSource={hasUsageSource}
        lastSyncAt={lastSyncAt}
        pendingUpdate={pendingUpdate}
        updating={updating}
        onUpdate={() => void handleUpdate()}
        onClose={() => void handleClose()}
      />
      <div className="wg-rule" />
      <div className="wg-scroll">
        <div className="wg-content">
          {/* Content column: the LIMITS section, then the switchable view
              region. The shell owns only the provider-level states that gate
              them. */}
          {usageSourcesLoading ? (
            <div className="wg-state">
              <span className="wg-state-lamp" aria-hidden="true" />
              Checking integrations…
            </div>
          ) : !hasUsageSource ? (
            <div className="wg-empty">
              <p className="wg-empty-title">{emptyState.title}</p>
              <p className="wg-empty-body">{emptyState.description}</p>
              <button
                type="button"
                className="wg-key wg-key-wide"
                onClick={() => void refreshIntegrations()}
              >
                Rescan providers
              </button>
            </div>
          ) : (
            <>
              <LimitsSection data={usageData} statuses={statuses} />
              <div className="wg-rule" />
              <ViewRegion />
            </>
          )}
        </div>
      </div>
      {showMenu && (
        <div
          className="context-menu"
          style={{ left: menuPos.x, top: menuPos.y }}
          onClick={(e) => e.stopPropagation()}
        >
          <button className="context-menu-item" onClick={() => void handleRefresh()}>
            Refresh
          </button>
          <button className="context-menu-item" onClick={() => void handleQuit()}>
            Quit
          </button>
        </div>
      )}
    </div>
  );
}

export default App;
