import UsageRow, { formatCountdown, gradientColor } from "../UsageRow";
import type {
  IntegrationProvider,
  ProviderCredits,
  TimeMode,
  UsageBucket,
} from "../../types";
import { providerLabel } from "../../utils/providers";

const PRIMARY_MINIMAX_MODELS = ["M*", "coding-plan-search", "coding-plan-vlm"];

function isPrimaryMinimaxBucket(bucket: UsageBucket): boolean {
  return PRIMARY_MINIMAX_MODELS.some(
    (model) => bucket.label.startsWith(model + " ") || bucket.label === model,
  );
}

interface ProviderUsageModuleProps {
  provider: IntegrationProvider;
  buckets: UsageBucket[];
  timeMode: TimeMode;
  showHeading?: boolean;
  credits?: ProviderCredits;
}

function providerMeta(provider: IntegrationProvider): string {
  if (provider === "claude") return "API limits";
  if (provider === "mini_max") return "Subscription";
  return "Usage limits";
}

function ProviderUsageModule({
  provider,
  buckets,
  timeMode,
  showHeading = true,
  credits,
}: ProviderUsageModuleProps) {
  const isMinimax = provider === "mini_max";
  const primaryBuckets = isMinimax
    ? buckets.filter(isPrimaryMinimaxBucket)
    : buckets;
  const secondaryBuckets = isMinimax
    ? buckets.filter((b) => !isPrimaryMinimaxBucket(b))
    : [];

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
          {secondaryBuckets.length > 0 && (
            <span className="minimax-all-models">
              <span className="minimax-all-models__badge">All models</span>
              <div className="minimax-all-models__tooltip">
                {secondaryBuckets.map((bucket) => (
                  <div
                    key={bucket.key}
                    className="minimax-all-models__row"
                  >
                    <span className="minimax-all-models__label">
                      {bucket.label}
                    </span>
                    <span
                      className="minimax-all-models__pct"
                      style={{ color: gradientColor(bucket.utilization) }}
                    >
                      {Math.round(bucket.utilization)}%
                    </span>
                    <span className="minimax-all-models__reset">
                      {formatCountdown(bucket.resets_at)}
                    </span>
                  </div>
                ))}
              </div>
            </span>
          )}
          {credits?.balance != null && (
            <span className="usage-provider-credits">
              {credits.balance} credits
            </span>
          )}
        </div>
      )}
      {primaryBuckets.map((bucket) => (
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
