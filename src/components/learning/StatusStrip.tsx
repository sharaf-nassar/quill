import type { LearningRun, ProviderFilter, ToolCount } from "../../types";
import {
  providerFilterLabel,
  providerLabel,
  PROVIDER_ASYMMETRY_DISCLOSURE,
} from "../../utils/providers";
import { timeAgo } from "../../utils/time";

/**
 * Per-provider contribution to shared-scope rules (feature 005 M-6 / FR-028).
 * Derived in `LearningWindow` from the already-fetched rules' `provider_scope`
 * — NOT a new fetch. Quantifies the asymmetry disclosure so it is concrete on
 * the combined-scope surface instead of generic.
 */
export interface SharedScopeContribution {
  claude: number;
  codex: number;
  sharedRuleCount: number;
}

interface StatusStripProps {
  providerFilter: ProviderFilter;
  observationCount: number;
  unanalyzedCount: number;
  topTools: ToolCount[];
  sparkline: number[];
  lastRun: LearningRun | undefined;
  analyzing: boolean;
  onAnalyze: () => void;
  /**
   * Shared-rule per-provider contribution. Only meaningful (and only
   * rendered) on the combined "All Providers" scope; omitted/zeroed on a
   * single-provider filter.
   */
  sharedScope?: SharedScopeContribution;
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
  sharedScope,
}: StatusStripProps) {
  const maxSparkline = Math.max(...sparkline, 1);
  // M-6: show the quantified asymmetry disclosure ONLY on the combined scope
  // and only when there is at least one shared rule to characterize.
  const showAsymmetryNote =
    providerFilter === "all" &&
    !!sharedScope &&
    sharedScope.sharedRuleCount > 0;

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
      {showAsymmetryNote && sharedScope && (
        <div
          className="learning-asymmetry-note learning-status-row"
          role="note"
        >
          <span className="learning-asymmetry-icon">\u24D8</span>
          <span>
            {PROVIDER_ASYMMETRY_DISCLOSURE}{" "}
            <span className="learning-asymmetry-quant">
              ({sharedScope.sharedRuleCount} shared rule
              {sharedScope.sharedRuleCount !== 1 ? "s" : ""}:{" "}
              {providerLabel("claude")} contributes to {sharedScope.claude},{" "}
              {providerLabel("codex")} to {sharedScope.codex})
            </span>
          </span>
        </div>
      )}
    </div>
  );
}

export default StatusStrip;
