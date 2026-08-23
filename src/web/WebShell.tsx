import { useCallback } from "react";
import LimitsSection from "../components/widget/LimitsSection";
import ViewRegion from "../components/widget/ViewRegion";
import { useWebMonitorData } from "./useWebMonitorData";

function WebShell() {
  const monitor = useWebMonitorData();
  const noRefresh = useCallback(async () => undefined, []);

  return (
    <main className="wg-shell" aria-label="Quill web monitor">
      <header className="wg-web-header">
        <div className="wg-tb-brand">
          <span className="wg-glyph" aria-hidden="true" />
          <span className="wg-wordmark">Quill</span>
        </div>
      </header>
      <div className="wg-rule" />
      <div className="wg-scroll">
        <div className="wg-content">
          {monitor.loading ? (
            <div className="wg-state" role="status">
              <span className="wg-state-lamp" aria-hidden="true" />
              Checking provider status…
            </div>
          ) : !monitor.hasUsageSource && monitor.sourcesUnavailable ? (
            <div className="wg-empty" role="status">
              <p className="wg-empty-title">Provider status unavailable</p>
              <p className="wg-empty-body">
                Quill could not confirm whether a provider is enabled on desktop.
              </p>
            </div>
          ) : !monitor.hasUsageSource ? (
            <div className="wg-empty">
              <p className="wg-empty-title">No provider enabled</p>
              <p className="wg-empty-body">
                Enable a provider on desktop to view this monitor.
              </p>
            </div>
          ) : (
            <>
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
            </>
          )}
        </div>
      </div>
    </main>
  );
}

export default WebShell;
