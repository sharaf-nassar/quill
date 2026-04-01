import type { LearningRun, ProviderFilter, ToolCount } from "../../types";
import { providerFilterLabel } from "../../utils/providers";
import { timeAgo } from "../../utils/time";

interface StatusStripProps {
  providerFilter: ProviderFilter;
  observationCount: number;
  unanalyzedCount: number;
  topTools: ToolCount[];
  sparkline: number[];
  lastRun: LearningRun | undefined;
  analyzing: boolean;
  onAnalyze: () => void;
}

function StatusStrip({
  providerFilter,
  observationCount,
  unanalyzedCount,
  topTools,
  sparkline,
  lastRun,
  analyzing,
  onAnalyze,
}: StatusStripProps) {
  const maxSparkline = Math.max(...sparkline, 1);

  return (
    <div className="learning-status">
      <div className="learning-status-row">
        <span className="learning-context-chip">
          {providerFilterLabel(providerFilter)}
        </span>
        <span className="learning-stat">
          {observationCount} obs
        </span>
        <span className="learning-stat-sep">&middot;</span>
        <span className="learning-stat learning-stat--accent">
          {unanalyzedCount} new
        </span>
        {lastRun && (
          <>
            <span className="learning-stat-sep">&middot;</span>
            <span className="learning-stat">
              last {timeAgo(lastRun.created_at)}
            </span>
          </>
        )}
        {topTools.length > 0 && (
          <>
            <span className="learning-stat-sep">&middot;</span>
            {topTools.slice(0, 3).map((t, i) => (
              <span key={t.tool_name}>
                {i > 0 && <span className="learning-stat-sep">&middot;</span>}
                <span className="learning-tool-name">{t.tool_name}</span>{" "}
                <span className="learning-tool-count">{t.count}</span>
              </span>
            ))}
          </>
        )}
      </div>
      <div className="learning-status-row learning-sparkline-row">
        <div className="learning-sparkline">
          {sparkline.map((v, i) => (
            <div
              key={i}
              className="learning-sparkline-bar"
              style={{ height: `${Math.max(2, (v / maxSparkline) * 100)}%` }}
            />
          ))}
        </div>
        <span className="learning-sparkline-label">7d</span>
        <button
          className="learning-analyze-btn"
          onClick={onAnalyze}
          disabled={analyzing}
        >
          {analyzing ? "Analyzing\u2026" : "\u25B6 Analyze"}
        </button>
      </div>
    </div>
  );
}

export default StatusStrip;
