// App — the 360px widget shell.
//
// The main window *is* the widget now: a fixed-width, always-on-top instrument
// made of a titlebar and a single content column. The split-pane layout, the
// draggable divider, the `--s` fit-to-height scaling system, arrow-key resize
// and the `quill-size-*` / `quill-split-ratio*` / `quill-layout-mode`
// preferences that fed them are gone. What survives here is the app-lifecycle
// work that has no other home: usage polling, the four-hour update check, the
// right-click Refresh/Quit menu, close-to-tray, and driving the window's
// content-derived height.
//
// See specs/018-widget-ui-redesign/plan.md#Affected Components.

import { useState, useEffect, useCallback, useRef } from "react";
import { invoke } from "@tauri-apps/api/core";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { LogicalSize } from "@tauri-apps/api/dpi";
import { check } from "@tauri-apps/plugin-updater";
import LimitsSection from "./components/widget/LimitsSection";
import WidgetTitleBar, {
  type WidgetSyncState,
} from "./components/widget/WidgetTitleBar";
import type { UseIntegrationsResult } from "./hooks/useIntegrations";
import { useToast } from "./hooks/useToast";
import type { UsageData, PendingUpdate } from "./types";

/** Fixed widget width. Mirrors `width` in src-tauri/tauri.conf.json. */
const WIDGET_WIDTH = 360;
/** Height bounds the content-derived resize is clamped into. */
const MIN_WIDGET_HEIGHT = 200;
const MAX_WIDGET_HEIGHT = 900;
/** Titlebar (40) + the hairline under it (1) + the shell's 1px border, top and bottom. */
const CHROME_HEIGHT = 43;
const USAGE_REFRESH_MS = 3 * 60_000;
const UPDATE_CHECK_MS = 4 * 60 * 60_000;

/**
 * Collapse the poller's per-provider error kinds into the single freshness
 * state the titlebar pill shows. Offline wins over cached because both say
 * "showing cached data" and the widget must not stack two near-identical
 * statements; this mirrors the precedence the old usage pills used.
 */
function syncStateFor(
  usageData: UsageData | null,
  hasEnabledProvider: boolean,
): WidgetSyncState {
  if (!hasEnabledProvider || !usageData) return "idle";
  const kinds = usageData.provider_errors.map((error) => error.kind);
  if (kinds.includes("network")) return "offline";
  if (kinds.includes("stale")) return "cached";
  if (kinds.includes("paused")) return "paused";
  return "live";
}

interface AppProps {
  integrations: UseIntegrationsResult;
}

function App({ integrations }: AppProps) {
  const { toast } = useToast();
  const [usageData, setUsageData] = useState<UsageData | null>(null);
  const [lastSyncAt, setLastSyncAt] = useState<number | null>(null);
  const [showMenu, setShowMenu] = useState(false);
  const [menuPos, setMenuPos] = useState({ x: 0, y: 0 });
  const [pendingUpdate, setPendingUpdate] = useState<PendingUpdate | null>(null);
  const [updating, setUpdating] = useState(false);
  const contentRef = useRef<HTMLDivElement>(null);
  const appliedHeightRef = useRef(0);

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

  const refresh = useCallback(async () => {
    try {
      const data = await invoke<UsageData>("fetch_usage_data");
      setUsageData(data);
      setLastSyncAt(Date.now());
    } catch (e) {
      toast("error", `Usage data fetch failed: ${e}`);
      setUsageData({
        buckets: [],
        provider_errors: [],
        provider_credits: [],
        error: String(e),
      });
    }
  }, [toast]);

  // Usage polling. Cadence is unchanged from the split-pane UI — the widget
  // changed the presentation of freshness, not how often Quill reads.
  useEffect(() => {
    if (providersLoading) return;
    if (!hasEnabledProvider) {
      setUsageData(null);
      setLastSyncAt(null);
      return;
    }
    void refresh();
    const interval = setInterval(() => void refresh(), USAGE_REFRESH_MS);
    return () => clearInterval(interval);
  }, [hasEnabledProvider, liveProviderKey, providersLoading, refresh]);

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

  // Content-derived height. Width is fixed by the window config, so the shell
  // only ever asks for the height its bands occupy, clamped to the configured
  // bounds. The measured element lives inside the scroll container and is
  // never itself constrained by the viewport, so this cannot oscillate.
  useEffect(() => {
    const element = contentRef.current;
    if (!element) return;

    let frame = 0;
    const apply = () => {
      cancelAnimationFrame(frame);
      frame = requestAnimationFrame(() => {
        const measured = Math.ceil(element.getBoundingClientRect().height);
        if (measured <= 0) return;
        const next = Math.min(
          MAX_WIDGET_HEIGHT,
          Math.max(MIN_WIDGET_HEIGHT, measured + CHROME_HEIGHT),
        );
        if (next === appliedHeightRef.current) return;
        appliedHeightRef.current = next;
        getCurrentWindow()
          .setSize(new LogicalSize(WIDGET_WIDTH, next))
          .catch(() => {
            // A platform that refuses a programmatic resize keeps the
            // configured height; the shell scrolls rather than clipping.
          });
      });
    };

    const observer = new ResizeObserver(apply);
    observer.observe(element);
    apply();
    return () => {
      observer.disconnect();
      cancelAnimationFrame(frame);
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
        syncState={syncStateFor(usageData, hasEnabledProvider)}
        lastSyncAt={lastSyncAt}
        pendingUpdate={pendingUpdate}
        updating={updating}
        onUpdate={() => void handleUpdate()}
        onClose={() => void handleClose()}
      />
      <div className="wg-rule" />
      <div className="wg-scroll">
        <div className="wg-content" ref={contentRef}>
          {/* Content column: the LIMITS section, then the switchable view
              region. The shell owns only the provider-level states that gate
              them. */}
          {providersLoading ? (
            <div className="wg-state">
              <span className="wg-state-lamp" aria-hidden="true" />
              Checking integrations…
            </div>
          ) : !hasEnabledProvider ? (
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
            <LimitsSection data={usageData} statuses={statuses} />
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
