import { useEffect, useMemo, useRef, useState } from "react";
import { useCodexLiveData } from "../../hooks/useCodexLiveData";
import type { CodexLiveRange, CodexLiveSessionRow, SparklinePoint } from "../../types";
import { formatNumber } from "../../utils/format";
import { providerLabel } from "../../utils/providers";
import { formatTokenCount } from "../../utils/tokens";

const CHART_WIDTH = 100;
const CHART_HEIGHT = 40;
const CHART_PADDING = 3;
const COMPACT_HEIGHT_PX = 460;
const MEDIUM_HEIGHT_PX = 430;
const TIGHT_HEIGHT_PX = 360;
const LIVE_RANGES: CodexLiveRange[] = ["1h", "6h", "12h", "24h"];
const LIVE_RANGE_LABELS: Record<CodexLiveRange, string> = {
  "1h": "1h",
  "6h": "6h",
  "12h": "12h",
  "24h": "24h",
};

interface LiveSparklineProps {
  points: SparklinePoint[];
  color: string;
  filled?: boolean;
  height?: number;
  lineWidth?: number;
}

interface CodexLiveModuleProps {
  showHeading?: boolean;
}

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

function projectName(project: string | null, sessionId: string): string {
  if (!project) return sessionId.slice(0, 8);
  const segments = project.split(/[\\/]/).filter(Boolean);
  return segments[segments.length - 1] ?? project;
}

function sessionSubline(session: CodexLiveSessionRow): string {
  if (session.project) return session.project;
  return `${session.sessionId.slice(0, 8)} • ${session.hostname}`;
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
  filled = false,
  height = CHART_HEIGHT,
  lineWidth = 2.25,
}: LiveSparklineProps) {
  const { area, line } = useMemo(
    () => buildSparklinePaths(points, CHART_WIDTH, height, CHART_PADDING),
    [points, height],
  );

  return (
    <svg
      className={`codex-live-sparkline${filled ? " codex-live-sparkline--filled" : ""}`}
      viewBox={`0 0 ${CHART_WIDTH} ${height}`}
      preserveAspectRatio="none"
      aria-hidden="true"
    >
      {filled && <path d={area} fill={color} fillOpacity="0.16" />}
      <path
        d={line}
        fill="none"
        stroke={color}
        strokeWidth={lineWidth}
        strokeLinecap="round"
        strokeLinejoin="round"
      />
    </svg>
  );
}

interface StatCardProps {
  accentClass: string;
  color: string;
  label: string;
  value: string;
  points: SparklinePoint[];
  title: string;
}

function StatCard({
  accentClass,
  color,
  label,
  value,
  points,
  title,
}: StatCardProps) {
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

function SessionRow({ session }: { session: CodexLiveSessionRow }) {
  return (
    <div className="codex-live__session" title={session.project ?? session.sessionId}>
      <div className="codex-live__session-main">
        <span className="codex-live__session-title">
          {projectName(session.project, session.sessionId)}
        </span>
        <span className="codex-live__session-path">{sessionSubline(session)}</span>
      </div>
      <div className="codex-live__session-side">
        <span className="codex-live__session-tokens">
          {formatTokenCount(session.tokenTotal)}
        </span>
        <span className="codex-live__session-age">
          {formatRelativeTime(session.lastActive)}
        </span>
      </div>
    </div>
  );
}

function CodexLiveModule({ showHeading = true }: CodexLiveModuleProps) {
  const rootRef = useRef<HTMLDivElement | null>(null);
  const [range, setRange] = useState<CodexLiveRange>("1h");
  const [pendingRange, setPendingRange] = useState<CodexLiveRange | null>(null);
  const [committedFetchedAt, setCommittedFetchedAt] = useState<string | null>(null);
  const [livePaneHeight, setLivePaneHeight] = useState<number | null>(null);
  const { data, loading, error } = useCodexLiveData(range);
  const rangeLabel = useMemo(() => LIVE_RANGE_LABELS[range], [range]);
  const isAwaitingFreshRange =
    pendingRange === range &&
    committedFetchedAt !== null &&
    data?.fetchedAt === committedFetchedAt;
  const compactHeight =
    livePaneHeight !== null && livePaneHeight < COMPACT_HEIGHT_PX;
  const visibleSessionCount =
    livePaneHeight === null
      ? 5
      : livePaneHeight < TIGHT_HEIGHT_PX
        ? 3
        : livePaneHeight < MEDIUM_HEIGHT_PX
          ? 4
          : 5;
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

  const header = (
    <div
      className={`usage-provider-group__header codex-live__header${showHeading ? "" : " codex-live__header--bare"}`}
    >
      {showHeading && (
        <div className="codex-live__header-main">
          <span className="usage-provider-badge usage-provider-badge--codex">
            {providerLabel("codex")}
          </span>
          <span className="codex-live__meta">Session activity</span>
        </div>
      )}
      <div className="codex-live__controls">
        <label className="codex-live__range-label" htmlFor="codex-live-range">
          Window
        </label>
        <select
          id="codex-live-range"
          className="codex-live__range-select"
          value={range}
          onChange={(event) => {
            const nextRange = event.target.value as CodexLiveRange;
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
  );

  const showLoadingBody = loading && !data;
  const showErrorBody = error && !data;
  const showPendingBody = isAwaitingFreshRange && !showErrorBody && !error;

  if (showLoadingBody) {
    return (
      <div
        ref={rootRef}
        className={`usage-provider-group codex-live${compactHeight ? " codex-live--compact" : ""}`}
      >
        {header}
        <div className="loading">{"Loading\u2026"}</div>
      </div>
    );
  }

  if (showErrorBody) {
    return (
      <div
        ref={rootRef}
        className={`usage-provider-group codex-live${compactHeight ? " codex-live--compact" : ""}`}
      >
        {header}
        <div className="error-label" role="alert">
          Failed to load Codex activity
        </div>
      </div>
    );
  }

  if (!data) return null;

  return (
    <div
      ref={rootRef}
      className={`usage-provider-group codex-live${compactHeight ? " codex-live--compact" : ""}`}
    >
      {header}
      <div className="codex-live__body">
        {showPendingBody ? (
          <div className="loading">{"Loading\u2026"}</div>
        ) : error ? (
          <div className="error-label" role="alert">
            Failed to load Codex activity
          </div>
        ) : (
          <div className="codex-live__grid">
            <div className="codex-live__summary">
              <StatCard
                accentClass="codex-live__stat--sessions"
                color="#60a5fa"
                label="Sessions"
                value={formatNumber(data.activeSessions.value)}
                points={data.activeSessions.sparkline}
                title={`Active Codex sessions across the last ${rangeLabel}`}
              />
              <StatCard
                accentClass="codex-live__stat--projects"
                color="#a78bfa"
                label="Projects"
                value={formatNumber(data.activeProjects.value)}
                points={data.activeProjects.sparkline}
                title={`Distinct active Codex projects across the last ${rangeLabel}`}
              />
              <StatCard
                accentClass="codex-live__stat--tokens"
                color="#4ade80"
                label={`Tokens ${rangeLabel.toUpperCase()}`}
                value={formatTokenCount(data.tokens.value)}
                points={data.tokens.sparkline}
                title={`Codex tokens across the last ${rangeLabel}`}
              />
            </div>
            <div className="codex-live__ledger">
              <div className="codex-live__ledger-head">
                <span className="codex-live__ledger-title">Active Sessions</span>
                <span className="codex-live__ledger-count">
                  {formatNumber(data.sessions.length)}
                </span>
              </div>
              {data.sessions.length > 0 ? (
                <div className="codex-live__session-list">
                  {data.sessions.slice(0, visibleSessionCount).map((session) => (
                    <SessionRow key={session.sessionId} session={session} />
                  ))}
                </div>
              ) : (
                <div className="codex-live__empty">No active sessions in the last {rangeLabel}.</div>
              )}
            </div>
          </div>
        )}
      </div>
    </div>
  );
}

export default CodexLiveModule;
