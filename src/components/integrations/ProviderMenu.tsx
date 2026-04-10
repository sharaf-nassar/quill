import type {
  IndicatorPrimaryProvider,
  IntegrationProvider,
  LayoutMode,
  ProviderStatus,
} from "../../types";

interface ProviderMenuProps {
  className?: string;
  statuses: ProviderStatus[];
  loading: boolean;
  error: string | null;
  inFlightProviders: ReadonlySet<IntegrationProvider>;
  indicatorPrimaryProvider: IndicatorPrimaryProvider;
  onRequestToggle: (
    provider: IntegrationProvider,
    nextEnabled: boolean,
  ) => void;
  onIndicatorPrimaryProviderChange: (provider: IndicatorPrimaryProvider) => void;
  layoutMode?: LayoutMode;
  onLayoutModeChange?: (mode: LayoutMode) => void;
}

function providerLabel(provider: IntegrationProvider): string {
  if (provider === "claude") return "Claude Code";
  if (provider === "codex") return "Codex";
  return "MiniMax";
}

function providerBadge(status: ProviderStatus): string {
  if (status.enabled) {
    return "Enabled";
  }
  if (!status.detectedCli) {
    return "Not installed";
  }
  if (status.setupState === "missing") {
    return "Needs setup";
  }
  return "Available";
}

function actionLabel(status: ProviderStatus): string {
  if (status.enabled) {
    return "Disable";
  }
  if (!status.detectedCli) {
    return "Missing CLI";
  }
  return "Enable";
}

function ProviderMenu({
  className,
  statuses,
  loading,
  error,
  inFlightProviders,
  indicatorPrimaryProvider,
  onRequestToggle,
  onIndicatorPrimaryProviderChange,
  layoutMode,
  onLayoutModeChange,
}: ProviderMenuProps) {
  const enabledProviders = statuses
    .filter((status) => status.enabled)
    .map((status) => status.provider);
  const unavailablePreferredProvider =
    indicatorPrimaryProvider != null &&
    !enabledProviders.includes(indicatorPrimaryProvider);

  return (
    <div
      className={className ? `provider-menu ${className}` : "provider-menu"}
      role="menu"
      aria-label="Provider settings"
    >
      {layoutMode != null && onLayoutModeChange != null && (
        <>
          <div className="provider-menu-header">Layout</div>
          <div className="provider-menu-layout-row">
            <button
              className={`layout-toggle-btn${layoutMode === "stacked" ? " layout-toggle-btn--active" : ""}`}
              onClick={() => onLayoutModeChange("stacked")}
              aria-pressed={layoutMode === "stacked"}
              aria-label="Stacked layout"
              title="Stacked"
            >
              <svg width="16" height="16" viewBox="0 0 16 16" fill="none" aria-hidden="true">
                <rect x="2" y="2" width="12" height="5" rx="1.5" fill="currentColor" />
                <rect x="2" y="9" width="12" height="5" rx="1.5" fill="currentColor" />
              </svg>
            </button>
            <button
              className={`layout-toggle-btn${layoutMode === "side-by-side" ? " layout-toggle-btn--active" : ""}`}
              onClick={() => onLayoutModeChange("side-by-side")}
              aria-pressed={layoutMode === "side-by-side"}
              aria-label="Side by side layout"
              title="Side by side"
            >
              <svg width="16" height="16" viewBox="0 0 16 16" fill="none" aria-hidden="true">
                <rect x="1" y="2" width="6" height="12" rx="1.5" fill="currentColor" />
                <rect x="9" y="2" width="6" height="12" rx="1.5" fill="currentColor" />
              </svg>
            </button>
          </div>
          <div className="provider-menu-section-divider" />
        </>
      )}
      <div className="provider-menu-header">Status</div>
      <div className="provider-menu-row">
        <div className="provider-menu-copy">
          <div className="provider-menu-title-row">
            <label className="provider-menu-title" htmlFor="indicator-primary-provider">
              Status provider
            </label>
          </div>
        </div>
        <select
          id="indicator-primary-provider"
          className="provider-menu-action"
          value={indicatorPrimaryProvider ?? ""}
          disabled={loading}
          onChange={(event) =>
            onIndicatorPrimaryProviderChange(
              (event.target.value || null) as IndicatorPrimaryProvider,
            )
          }
          aria-label="Status provider"
        >
          <option value="">Automatic</option>
          {unavailablePreferredProvider ? (
            <option value={indicatorPrimaryProvider ?? ""} disabled>
              {providerLabel(indicatorPrimaryProvider)} (unavailable)
            </option>
          ) : null}
          {enabledProviders.map((provider) => (
            <option key={provider} value={provider}>
              {providerLabel(provider)}
            </option>
          ))}
        </select>
      </div>
      <div className="provider-menu-section-divider" />
      <div className="provider-menu-header">Integrations</div>
      {loading ? (
        <div className="provider-menu-empty">Checking provider status...</div>
      ) : error ? (
        <div className="provider-menu-empty provider-menu-empty--error">{error}</div>
      ) : (
        statuses.map((status) => {
          const busy = inFlightProviders.has(status.provider);
          const canEnable = status.detectedCli;
          const actionDisabled = busy || (!status.enabled && !canEnable);

          return (
            <div key={status.provider} className="provider-menu-row">
              <div className="provider-menu-copy">
                <div className="provider-menu-title-row">
                  <span className="provider-menu-title">
                    {providerLabel(status.provider)}
                  </span>
                  <span
                    className={`provider-menu-badge${status.enabled ? " provider-menu-badge--enabled" : ""}`}
                  >
                    {providerBadge(status)}
                  </span>
                </div>
              </div>
              <button
                className={`provider-menu-action${status.enabled ? " provider-menu-action--destructive" : ""}`}
                disabled={actionDisabled}
                onClick={() => onRequestToggle(status.provider, !status.enabled)}
              >
                {busy ? "Working..." : actionLabel(status)}
              </button>
            </div>
          );
        })
      )}
    </div>
  );
}

export default ProviderMenu;
