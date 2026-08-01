// ChartsView — three series, one time axis, one crosshair.
//
// The compact adaptation of the old full-window Charts tab: token flow, code
// changes and cache efficiency stacked at 360px so the operator can read one
// against the others. Its whole reason to exist is *correlation* — the Usage
// view already states each number on its own — so the three panels are only
// worth drawing if they are genuinely comparable.
//
// Three rules follow from that, and the file obeys them without exception:
//
//   - **One grid.** Every panel is bucketed onto the timestamps returned by
//     `get_provider_token_series` for the region's range. The code and cache
//     sources arrive on their own server-side granularity and are re-bucketed
//     here; a panel drawn on a different grid would make the shared crosshair
//     assert an alignment that does not exist (constitution #1).
//   - **One readout.** There is no floating tooltip. Scrubbing swaps each
//     panel's head value in place and brightens the matching axis tick, so the
//     crosshair costs no extra ink and no layout shift — the values are
//     tabular and right-aligned, the tick row never changes length.
//   - **Gaps stay gaps.** A bucket with no token traffic has no cache hit rate
//     to report, so the cache series breaks there rather than drawing 0%.
//     Tokens and code genuinely are zero when nothing happened, and are drawn
//     as zero.
//
// Colour: tokens carry provider identity (the same hues LIMITS and the hero
// chart teach). Added and removed lines are a *category*, and DESIGN.md's
// severity-code rule reserves green/amber/red for threshold state — so the
// diff pair is taken from the metric ramp (lime up, pink down) rather than the
// conventional green/red, and cache efficiency takes the widget's throughput
// cyan.
//
// See specs/018-widget-ui-redesign/plan.md#Affected Components.

import { useCallback, useEffect, useId, useMemo, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { areaPath, scalePoints, seriesMax, smoothPath, type VizPoint } from "../viz";
import { useCachedInvoke } from "../../../hooks/useCachedInvoke";
import { useCodeStats } from "../../../hooks/useCodeStats";
import { useRetentionCutoff } from "../../../hooks/useRetentionCutoff";
import { useProviderTokenSeries } from "../../../hooks/useWidgetSeries";
import { formatNumber } from "../../../utils/format";
import { formatRetentionCutoff } from "../../../utils/retention";
import { formatTokenCount } from "../../../utils/tokens";
import type {
  CodeStatsHistoryPoint,
  ProviderTokenSeriesResponse,
  RangeType,
  TokenDataPoint,
} from "../../../types";

/** viewBox width; matches the hero chart so both trace the same 332px plot. */
const CHART_WIDTH = 332;
const AREA_HEIGHT = 52;
const BAR_HEIGHT = 56;
/** Headroom above the tallest value so a peak never touches the head row. */
const AREA_TOP_PAD = 7;
/** Half the gap between the zero line and the tallest bar, in either direction. */
const BAR_PAD = 4;
const REFRESH_DEBOUNCE_MS = 1000;
const REFRESH_INTERVAL_MS = 60_000;

/** Added and removed lines are categories, so they take metric hues, not severity. */
const HUE_ADDED = "var(--metric-net-lines)";
const HUE_REMOVED = "var(--metric-loc-per-hr)";
const HUE_CACHE = "var(--metric-runtime)";

/**
 * Area fills fade to nothing rather than sitting as a flat slab, matching the
 * hero chart. `useId()` emits punctuation that is illegal inside a `url(#…)`
 * reference, so the ids are reduced to word characters before they reach the
 * DOM.
 */
function gradientId(prefix: string, key: string): string {
  return `${prefix}-${key.replace(/\W/g, "")}`;
}

/** Provider identity hue. An unrecognized producer still has to be charted. */
function providerHue(provider: string): string {
  if (provider === "claude") return "var(--provider-claude)";
  if (provider === "codex") return "var(--provider-codex)";
  if (provider === "mini_max") return "var(--provider-minimax)";
  return "var(--provider-agent)";
}

function providerTag(provider: string): string {
  if (provider === "mini_max") return "MINIMAX";
  return provider.replace(/_/g, "").toUpperCase();
}

/**
 * Axis captions for the shared axis. Intraday ranges read as clock time; a
 * week reads as weekdays, because eight `HH:MM` labels across seven days say
 * nothing about which day.
 */
function axisLabels(timestamps: readonly string[], range: RangeType): string[] {
  const daily = range === "7d" || range === "30d";
  return timestamps.map((timestamp) => {
    const date = new Date(timestamp);
    if (Number.isNaN(date.getTime())) return "";
    return daily
      ? date.toLocaleDateString(undefined, { weekday: "short" })
      : date.toLocaleTimeString(undefined, {
          hour: "2-digit",
          minute: "2-digit",
          hour12: false,
        });
  });
}

/** Full bucket caption used by the scrub announcement, e.g. `Mar 4, 18:30`. */
function scrubCaption(timestamp: string | undefined): string {
  if (!timestamp) return "";
  const date = new Date(timestamp);
  if (Number.isNaN(date.getTime())) return "";
  return date.toLocaleString(undefined, {
    month: "short",
    day: "numeric",
    hour: "2-digit",
    minute: "2-digit",
    hour12: false,
  });
}

/**
 * Maps an instant onto the token series' bucket grid.
 *
 * The bucket width is read from the grid itself rather than recomputed from
 * the range, so the panels follow whatever alignment the server chose. Points
 * older than the first bucket are dropped; points past the final bucket edge
 * fold into the last bucket, which is still filling.
 */
function bucketAssigner(timestamps: readonly string[]): (iso: string) => number {
  const starts = timestamps.map((value) => new Date(value).getTime());
  const last = starts.length - 1;
  const step = starts.length > 1 ? starts[1] - starts[0] : 0;
  return (iso: string) => {
    if (last < 0) return -1;
    const at = new Date(iso).getTime();
    if (!Number.isFinite(at) || !Number.isFinite(starts[0])) return -1;
    if (at < starts[0]) return -1;
    if (step <= 0) return last;
    return Math.min(last, Math.floor((at - starts[0]) / step));
  };
}

interface CodeBuckets {
  readonly added: readonly number[];
  readonly removed: readonly number[];
  readonly totalAdded: number;
  readonly totalRemoved: number;
}

/** Re-buckets `get_code_stats_history` onto the token series grid. */
function bucketCode(
  history: readonly CodeStatsHistoryPoint[],
  timestamps: readonly string[],
): CodeBuckets {
  const assign = bucketAssigner(timestamps);
  const added = new Array<number>(timestamps.length).fill(0);
  const removed = new Array<number>(timestamps.length).fill(0);
  let totalAdded = 0;
  let totalRemoved = 0;
  for (const point of history) {
    const index = assign(point.timestamp);
    if (index < 0) continue;
    added[index] += point.lines_added;
    removed[index] += point.lines_removed;
    totalAdded += point.lines_added;
    totalRemoved += point.lines_removed;
  }
  return { added, removed, totalAdded, totalRemoved };
}

interface CacheBuckets {
  /** Hit rate as a percentage; meaningless where `covered` is false. */
  readonly rates: readonly number[];
  /** Whether the bucket saw any cacheable input at all. */
  readonly covered: readonly boolean[];
  /** Weighted rate across the whole range, or null when nothing was cacheable. */
  readonly rangeRate: number | null;
}

/**
 * Re-buckets token history into a per-bucket cache hit rate.
 *
 * Rates are computed from summed numerators and denominators rather than by
 * averaging point-wise rates, so a busy minute is not outvoted by an idle one,
 * and the range figure is the same weighted rate the footer reports.
 */
function bucketCache(
  history: readonly TokenDataPoint[],
  timestamps: readonly string[],
): CacheBuckets {
  const assign = bucketAssigner(timestamps);
  const reads = new Array<number>(timestamps.length).fill(0);
  const totals = new Array<number>(timestamps.length).fill(0);
  let totalRead = 0;
  let totalCacheable = 0;
  for (const point of history) {
    const index = assign(point.timestamp);
    if (index < 0) continue;
    const cacheable =
      point.input_tokens +
      point.cache_creation_input_tokens +
      point.cache_read_input_tokens;
    reads[index] += point.cache_read_input_tokens;
    totals[index] += cacheable;
    totalRead += point.cache_read_input_tokens;
    totalCacheable += cacheable;
  }
  return {
    rates: totals.map((total, index) =>
      total > 0 ? Math.round((reads[index] / total) * 1000) / 10 : 0,
    ),
    covered: totals.map((total) => total > 0),
    rangeRate:
      totalCacheable > 0 ? Math.round((totalRead / totalCacheable) * 100) : null,
  };
}

/** Contiguous `[start, end]` index runs where `covered` holds. */
function coveredRuns(covered: readonly boolean[]): ReadonlyArray<[number, number]> {
  const runs: Array<[number, number]> = [];
  let start = -1;
  covered.forEach((value, index) => {
    if (value && start < 0) start = index;
    if (!value && start >= 0) {
      runs.push([start, index - 1]);
      start = -1;
    }
  });
  if (start >= 0) runs.push([start, covered.length - 1]);
  return runs;
}

/** Per-bucket sum across every provider series. */
function bucketTotals(series: ReadonlyArray<{ values: number[] }>): number[] {
  const length = series.reduce((max, entry) => Math.max(max, entry.values.length), 0);
  const totals = new Array<number>(length).fill(0);
  for (const entry of series) {
    entry.values.forEach((value, index) => {
      totals[index] += value;
    });
  }
  return totals;
}

/** Signed line counts with a typographic minus: `+1,923 / −412`. */
function formatDiff(added: number, removed: number): string {
  return `+${formatNumber(added)} / −${formatNumber(removed)}`;
}

/**
 * Token history for the selected range.
 *
 * Deliberately narrow: the legacy analytics hook this replaces also fetched
 * range totals and the hostname list, and this view draws neither. Only the
 * four token components are needed, and only to derive a per-bucket cache hit
 * rate.
 */
function useWidgetTokenHistory(range: RangeType) {
  const request = useCallback(
    () =>
      invoke<TokenDataPoint[]>("get_token_history", {
        range,
        provider: null,
        hostname: null,
        sessionId: null,
        cwd: null,
      }),
    [range],
  );
  const { state, refresh } = useCachedInvoke({
    identity: `widget-token-history:${range}`,
    request,
    normalizeError: String,
  });

  useEffect(() => {
    let timer: ReturnType<typeof setTimeout> | null = null;
    const schedule = () => {
      if (timer) clearTimeout(timer);
      timer = setTimeout(refresh, REFRESH_DEBOUNCE_MS);
    };
    const unlisten = listen("tokens-updated", schedule);
    const interval = setInterval(refresh, REFRESH_INTERVAL_MS);
    return () => {
      if (timer) clearTimeout(timer);
      clearInterval(interval);
      unlisten.then((stop) => stop());
    };
  }, [refresh]);

  return {
    history: state.data ?? [],
    loading: state.initialLoading,
    error: state.error,
  };
}

/** Point x coordinates are bucket centres, so bars and lines share one grid. */
function bucketGeometry(buckets: number) {
  const cell = buckets > 0 ? CHART_WIDTH / buckets : CHART_WIDTH;
  return {
    cell,
    pad: cell / 2,
    center: (index: number) => (index + 0.5) * cell,
  };
}

interface PanelProps {
  label: string;
  /** One swatch per series drawn in the panel. */
  swatches: ReadonlyArray<{ key: string; hue: string }>;
  value: string;
  children: React.ReactNode;
}

/** One stacked panel: a key row, a right-aligned readout, and its plot. */
function ChartPanel({ label, swatches, value, children }: PanelProps) {
  return (
    <div className="wg-chart">
      <div className="wg-chart-head">
        <span className="wg-chart-key">
          {/* The swatch column holds a fixed width so all three keys share one
              left edge and the tokens label does not jog when its providers
              resolve. */}
          <span className="wg-chart-swatches" aria-hidden="true">
            {swatches.map((swatch) => (
              <i
                className="wg-chart-swatch"
                key={swatch.key}
                style={{ background: swatch.hue }}
              />
            ))}
          </span>
          {label}
        </span>
        <span className="wg-chart-value wg-num">{value}</span>
      </div>
      {children}
    </div>
  );
}

/** The band's loading and failure shapes, at the panels' own heights. */
function PanelPlaceholder({ height, error }: { height: number; error?: string }) {
  if (error) {
    return (
      <div className="wg-state wg-state-error" style={{ minHeight: `${height}px` }}>
        <span className="wg-state-lamp" aria-hidden="true" />
        {error}
      </div>
    );
  }
  return (
    <div
      className="wg-skeleton wg-skeleton-block"
      style={{ height: `${height}px` }}
      aria-hidden="true"
    />
  );
}

function EmptyPanel({ height, label }: { height: number; label: string }) {
  return (
    <div className="wg-state" style={{ minHeight: `${height}px` }}>
      <span className="wg-state-lamp" aria-hidden="true" />
      {label}
    </div>
  );
}

/** The vertical rule every panel draws at the scrubbed bucket. */
function Crosshair({ x, height }: { x: number; height: number }) {
  return (
    <line
      className="wg-chart-cross"
      x1={x}
      y1={0}
      x2={x}
      y2={height}
      stroke="rgba(255,255,255,0.16)"
      strokeWidth={1}
    />
  );
}

export interface ChartsViewProps {
  range: RangeType;
}

function ChartsView({ range }: ChartsViewProps) {
  const tokenSeries = useProviderTokenSeries(range);
  const code = useCodeStats(range);
  const tokenHistory = useWidgetTokenHistory(range);
  const retention = useRetentionCutoff();

  const [scrub, setScrub] = useState<number | null>(null);
  const instanceId = useId().replace(/\W/g, "");
  const helpId = `${instanceId}-charts-help`;
  const gradientPrefix = `viz${instanceId}`;

  const response: ProviderTokenSeriesResponse | null = tokenSeries.data;
  const timestamps = useMemo(() => response?.timestamps ?? [], [response]);
  const buckets = timestamps.length;
  const geometry = useMemo(() => bucketGeometry(buckets), [buckets]);

  const labels = useMemo(() => axisLabels(timestamps, range), [timestamps, range]);

  const tokens = useMemo(() => {
    if (!response) return null;
    const totals = bucketTotals(response.series);
    const max = seriesMax(response.series.map((entry) => entry.values));
    const plotted = response.series.map((entry) => ({
      id: entry.provider,
      hue: providerHue(entry.provider),
      tag: providerTag(entry.provider),
      total: entry.total_tokens,
      points: scalePoints(entry.values, {
        width: CHART_WIDTH,
        baseline: AREA_HEIGHT - 1,
        plotHeight: AREA_HEIGHT - AREA_TOP_PAD - 1,
        padStart: geometry.pad,
        padEnd: geometry.pad,
        max: max > 0 ? max : 1,
      }),
    }));
    return { totals, max, plotted, rangeTotal: response.total_tokens };
  }, [response, geometry]);

  const codeBuckets = useMemo(
    () => bucketCode(code.history, timestamps),
    [code.history, timestamps],
  );

  const cacheBuckets = useMemo(
    () => bucketCache(tokenHistory.history, timestamps),
    [tokenHistory.history, timestamps],
  );

  const cachePoints = useMemo<VizPoint[]>(
    () =>
      scalePoints(cacheBuckets.rates, {
        width: CHART_WIDTH,
        baseline: AREA_HEIGHT - 1,
        plotHeight: AREA_HEIGHT - AREA_TOP_PAD - 1,
        padStart: geometry.pad,
        padEnd: geometry.pad,
        min: 0,
        max: 100,
      }),
    [cacheBuckets.rates, geometry],
  );

  const cacheRuns = useMemo(() => coveredRuns(cacheBuckets.covered), [cacheBuckets]);

  const codeScale = useMemo(() => {
    let peak = 0;
    for (const value of codeBuckets.added) peak = Math.max(peak, value);
    for (const value of codeBuckets.removed) peak = Math.max(peak, value);
    return peak;
  }, [codeBuckets]);

  // A range change re-grids everything, so a held index would point at a
  // bucket that no longer exists.
  useEffect(() => {
    setScrub(null);
  }, [range]);

  const clampScrub = useCallback(
    (index: number) => Math.min(Math.max(index, 0), Math.max(0, buckets - 1)),
    [buckets],
  );

  const handlePointer = useCallback(
    (event: React.PointerEvent<HTMLDivElement>) => {
      if (buckets === 0) return;
      const rect = event.currentTarget.getBoundingClientRect();
      if (rect.width <= 0) return;
      const fraction = (event.clientX - rect.left) / rect.width;
      setScrub(clampScrub(Math.floor(fraction * buckets)));
    },
    [buckets, clampScrub],
  );

  const handleKeyDown = useCallback(
    (event: React.KeyboardEvent<HTMLDivElement>) => {
      if (buckets === 0) return;
      const current = scrub ?? buckets - 1;
      if (event.key === "ArrowRight") {
        event.preventDefault();
        setScrub(clampScrub(current + 1));
      } else if (event.key === "ArrowLeft") {
        event.preventDefault();
        setScrub(clampScrub(current - 1));
      } else if (event.key === "Home") {
        event.preventDefault();
        setScrub(0);
      } else if (event.key === "End") {
        event.preventDefault();
        setScrub(buckets - 1);
      } else if (event.key === "Escape") {
        setScrub(null);
      }
    },
    [buckets, clampScrub, scrub],
  );

  const active = scrub !== null && scrub < buckets ? scrub : null;
  const crossX = active === null ? 0 : geometry.center(active);

  // Heads read "—" until their own source lands, so a panel never states a
  // zero it has not measured, and the head rows keep their place from the
  // first paint.
  const codeReady = response !== null && !code.error && !(code.loading && code.history.length === 0);
  const cacheReady =
    response !== null &&
    !tokenHistory.error &&
    !(tokenHistory.loading && tokenHistory.history.length === 0);

  const tokenValue =
    tokens === null
      ? "—"
      : active === null
        ? formatTokenCount(tokens.rangeTotal)
        : formatTokenCount(tokens.totals[active] ?? 0);

  const codeValue = !codeReady
    ? "—"
    : active === null
      ? formatDiff(codeBuckets.totalAdded, codeBuckets.totalRemoved)
      : formatDiff(codeBuckets.added[active] ?? 0, codeBuckets.removed[active] ?? 0);

  const cacheValue = (() => {
    if (!cacheReady) return "—";
    if (active === null) {
      return cacheBuckets.rangeRate === null ? "—" : `${cacheBuckets.rangeRate}%`;
    }
    if (!cacheBuckets.covered[active]) return "—";
    return `${Math.round(cacheBuckets.rates[active])}%`;
  })();

  const announcement =
    active === null
      ? ""
      : `${scrubCaption(timestamps[active])} — tokens ${tokenValue}, code ${codeValue}, cache ${cacheValue}`;

  // Retention prunes `tool_actions`, which is what the code panel reads. The
  // disclosure appears only when the watermark actually falls inside the drawn
  // window; outside it, nothing on screen is degraded.
  const retentionInWindow =
    retention.cutoff !== null &&
    buckets > 0 &&
    new Date(retention.cutoff).getTime() > new Date(timestamps[0]).getTime();

  const tokenSwatches = (tokens?.plotted ?? []).map((entry) => ({
    key: entry.id,
    hue: entry.hue,
  }));

  const seriesSummary = (tokens?.plotted ?? [])
    .map((entry) => `${entry.tag} ${formatTokenCount(entry.total)}`)
    .join(", ");

  // Without the token grid there is no shared axis, so the whole band fails as
  // one rather than drawing three panels that cannot be compared.
  if (!response && tokenSeries.error) {
    return (
      <div className="wg-charts">
        <div className="wg-state wg-state-error">
          <span className="wg-state-lamp" aria-hidden="true" />
          Chart series unavailable
        </div>
      </div>
    );
  }

  return (
    <div className="wg-charts">
      <p className="wg-sr" id={helpId}>
        Move the pointer across the charts, or use the left and right arrow keys,
        to read all three series at one point in time. Escape clears the reading.
      </p>
      <div
        className="wg-charts-stack"
        role="group"
        aria-label="Tokens, code changes and cache efficiency on one time axis"
        aria-describedby={helpId}
        tabIndex={0}
        onPointerMove={handlePointer}
        onPointerLeave={() => setScrub(null)}
        onBlur={() => setScrub(null)}
        onKeyDown={handleKeyDown}
      >
        <ChartPanel label="Tokens" swatches={tokenSwatches} value={tokenValue}>
          {tokens === null ? (
            <PanelPlaceholder height={AREA_HEIGHT} />
          ) : tokens.max <= 0 ? (
            <EmptyPanel height={AREA_HEIGHT} label="No tokens in this range" />
          ) : (
            <svg
              className="wg-chart-svg"
              viewBox={`0 0 ${CHART_WIDTH} ${AREA_HEIGHT}`}
              preserveAspectRatio="none"
              style={{ height: `${AREA_HEIGHT}px` }}
              role="img"
              aria-label={`Tokens over the selected range: ${formatTokenCount(
                tokens?.rangeTotal ?? 0,
              )} total${seriesSummary ? ` — ${seriesSummary}` : ""}`}
            >
              <defs>
                {tokens?.plotted.map((entry) => (
                  <linearGradient
                    key={entry.id}
                    id={gradientId(gradientPrefix, entry.id)}
                    x1="0"
                    y1="0"
                    x2="0"
                    y2="1"
                  >
                    <stop offset="0%" stopColor={entry.hue} stopOpacity={0.2} />
                    <stop offset="100%" stopColor={entry.hue} stopOpacity={0} />
                  </linearGradient>
                ))}
              </defs>
              {tokens?.plotted.map((entry) => (
                <path
                  key={`${entry.id}-fill`}
                  d={areaPath(entry.points, AREA_HEIGHT)}
                  fill={`url(#${gradientId(gradientPrefix, entry.id)})`}
                />
              ))}
              {tokens?.plotted.map((entry) => (
                <path
                  key={`${entry.id}-line`}
                  d={smoothPath(entry.points)}
                  fill="none"
                  stroke={entry.hue}
                  strokeWidth={1.4}
                  strokeLinecap="round"
                />
              ))}
              {active !== null && <Crosshair x={crossX} height={AREA_HEIGHT} />}
              {active !== null &&
                tokens?.plotted.map((entry) => {
                  const point = entry.points[active];
                  if (!point) return null;
                  return (
                    <circle
                      key={`${entry.id}-mark`}
                      cx={point.x}
                      cy={point.y}
                      r={2.2}
                      fill={entry.hue}
                      stroke="var(--surface)"
                      strokeWidth={1.2}
                    />
                  );
                })}
            </svg>
          )}
        </ChartPanel>

        <ChartPanel
          label="Code"
          swatches={[
            { key: "added", hue: HUE_ADDED },
            { key: "removed", hue: HUE_REMOVED },
          ]}
          value={codeValue}
        >
          {code.error ? (
            <PanelPlaceholder height={BAR_HEIGHT} error="Code history unavailable" />
          ) : !codeReady ? (
            <PanelPlaceholder height={BAR_HEIGHT} />
          ) : codeScale <= 0 ? (
            <EmptyPanel height={BAR_HEIGHT} label="No code changes in this range" />
          ) : (
            <svg
              className="wg-chart-svg"
              viewBox={`0 0 ${CHART_WIDTH} ${BAR_HEIGHT}`}
              preserveAspectRatio="none"
              style={{ height: `${BAR_HEIGHT}px` }}
              role="img"
              aria-label={`Code changes over the selected range: ${formatDiff(
                codeBuckets.totalAdded,
                codeBuckets.totalRemoved,
              )} lines`}
            >
              <line
                x1={0}
                y1={BAR_HEIGHT / 2}
                x2={CHART_WIDTH}
                y2={BAR_HEIGHT / 2}
                stroke="var(--line)"
                strokeWidth={1}
              />
              {codeBuckets.added.map((value, index) => {
                const span = BAR_HEIGHT / 2 - BAR_PAD;
                const width = geometry.cell * 0.46;
                const x = geometry.center(index) - width / 2;
                const height = codeScale > 0 ? (value / codeScale) * span : 0;
                if (height <= 0) return null;
                return (
                  <rect
                    key={`added-${index}`}
                    x={x}
                    y={BAR_HEIGHT / 2 - height}
                    width={width}
                    height={height}
                    rx={1}
                    fill={HUE_ADDED}
                    fillOpacity={0.62}
                  />
                );
              })}
              {codeBuckets.removed.map((value, index) => {
                const span = BAR_HEIGHT / 2 - BAR_PAD;
                const width = geometry.cell * 0.46;
                const x = geometry.center(index) - width / 2;
                const height = codeScale > 0 ? (value / codeScale) * span : 0;
                if (height <= 0) return null;
                return (
                  <rect
                    key={`removed-${index}`}
                    x={x}
                    y={BAR_HEIGHT / 2}
                    width={width}
                    height={height}
                    rx={1}
                    fill={HUE_REMOVED}
                    fillOpacity={0.52}
                  />
                );
              })}
              {active !== null && <Crosshair x={crossX} height={BAR_HEIGHT} />}
            </svg>
          )}
        </ChartPanel>

        <ChartPanel
          label="Cache"
          swatches={[{ key: "cache", hue: HUE_CACHE }]}
          value={cacheValue}
        >
          {tokenHistory.error ? (
            <PanelPlaceholder height={AREA_HEIGHT} error="Cache history unavailable" />
          ) : !cacheReady ? (
            <PanelPlaceholder height={AREA_HEIGHT} />
          ) : cacheRuns.length === 0 ? (
            <EmptyPanel height={AREA_HEIGHT} label="No cacheable input in this range" />
          ) : (
            <svg
              className="wg-chart-svg"
              viewBox={`0 0 ${CHART_WIDTH} ${AREA_HEIGHT}`}
              preserveAspectRatio="none"
              style={{ height: `${AREA_HEIGHT}px` }}
              role="img"
              aria-label={`Cache hit rate over the selected range: ${cacheValue} of cacheable input served from cache`}
            >
              <defs>
                <linearGradient
                  id={gradientId(gradientPrefix, "cache")}
                  x1="0"
                  y1="0"
                  x2="0"
                  y2="1"
                >
                  <stop offset="0%" stopColor={HUE_CACHE} stopOpacity={0.22} />
                  <stop offset="100%" stopColor={HUE_CACHE} stopOpacity={0} />
                </linearGradient>
              </defs>
              {/* Half-scale hairline: the cache axis is a fixed 0–100%, so the
                  line is a real reference rather than decoration. */}
              <line
                x1={0}
                y1={(AREA_HEIGHT - 1) / 2}
                x2={CHART_WIDTH}
                y2={(AREA_HEIGHT - 1) / 2}
                stroke="var(--line-soft)"
                strokeWidth={1}
              />
              {cacheRuns.map(([start, end]) => {
                const run = cachePoints.slice(start, end + 1);
                if (run.length < 2) {
                  const only = run[0];
                  return only ? (
                    <circle
                      key={`run-${start}`}
                      cx={only.x}
                      cy={only.y}
                      r={1.8}
                      fill={HUE_CACHE}
                    />
                  ) : null;
                }
                return (
                  <g key={`run-${start}`}>
                    <path
                      d={areaPath(run, AREA_HEIGHT)}
                      fill={`url(#${gradientId(gradientPrefix, "cache")})`}
                    />
                    <path
                      d={smoothPath(run)}
                      fill="none"
                      stroke={HUE_CACHE}
                      strokeWidth={1.4}
                      strokeLinecap="round"
                    />
                  </g>
                );
              })}
              {active !== null && <Crosshair x={crossX} height={AREA_HEIGHT} />}
              {active !== null && cacheBuckets.covered[active] && cachePoints[active] && (
                <circle
                  cx={cachePoints[active].x}
                  cy={cachePoints[active].y}
                  r={2.2}
                  fill={HUE_CACHE}
                  stroke="var(--surface)"
                  strokeWidth={1.2}
                />
              )}
            </svg>
          )}
        </ChartPanel>

        {/* The axis is the time readout: the scrubbed tick brightens instead of
            a floating chip appearing, so nothing below ever moves. */}
        <div className="wg-charts-axis wg-num" aria-hidden="true">
          {labels.map((label, index) => (
            <span
              key={`${label}-${index}`}
              data-active={index === active ? "true" : undefined}
            >
              {label}
            </span>
          ))}
        </div>
      </div>

      {retentionInWindow && retention.cutoff && (
        <p
          className="wg-chart-note"
          role="note"
          title="Code changes are derived from recorded tool activity. Activity older than this date was pruned, so buckets before it under-report rather than being empty."
        >
          Retention · code changes before {formatRetentionCutoff(retention.cutoff)} were
          pruned
        </p>
      )}

      <p className="wg-sr" role="status" aria-live="polite">
        {announcement}
      </p>
    </div>
  );
}

export default ChartsView;
