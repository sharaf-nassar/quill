import UsageRow from "../UsageRow";
import type {
  IntegrationProvider,
  ProviderCredits,
  TimeMode,
  UsageBucket,
} from "../../types";
import { providerLabel } from "../../utils/providers";

interface ProviderUsageModuleProps {
  provider: IntegrationProvider;
  buckets: UsageBucket[];
  timeMode: TimeMode;
  showHeading?: boolean;
  credits?: ProviderCredits;
}

function providerMeta(provider: IntegrationProvider): string {
  return provider === "claude" ? "API limits" : "Usage limits";
}

function ProviderUsageModule({
  provider,
  buckets,
  timeMode,
  showHeading = true,
  credits,
}: ProviderUsageModuleProps) {
  return (
    <div className="usage-provider-group">
      {(showHeading || credits?.balance != null) && (
        <div className="usage-provider-group__header">
          {showHeading && (
            <>
              <span className={`usage-provider-badge usage-provider-badge--${provider}`}>
                {providerLabel(provider)}
              </span>
              <span className="usage-provider-group__meta">{providerMeta(provider)}</span>
            </>
          )}
          {credits?.balance != null && (
            <span className="usage-provider-credits">
              {credits.balance} credits
            </span>
          )}
        </div>
      )}
      {buckets.map((bucket) => (
        <UsageRow
          key={`${bucket.provider}:${bucket.key}`}
          label={bucket.label}
          utilization={bucket.utilization}
          resetsAt={bucket.resets_at}
          timeMode={timeMode}
        />
      ))}
    </div>
  );
}

export default ProviderUsageModule;
