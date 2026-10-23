import { lazy, Suspense, useCallback, useEffect, useState } from "react";
import LimitsSection from "../components/widget/LimitsSection";
import ViewRegion from "../components/widget/ViewRegion";
import { useWebMonitorData } from "./useWebMonitorData";

// Same section the desktop Manage workspace mounts; its five reads are all in
// the web allowlist, and the shell's 55 s stale-refresh covers its freshness
// because the browser has no ingest events.
const ExploreSection = lazy(() => import("../windows/ExploreWindowView"));

/** Explore lives at `/#explore` so it survives reload and the back button. */
const EXPLORE_HASH = "#explore";
const readSurface = (): "monitor" | "explore" =>
  window.location.hash === EXPLORE_HASH ? "explore" : "monitor";

function WebShell() {
  const monitor = useWebMonitorData();
  const noRefresh = useCallback(async () => undefined, []);
  const [surface, setSurface] = useState(readSurface);

  useEffect(() => {
    const sync = () => setSurface(readSurface());
    window.addEventListener("popstate", sync);
    return () => window.removeEventListener("popstate", sync);
  }, []);

  const toggleSurface = () => {
    const next = surface === "explore" ? "monitor" : "explore";
    window.history.pushState(
      null,
      "",
      next === "explore" ? EXPLORE_HASH : window.location.pathname,
    );
    setSurface(next);
  };

  return (
    <main
      className="wg-shell wg-shell--web"
      data-surface={surface}
      aria-label="Quill web monitor"
    >
      <header className="wg-web-header">
        <div className="wg-tb-brand">
          <span className="wg-glyph" aria-hidden="true" />
          <span className="wg-wordmark">Quill</span>
        </div>
        {monitor.hasUsageSource && (
          <button
            type="button"
            className="wg-head-explore wg-web-surface-toggle"
            aria-pressed={surface === "explore"}
            title={
              surface === "explore"
                ? "Back to the live monitor"
                : "Open the Explore graph — every metric overlaid, over weeks or months"
            }
            onClick={toggleSurface}
          >
            {surface === "explore" ? (
              <>
                <span aria-hidden="true">←</span> Monitor
              </>
            ) : (
              <>
                <span aria-hidden="true">⤢</span> Explore
              </>
            )}
          </button>
        )}
      </header>
      <div className="wg-rule" />
      <div className="wg-scroll">
        {monitor.loading ? (
          <div className="wg-content">
            <div className="wg-state" role="status">
              <span className="wg-state-lamp" aria-hidden="true" />
              Checking provider status…
            </div>
          </div>
        ) : !monitor.hasUsageSource && monitor.sourcesUnavailable ? (
          <div className="wg-content">
            <div className="wg-empty" role="status">
              <p className="wg-empty-title">Provider status unavailable</p>
              <p className="wg-empty-body">
                Quill could not confirm whether a provider is enabled on desktop.
              </p>
            </div>
          </div>
        ) : !monitor.hasUsageSource ? (
          <div className="wg-content">
            <div className="wg-empty">
              <p className="wg-empty-title">No provider enabled</p>
              <p className="wg-empty-body">
                Enable a provider on desktop to view this monitor.
              </p>
            </div>
          </div>
        ) : surface === "explore" ? (
          <Suspense
            fallback={
              <div className="wg-content">
                <div className="wg-state" role="status">
                  <span className="wg-state-lamp" aria-hidden="true" />
                  Loading Explore…
                </div>
              </div>
            }
          >
            <ExploreSection />
          </Suspense>
        ) : (
          <div className="wg-content">
            {monitor.usageUnavailable && (
              <div className="wg-state" role="status">
                <span className="wg-state-lamp" aria-hidden="true" />
                Cached usage unavailable
              </div>
            )}
            <LimitsSection
              data={monitor.usageData}
              statuses={monitor.statuses}
              providerErrors={monitor.usageData?.provider_errors ?? null}
              hasUsageSource={monitor.hasUsageSource}
              lastSyncAt={null}
              onRefresh={noRefresh}
              webSurface
            />
            <div className="wg-rule" />
            <ViewRegion webSurface />
          </div>
        )}
      </div>
    </main>
  );
}

export default WebShell;
