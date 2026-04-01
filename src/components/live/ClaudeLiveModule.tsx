import UsageRow from "../UsageRow";
import type { TimeMode, UsageBucket } from "../../types";
import { providerLabel } from "../../utils/providers";

type ClaudeUsageBucket = UsageBucket & { provider: "claude" };

interface ClaudeProviderSection {
  provider: "claude";
  buckets: ClaudeUsageBucket[];
}

interface ClaudeLiveModuleProps {
  section: ClaudeProviderSection;
  timeMode: TimeMode;
  showHeading?: boolean;
}

function ClaudeLiveModule({
  section,
  timeMode,
  showHeading = true,
}: ClaudeLiveModuleProps) {
  return (
    <div className="usage-provider-group">
      {showHeading && (
        <div className="usage-provider-group__header">
          <span className="usage-provider-badge usage-provider-badge--claude">
            {providerLabel("claude")}
          </span>
          <span className="usage-provider-group__meta">API limits</span>
        </div>
      )}
      {section.buckets.map((bucket) => (
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

export default ClaudeLiveModule;
