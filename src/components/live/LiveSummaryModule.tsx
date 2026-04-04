import { useEffect, useMemo, useRef, useState } from "react";
import { formatNumber } from "../../utils/format";
import { formatTokenCount } from "../../utils/tokens";
import {
  type LiveSummaryRange,
  useLiveSummaryData,
} from "../../hooks/useLiveSummaryData";
import type { IntegrationProvider, SparklinePoint } from "../../types";

const CHART_WIDTH = 100;
const CHART_HEIGHT = 40;
const CHART_PADDING = 3;
const COMPACT_HEIGHT_PX = 460;

const LIVE_RANGES: LiveSummaryRange[] = ["1h", "6h", "12h", "24h"];
const LIVE_RANGE_LABELS: Record<LiveSummaryRange, string> = {
  "1h": "1h",
  "6h": "6h",
  "12h": "12h",
  "24h": "24h",
};

function formatRelativeTime(isoDate: string | null): string {
  if (!isoDate) return "idle";
  const diffMs = Date.now() - new Date(isoDate).getTime();
  if (!Number.isFinite(diffMs) || diffMs < 0) return "now";
  const seconds = Math.floor(diffMs / 1000);
  if (seconds < 60) return `${seconds}s ago`;
  const minutes = Math.floor(seconds / 60);
  if (minutes < 60) return `${minutes}m ago`;
  const hours = Math.floor(minutes / 60);
  if (hours < 24) return `${hours}h ago`;
  const days = Math.floor(hours / 24);
  return `${days}d ago`;
}

function buildSparklinePaths(
  points: SparklinePoint[],
  width: number,
  height: number,
  padding: number,
): { area: string; line: string } {
  const values = points.length > 0 ? points.map((point) => point.value) : [0, 0];
  const minValue = Math.min(...values);
  const maxValue = Math.max(...values);
  const range = maxValue - minValue;
  const innerWidth = width - padding * 2;
  const innerHeight = height - padding * 2;

  const coords = values.map((value, index) => {
    const x =
      padding +
      (values.length === 1 ? innerWidth / 2 : (index * innerWidth) / (values.length - 1));
    const normalized =
      range === 0 ? (maxValue === 0 ? 0 : 0.5) : (value - minValue) / range;
    const y = height - padding - normalized * innerHeight;
    return { x, y };
  });

  const line = coords
    .map(({ x, y }, index) => `${index === 0 ? "M" : "L"}${x.toFixed(2)},${y.toFixed(2)}`)
    .join(" ");
  const area = `${line} L${(width - padding).toFixed(2)},${(height - padding).toFixed(2)} L${padding.toFixed(2)},${(height - padding).toFixed(2)} Z`;

  return { area, line };
}

function LiveSparkline({
  points,
  color,
}: {
  points: SparklinePoint[];
  color: string;
}) {
  const { area, line } = useMemo(
    () => buildSparklinePaths(points, CHART_WIDTH, CHART_HEIGHT, CHART_PADDING),
    [points],
  );

  return (
    <svg
      className="codex-live-sparkline"
      viewBox={`0 0 ${CHART_WIDTH} ${CHART_HEIGHT}`}
      preserveAspectRatio="none"
      aria-hidden="true"
    >
      <path
        d={line}
        fill="none"
        stroke={color}
        strokeWidth={2.25}
        strokeLinecap="round"
        strokeLinejoin="round"
      />
      <path d={area} fill={color} fillOpacity="0.08" />
    </svg>
  );
}

function StatCard({
  accentClass,
  color,
  label,
  value,
  points,
  title,
}: {
  accentClass: string;
  color: string;
  label: string;
  value: string;
  points: SparklinePoint[];
  title: string;
}) {
  return (
    <div className={`codex-live__stat ${accentClass}`} title={title}>
      <div className="codex-live__stat-head">
        <span className="codex-live__stat-label">{label}</span>
        <span className="codex-live__stat-value">{value}</span>
      </div>
      <div className="codex-live__stat-chart">
        <LiveSparkline points={points} color={color} />
      </div>
    </div>
  );
}

interface LiveSummaryModuleProps {
  enabledProviders: IntegrationProvider[];
}

function LiveSummaryModule({ enabledProviders }: LiveSummaryModuleProps) {
  const rootRef = useRef<HTMLDivElement | null>(null);
  const [range, setRange] = useState<LiveSummaryRange>("6h");
  const [pendingRange, setPendingRange] = useState<LiveSummaryRange | null>(null);
  const [committedFetchedAt, setCommittedFetchedAt] = useState<string | null>(null);
  const [livePaneHeight, setLivePaneHeight] = useState<number | null>(null);
  const { data, loading, error } = useLiveSummaryData(range, enabledProviders);
  const rangeLabel = LIVE_RANGE_LABELS[range];
  const isAwaitingFreshRange =
    pendingRange === range &&
    committedFetchedAt !== null &&
    data?.fetchedAt === committedFetchedAt;
  const compactHeight = livePaneHeight !== null && livePaneHeight < COMPACT_HEIGHT_PX;
  const freshness = isAwaitingFreshRange
    ? "loading"
    : formatRelativeTime(data?.lastActivityAt ?? null);

  useEffect(() => {
    if (!data) return;
    if (data.fetchedAt === committedFetchedAt) return;

    setCommittedFetchedAt(data.fetchedAt);
    if (pendingRange === range) {
      setPendingRange(null);
    }
  }, [committedFetchedAt, data, pendingRange, range]);

  useEffect(() => {
    if (!error) return;
    if (pendingRange !== range) return;
    setPendingRange(null);
  }, [error, pendingRange, range]);

  useEffect(() => {
    const el = rootRef.current?.closest(".live-content");
    if (!(el instanceof HTMLElement)) return;

    const observer = new ResizeObserver((entries) => {
      const entry = entries[0];
      if (!entry) return;
      setLivePaneHeight(entry.contentRect.height);
    });
    observer.observe(el);
    setLivePaneHeight(el.getBoundingClientRect().height);
    return () => observer.disconnect();
  }, []);

  if (enabledProviders.length === 0) {
    return null;
  }

  return (
    <div
      ref={rootRef}
      className={`usage-provider-group codex-live${compactHeight ? " codex-live--compact" : ""}`}
    >
      <div className="usage-provider-group__header codex-live__header codex-live__header--bare">
        <div className="codex-live__controls">
          <label className="codex-live__range-label" htmlFor="live-summary-range">
            Window
          </label>
          <select
            id="live-summary-range"
            className="codex-live__range-select"
            value={range}
            onChange={(event) => {
              const nextRange = event.target.value as LiveSummaryRange;
              setPendingRange(nextRange);
              setRange(nextRange);
            }}
          >
            {LIVE_RANGES.map((option) => (
              <option key={option} value={option}>
                {LIVE_RANGE_LABELS[option]}
              </option>
            ))}
          </select>
          <span className="codex-live__freshness">{freshness}</span>
        </div>
      </div>
      <div className="codex-live__body">
        {loading && !data ? (
          <div className="loading">{"Loading\u2026"}</div>
        ) : error && !data ? (
          <div className="error-label" role="alert">
            Failed to load live summary
          </div>
        ) : !data ? null : (
          <div className="codex-live__summary">
            <StatCard
              accentClass="codex-live__stat--sessions"
              color="#60a5fa"
              label="Sessions"
              value={formatNumber(data.activeSessions.value)}
              points={data.activeSessions.sparkline}
              title={`Active sessions across the last ${rangeLabel}`}
            />
            <StatCard
              accentClass="codex-live__stat--projects"
              color="#a78bfa"
              label="Projects"
              value={formatNumber(data.activeProjects.value)}
              points={data.activeProjects.sparkline}
              title={`Distinct active projects across the last ${rangeLabel}`}
            />
            <StatCard
              accentClass="codex-live__stat--tokens"
              color="#4ade80"
              label={`Tokens ${rangeLabel.toUpperCase()}`}
              value={formatTokenCount(data.tokens.value)}
              points={data.tokens.sparkline}
              title={`Tokens across the last ${rangeLabel}`}
            />
          </div>
        )}
      </div>
    </div>
  );
}

export default LiveSummaryModule;
