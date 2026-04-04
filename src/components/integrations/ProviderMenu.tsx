import type { IntegrationProvider, ProviderStatus } from "../../types";

interface ProviderMenuProps {
  className?: string;
  statuses: ProviderStatus[];
  loading: boolean;
  error: string | null;
  inFlightProviders: ReadonlySet<IntegrationProvider>;
  onRequestToggle: (
    provider: IntegrationProvider,
    nextEnabled: boolean,
  ) => void;
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

function providerSummary(status: ProviderStatus): string {
  if (status.lastError) {
    return status.lastError;
  }
  if (status.provider === "mini_max") {
    return status.enabled
      ? "Subscription usage tracking is active."
      : "Enable to track your MiniMax subscription usage.";
  }
  if (status.enabled) {
    return "Quill integration is installed and active.";
  }
  if (!status.detectedCli && status.detectedHome) {
    return "Provider files exist locally, but the CLI is not available on PATH.";
  }
  if (!status.detectedCli) {
    return "Install the CLI first, then enable Quill integration here.";
  }
  if (!status.detectedHome) {
    return "CLI detected. Quill will install its managed files when enabled.";
  }
  return "Detected and ready. Enable to install Quill hooks, commands, and instructions.";
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
  onRequestToggle,
}: ProviderMenuProps) {
  return (
    <div
      className={className ? `provider-menu ${className}` : "provider-menu"}
      role="menu"
      aria-label="Provider settings"
    >
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
                <p className="provider-menu-description">
                  {providerSummary(status)}
                </p>
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
