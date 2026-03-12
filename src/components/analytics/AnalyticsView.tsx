import { useState, useRef, useEffect, useMemo } from "react";
import { useAnalyticsData } from "../../hooks/useAnalyticsData";
import { useTokenData } from "../../hooks/useTokenData";
import UsageChart from "./UsageChart";
import BreakdownPanel from "./BreakdownPanel";
import { getColor, TrendArrow } from "./shared";
import { formatTokenCount } from "../../utils/tokens";
import type { RangeType, UsageBucket, BreakdownSelection, TokenStats } from "../../types";

function cacheColor(stats: TokenStats): string {
  const rate = stats.total_cache_read / (stats.total_input + stats.total_cache_read);
  if (rate >= 0.6) return "#22C55E";
  if (rate >= 0.3) return "#EAB308";
  return "#EF4444";
}

interface BucketDropdownProps {
  value: string;
  options: string[];
  onChange: (value: string) => void;
}

function BucketDropdown({ value, options, onChange }: BucketDropdownProps) {
  const [open, setOpen] = useState(false);
  const [focusIdx, setFocusIdx] = useState(-1);
  const ref = useRef<HTMLDivElement>(null);
  const itemRefs = useRef<(HTMLButtonElement | null)[]>([]);

  useEffect(() => {
    if (!open) return;
    const handler = (e: MouseEvent) => {
      if (ref.current && !ref.current.contains(e.target as Node))
        setOpen(false);
    };
    document.addEventListener("mousedown", handler);
    return () => document.removeEventListener("mousedown", handler);
  }, [open]);

  useEffect(() => {
    if (open && focusIdx >= 0 && itemRefs.current[focusIdx]) {
      itemRefs.current[focusIdx]!.focus();
    }
  }, [open, focusIdx]);

  const handleTriggerKeyDown = (e: React.KeyboardEvent) => {
    if (e.key === "ArrowDown" || e.key === "Enter" || e.key === " ") {
      e.preventDefault();
      setOpen(true);
      setFocusIdx(Math.max(0, options.indexOf(value)));
    }
  };

  const handleMenuKeyDown = (e: React.KeyboardEvent) => {
    if (e.key === "Escape") {
      setOpen(false);
      setFocusIdx(-1);
      e.stopPropagation();
    } else if (e.key === "ArrowDown") {
      e.preventDefault();
      setFocusIdx((i) => Math.min(i + 1, options.length - 1));
    } else if (e.key === "ArrowUp") {
      e.preventDefault();
      setFocusIdx((i) => Math.max(i - 1, 0));
    } else if (e.key === "Enter" || e.key === " ") {
      e.preventDefault();
      if (focusIdx >= 0 && focusIdx < options.length) {
        onChange(options[focusIdx]);
        setOpen(false);
        setFocusIdx(-1);
      }
    }
  };

  return (
    <div className="bucket-dropdown-wrap" ref={ref}>
      <button
        className="bucket-dropdown-trigger"
        onClick={() => setOpen((v) => !v)}
        onKeyDown={handleTriggerKeyDown}
        aria-haspopup="listbox"
        aria-expanded={open}
        aria-label={`Select bucket: ${value}`}
      >
        {value}
        <span className="bucket-dropdown-arrow">&#9662;</span>
      </button>
      {open && (
        <div
          className="bucket-dropdown-menu"
          role="listbox"
          aria-label="Usage buckets"
          onKeyDown={handleMenuKeyDown}
        >
          {options.map((opt, i) => (
            <button
              key={opt}
              ref={(el) => {
                itemRefs.current[i] = el;
              }}
              className={`bucket-dropdown-item${opt === value ? " active" : ""}`}
              role="option"
              aria-selected={opt === value}
              tabIndex={focusIdx === i ? 0 : -1}
              onClick={() => {
                onChange(opt);
                setOpen(false);
                setFocusIdx(-1);
              }}
            >
              {opt}
            </button>
          ))}
        </div>
      )}
    </div>
  );
}

const RANGES: RangeType[] = ["1h", "24h", "7d", "30d"];
const RANGE_LABELS: Record<RangeType, string> = {
  "1h": "1H",
  "24h": "24H",
  "7d": "7D",
  "30d": "30D",
};
const RANGE_DAYS: Record<RangeType, number> = {
  "1h": 1,
  "24h": 1,
  "7d": 7,
  "30d": 30,
};
const DAYS_TO_RANGE: Record<number, RangeType> = {
  1: "24h",
  7: "7d",
  30: "30d",
};

interface AnalyticsViewProps {
  currentBuckets: UsageBucket[];
}

const BREAKDOWN_COLLAPSED_KEY = "quill-breakdown-collapsed";

function AnalyticsView({ currentBuckets }: AnalyticsViewProps) {
  const [range, setRange] = useState<RangeType>("24h");
  const [selectedBucket, setSelectedBucket] = useState(
    () => currentBuckets?.[0]?.label ?? "7 days",
  );
  const [breakdownSelection, setBreakdownSelection] =
    useState<BreakdownSelection | null>(null);
  const [breakdownCollapsed, setBreakdownCollapsed] = useState(() => {
    try {
      return localStorage.getItem(BREAKDOWN_COLLAPSED_KEY) === "true";
    } catch {
      return false;
    }
  });

  const breakdownDays = RANGE_DAYS[range] ?? 1;
  const hasSelection = breakdownSelection !== null;
  // When a breakdown entry is selected, use the breakdown's full time scope
  // so older entries always have visible data in the chart
  const tokenRange: RangeType = hasSelection
    ? (DAYS_TO_RANGE[breakdownDays] ?? "24h")
    : range;

  const bucketsKey = (currentBuckets ?? [])
    .map((b) => `${b.label}:${b.utilization}:${b.resets_at ?? ""}`)
    .join(",");

  // eslint-disable-next-line react-hooks/exhaustive-deps -- bucketsKey is an intentional stabilizer for currentBuckets
  const stableBuckets = useMemo(() => currentBuckets, [bucketsKey]);

  const { history, stats, snapshotCount, loading, error } = useAnalyticsData(
    selectedBucket,
    range,
    stableBuckets,
  );

  const tokenHostname =
    breakdownSelection?.type === "host" ? breakdownSelection.key : null;
  const tokenSessionId =
    breakdownSelection?.type === "session" ? breakdownSelection.key : null;
  const tokenCwd =
    breakdownSelection?.type === "project" ? breakdownSelection.key : null;
  const { history: tokenHistory, stats: tokenStats } = useTokenData(
    tokenRange,
    tokenHostname,
    tokenSessionId,
    tokenCwd,
  );

  if (snapshotCount === 0 && !loading) {
    return (
      <div className="analytics-view">
        <div className="analytics-empty-state">
          <svg
            className="analytics-empty-icon"
            width="32"
            height="32"
            viewBox="0 0 24 24"
            fill="none"
            stroke="currentColor"
            strokeWidth="1.5"
            strokeLinecap="round"
            strokeLinejoin="round"
            aria-hidden="true"
          >
            <circle cx="12" cy="12" r="10" />
            <polyline points="12 6 12 12 16 14" />
          </svg>
          <div className="analytics-empty-title">
            {"Collecting usage data\u2026"}
          </div>
          <div className="analytics-empty-desc">
            Analytics will appear here once enough data has been recorded. Data
            is captured every 60 seconds.
          </div>
        </div>
      </div>
    );
  }

  return (
    <div className="analytics-view">
      <div className="analytics-controls">
        <div className={`range-tabs${hasSelection ? " dimmed" : ""}`}>
          {RANGES.map((r) => (
            <button
              key={r}
              className={`range-tab${range === r ? " active" : ""}`}
              aria-pressed={range === r}
              onClick={() => setRange(r)}
            >
              {RANGE_LABELS[r]}
            </button>
          ))}
        </div>
        {stats && (
          <div className="inline-stats">
            <span className="inline-stat">
              <span className="inline-stat-label">Avg</span>
              <span
                className="inline-stat-value"
                style={{ color: getColor(stats.avg) }}
              >
                {stats.avg.toFixed(1)}%
              </span>
            </span>
            <span className="inline-stat">
              <span className="inline-stat-label">Peak</span>
              <span
                className="inline-stat-value"
                style={{ color: getColor(stats.max) }}
              >
                {stats.max.toFixed(1)}%
              </span>
            </span>
            <span className="inline-stat">
              <TrendArrow trend={stats.trend} />
            </span>
          </div>
        )}
        {tokenStats && tokenStats.total_tokens > 0 && (
          <div className="inline-stats token-stats">
            <span className="inline-stat">
              <span className="inline-stat-label">In</span>
              <span className="inline-stat-value">{formatTokenCount(tokenStats.total_input)}</span>
            </span>
            <span className="inline-stat">
              <span className="inline-stat-label">Out</span>
              <span className="inline-stat-value">{formatTokenCount(tokenStats.total_output)}</span>
            </span>
            {(tokenStats.total_input + tokenStats.total_cache_read) > 0 && (
              <span className="inline-stat">
                <span className="inline-stat-label">Cache</span>
                <span className="inline-stat-value" style={{ color: cacheColor(tokenStats) }}>
                  {Math.round(tokenStats.total_cache_read / (tokenStats.total_input + tokenStats.total_cache_read) * 100)}%
                </span>
              </span>
            )}
          </div>
        )}
        <BucketDropdown
          value={selectedBucket}
          options={(currentBuckets ?? []).map((b) => b.label)}
          onChange={setSelectedBucket}
        />
      </div>

      {error && (
        <div className="analytics-error" role="alert">
          Failed to load analytics
        </div>
      )}

      {loading ? (
        <>
          <div className="chart-skeleton" />
          <div className="breakdown-skeleton">
            <div className="breakdown-skeleton-row" />
            <div className="breakdown-skeleton-row" />
            <div className="breakdown-skeleton-row" />
          </div>
        </>
      ) : (
        <>
          <div className="chart-section">
            <div className="section-title">
              {selectedBucket} Usage
              {hasSelection && breakdownSelection && (
                <span className="filter-badge">
                  {breakdownSelection.type === "host"
                    ? breakdownSelection.key
                    : breakdownSelection.type === "project"
                      ? breakdownSelection.key.split("/").filter(Boolean).pop()
                      : breakdownSelection.key.slice(0, 8)}
                  <button
                    className="filter-badge-clear"
                    onClick={() => setBreakdownSelection(null)}
                    aria-label="Clear filter"
                  >
                    &#10005;
                  </button>
                </span>
              )}
            </div>
            <UsageChart
              data={history}
              range={range}
              bucket={selectedBucket}
              tokenData={tokenHistory}
            />
          </div>
          <div className="breakdown-collapsible">
            <button
              className="breakdown-collapse-toggle"
              onClick={() => {
                const next = !breakdownCollapsed;
                setBreakdownCollapsed(next);
                try {
                  localStorage.setItem(BREAKDOWN_COLLAPSED_KEY, String(next));
                } catch { /* ignore */ }
              }}
              aria-expanded={!breakdownCollapsed}
              aria-label={breakdownCollapsed ? "Show breakdown" : "Hide breakdown"}
            >
              <span className="breakdown-collapse-chevron">
                {breakdownCollapsed ? "\u25B8" : "\u25BE"}
              </span>
              <span className="section-title" style={{ marginBottom: 0 }}>Breakdown</span>
            </button>
            {!breakdownCollapsed && (
              <BreakdownPanel
                days={RANGE_DAYS[range] ?? 1}
                selection={breakdownSelection}
                onSelect={setBreakdownSelection}
              />
            )}
          </div>
        </>
      )}
    </div>
  );
}

export default AnalyticsView;
