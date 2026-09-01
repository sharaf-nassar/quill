// Explore — every usage metric overlaid on one large graph.
//
// The widget's Usage view answers "what happened in this window" one band at
// a time; this section exists to answer "how do these move *together*" over
// weeks or months. Token series (grouped by CLI, provider, or model — the
// same dimensions as the widget chart) and the six readout metrics share one
// daily grid from [[useExploreSeries]], each rescaled onto a common plane.
//
// Two honesty rules carried over from the widget:
//   - Units differ wildly (tokens vs hours vs lines), so series are rescaled
//     per-series and the header states the rule in words. Raw values stay
//     authoritative in the tooltip, panel totals, and endpoint labels.
//   - Runtime has no daily backend source (its sparkline is a fixed 7-bucket
//     grid), so its series is labelled avg/day rather than pretending to
//     daily evidence. See the useExploreSeries header.

import { useEffect, useMemo, useState } from "react";
import ExploreChart, {
  type ExploreChartSeries,
  type ExploreScale,
} from "../components/explore/ExploreChart";
import { chartSeriesFor } from "../components/widget/chartDimensions";
import type { WidgetChartDimension } from "../components/widget/rangePreference";
import {
  EXPLORE_RANGE_DAYS,
  useExploreSeries,
  type ExploreRange,
} from "../hooks/useExploreSeries";
import { formatDurationSecs, formatNumber, formatSessionModel } from "../utils/format";
import { providerTag } from "../utils/providers";
import { formatTokenCount } from "../utils/tokens";
import type { IntegrationProvider } from "../types";
import "../styles/explore.css";

// Literal hex twins of the identity tokens in index.css — SVG gradient stops
// cannot resolve CSS vars, so the chart needs real colors.
const PROVIDER_HEX: Record<string, string> = {
  claude: "#fb923c",
  codex: "#60a5fa",
  pi: "#15803d",
  mini_max: "#a78bfa",
};
const providerHex = (provider: string): string =>
  PROVIDER_HEX[provider] ?? "#c084fc";

const fmtSignedLines = (value: number): string =>
  value > 0 ? `+${formatNumber(Math.round(value))}` : formatNumber(Math.round(value));

interface MetricDef {
  id: string;
  key: "runtime" | "tokPerLoc" | "locPerHour" | "sessions" | "projects" | "netLines";
  label: string;
  color: string;
  format: (value: number) => string;
}

/** Hues match the widget's --metric-* tokens. */
const METRICS: MetricDef[] = [
  { id: "m:runtime", key: "runtime", label: "Runtime · avg/day", color: "#22d3ee", format: formatDurationSecs },
  { id: "m:tokloc", key: "tokPerLoc", label: "Tok / LOC", color: "#a78bfa", format: (v) => formatNumber(Math.round(v)) },
  { id: "m:lochr", key: "locPerHour", label: "LOC / hr", color: "#f472b6", format: (v) => formatNumber(Math.round(v)) },
  { id: "m:sessions", key: "sessions", label: "Sessions", color: "#818cf8", format: (v) => formatNumber(Math.round(v)) },
  { id: "m:projects", key: "projects", label: "Projects", color: "#2dd4bf", format: (v) => formatNumber(Math.round(v)) },
  { id: "m:netlines", key: "netLines", label: "Net lines", color: "#a3e635", format: fmtSignedLines },
];

const RANGES: ReadonlyArray<[ExploreRange, string]> = [
  ["7d", "1W"],
  ["30d", "1M"],
  ["90d", "3M"],
];
const SCALES: ReadonlyArray<[ExploreScale, string]> = [
  ["norm", "Normalized"],
  ["index", "Indexed"],
];
const DIMENSIONS: ReadonlyArray<[WidgetChartDimension, string]> = [
  ["cli", "CLI"],
  ["llm", "Provider"],
  ["models", "Model"],
];

const STORAGE_KEY = "quill-explore-state";
/** Metrics off by default so the first paint is not twelve-line spaghetti. */
const DEFAULT_HIDDEN = ["m:tokloc", "m:lochr", "m:projects"];

interface StoredState {
  range: ExploreRange;
  scale: ExploreScale;
  smooth: boolean;
  dimension: WidgetChartDimension;
  hidden: string[];
}

function loadStored(): StoredState {
  const fallback: StoredState = {
    range: "30d",
    scale: "norm",
    smooth: false,
    dimension: "models",
    hidden: DEFAULT_HIDDEN,
  };
  try {
    const raw = JSON.parse(localStorage.getItem(STORAGE_KEY) ?? "null") as
      | Partial<StoredState>
      | null;
    if (!raw) return fallback;
    return {
      range: RANGES.some(([id]) => id === raw.range) ? (raw.range as ExploreRange) : fallback.range,
      scale: SCALES.some(([id]) => id === raw.scale) ? (raw.scale as ExploreScale) : fallback.scale,
      smooth: raw.smooth === true,
      dimension: DIMENSIONS.some(([id]) => id === raw.dimension)
        ? (raw.dimension as WidgetChartDimension)
        : fallback.dimension,
      hidden: Array.isArray(raw.hidden) ? raw.hidden.filter((id) => typeof id === "string") : fallback.hidden,
    };
  } catch {
    return fallback;
  }
}

/** Trailing moving average, window 7 — reads month-scale trends. */
function movingAvg(values: readonly number[], window: number): number[] {
  if (window <= 1) return [...values];
  return values.map((_, index) => {
    const from = Math.max(0, index - window + 1);
    let sum = 0;
    for (let cursor = from; cursor <= index; cursor += 1) sum += values[cursor];
    return sum / (index - from + 1);
  });
}

function pearson(a: readonly number[], b: readonly number[]): number {
  const length = Math.min(a.length, b.length);
  if (length === 0) return 0;
  const meanA = a.reduce((sum, value) => sum + value, 0) / length;
  const meanB = b.reduce((sum, value) => sum + value, 0) / length;
  let numerator = 0;
  let denomA = 0;
  let denomB = 0;
  for (let index = 0; index < length; index += 1) {
    numerator += (a[index] - meanA) * (b[index] - meanB);
    denomA += (a[index] - meanA) ** 2;
    denomB += (b[index] - meanB) ** 2;
  }
  const denominator = Math.sqrt(denomA * denomB);
  return denominator === 0 ? 0 : numerator / denominator;
}

interface PanelSeries extends ExploreChartSeries {
  group: "tokens" | "metrics";
  /** Range aggregate for the panel's value column; null renders an em dash. */
  total: string | null;
}

interface SegProps<T extends string> {
  options: ReadonlyArray<[T, string]>;
  value: T;
  onChange: (value: T) => void;
  label: string;
}

function Seg<T extends string>({ options, value, onChange, label }: SegProps<T>) {
  return (
    <div className="explore-seg" role="group" aria-label={label}>
      {options.map(([id, text]) => (
        <button
          key={id}
          type="button"
          aria-pressed={id === value}
          onClick={() => onChange(id)}
        >
          {text}
        </button>
      ))}
    </div>
  );
}

function ExploreWindowView() {
  const [stored] = useState(loadStored);
  const [range, setRange] = useState<ExploreRange>(stored.range);
  const [scale, setScale] = useState<ExploreScale>(stored.scale);
  const [smooth, setSmooth] = useState(stored.smooth);
  const [dimension, setDimension] = useState<WidgetChartDimension>(stored.dimension);
  const [hidden, setHidden] = useState<ReadonlySet<string>>(new Set(stored.hidden));
  /** Selection before the last solo, so a solo is one click to undo. */
  const [prevHidden, setPrevHidden] = useState<ReadonlySet<string> | null>(null);
  const [emphasized, setEmphasized] = useState<string[] | null>(null);

  const { data, loading, error } = useExploreSeries(range);
  const days = EXPLORE_RANGE_DAYS[range];

  useEffect(() => {
    try {
      const state: StoredState = {
        range,
        scale,
        smooth,
        dimension,
        hidden: [...hidden],
      };
      localStorage.setItem(STORAGE_KEY, JSON.stringify(state));
    } catch {
      /* ignore */
    }
  }, [range, scale, smooth, dimension, hidden]);

  const allSeries = useMemo<PanelSeries[]>(() => {
    if (!data) return [];
    const tokens = chartSeriesFor(
      data.activity ?? undefined,
      dimension,
      providerTag,
      providerHex,
    ).series.map<PanelSeries>((entry) => ({
      id: `tok:${entry.id}`,
      label:
        dimension === "models"
          ? formatSessionModel(entry.provider as IntegrationProvider, entry.label)
          : entry.label,
      color: entry.color,
      values: entry.values,
      format: formatTokenCount,
      group: "tokens",
      total: formatTokenCount(
        entry.values.reduce((sum, value) => sum + value, 0),
      ),
    }));

    const { totals, metrics } = data;
    const runtimeHours = (totals.runtimeSecs ?? 0) / 3600;
    const metricTotal = (key: MetricDef["key"]): string | null => {
      switch (key) {
        case "runtime":
          return totals.runtimeSecs === null ? null : formatDurationSecs(totals.runtimeSecs);
        case "tokPerLoc":
          return totals.changedLines > 0
            ? formatNumber(Math.round(totals.tokens / totals.changedLines))
            : null;
        case "locPerHour":
          return runtimeHours > 0
            ? formatNumber(Math.round(totals.changedLines / runtimeHours))
            : formatNumber(Math.round(totals.changedLines / (days * 24)));
        case "sessions":
          return formatNumber(totals.sessionCount);
        case "projects":
          return `≤${formatNumber(Math.max(0, ...metrics.projects))}/d`;
        case "netLines":
          return fmtSignedLines(totals.netLines);
      }
    };
    return [
      ...tokens,
      ...METRICS.map<PanelSeries>((def) => ({
        id: def.id,
        label: def.label,
        color: def.color,
        values: metrics[def.key],
        format: def.format,
        group: "metrics",
        total: metricTotal(def.key),
      })),
    ];
  }, [data, dimension, days]);

  const visible = useMemo(
    () =>
      allSeries
        .filter((entry) => !hidden.has(entry.id))
        .map((entry) => ({
          ...entry,
          values: smooth ? movingAvg(entry.values, 7) : entry.values,
        })),
    [allSeries, hidden, smooth],
  );

  const correlations = useMemo(() => {
    const pairs: Array<{ a: PanelSeries; b: PanelSeries; r: number }> = [];
    for (let left = 0; left < visible.length; left += 1) {
      for (let right = left + 1; right < visible.length; right += 1) {
        pairs.push({
          a: visible[left],
          b: visible[right],
          r: pearson(visible[left].values, visible[right].values),
        });
      }
    }
    return pairs
      .filter((pair) => Number.isFinite(pair.r))
      .sort((left, right) => Math.abs(right.r) - Math.abs(left.r))
      .slice(0, 4);
  }, [visible]);

  const toggleSeries = (id: string, solo: boolean) => {
    setPrevHidden(solo ? hidden : null);
    setHidden((current) => {
      if (solo) {
        return new Set(allSeries.map((entry) => entry.id).filter((other) => other !== id));
      }
      const next = new Set(current);
      if (next.has(id)) next.delete(id);
      else next.add(id);
      return next;
    });
  };

  const soloPair = (a: string, b: string) => {
    setPrevHidden(hidden);
    setHidden(
      new Set(
        allSeries.map((entry) => entry.id).filter((id) => id !== a && id !== b),
      ),
    );
  };

  const setGroup = (group: PanelSeries["group"], on: boolean) => {
    setPrevHidden(null);
    setHidden((current) => {
      const next = new Set(current);
      for (const entry of allSeries) {
        if (entry.group !== group) continue;
        if (on) next.delete(entry.id);
        else next.add(entry.id);
      }
      return next;
    });
  };

  const restorePrev = () => {
    if (prevHidden === null) return;
    setHidden(prevHidden);
    setPrevHidden(null);
  };

  const totalTokens = data?.totals.tokens ?? 0;
  const scaleNote =
    scale === "norm"
      ? "each line scaled to its own min–max"
      : "each line relative to its own average";

  const groups: ReadonlyArray<[PanelSeries["group"], string]> = [
    ["tokens", dimension === "cli" ? "Tokens by CLI" : dimension === "llm" ? "Tokens by provider" : "Tokens by model"],
    ["metrics", "Metrics"],
  ];

  return (
    <div className="explore-root">
      <header className="explore-head">
        <div className="explore-title">
          <h1>Explore</h1>
          <p>every metric on one graph — toggle, rescale, correlate</p>
        </div>
        <div className="explore-controls">
          <span className="explore-control-label">TOKENS BY</span>
          <Seg options={DIMENSIONS} value={dimension} onChange={setDimension} label="Token grouping" />
          <span className="explore-control-label">RANGE</span>
          <Seg options={RANGES} value={range} onChange={setRange} label="Time range" />
          <span className="explore-control-label">SCALE</span>
          <Seg options={SCALES} value={scale} onChange={setScale} label="Scale mode" />
          <span className="explore-control-label">SMOOTH</span>
          <Seg
            options={[["off", "Off"], ["7d", "7d avg"]] as const}
            value={smooth ? "7d" : "off"}
            onChange={(value) => setSmooth(value === "7d")}
            label="Smoothing"
          />
        </div>
      </header>

      <div className="explore-layout">
        <section className="explore-card explore-chart-card" aria-label="Overlay chart">
          <div className="explore-chart-head" aria-live="polite">
            <span className="explore-chart-total">{formatTokenCount(totalTokens)}</span>
            <span className="explore-chart-sub">
              tokens · {days}d · {visible.length} series
            </span>
            <span className="explore-chart-note">{scaleNote}</span>
          </div>

          {loading && !data ? (
            <div className="explore-chart explore-chart-skeleton" aria-hidden="true" />
          ) : error && !data ? (
            <div className="explore-state" role="alert">
              Usage data unavailable — {error}
            </div>
          ) : (
            <>
              <ExploreChart
                grid={data?.grid ?? []}
                series={visible}
                scale={scale}
                emphasized={emphasized}
              />
              {visible.length === 0 && (
                <div className="explore-empty">
                  <p>No series selected — pick some on the right, or</p>
                  <button
                    type="button"
                    onClick={() => {
                      setPrevHidden(null);
                      setHidden(new Set());
                    }}
                  >
                    Show all series
                  </button>
                </div>
              )}
            </>
          )}

          <div className="explore-corr">
            <span className="explore-corr-key">
              STRONGEST CORRELATIONS · {days}D
            </span>
            {visible.length < 2 ? (
              <span className="explore-corr-hint">
                turn on two or more series to compare
              </span>
            ) : (
              correlations.map((pair) => (
                <button
                  key={`${pair.a.id}|${pair.b.id}`}
                  type="button"
                  className="explore-corr-pair"
                  title="Click to view just these two series"
                  onClick={() => soloPair(pair.a.id, pair.b.id)}
                  onMouseEnter={() => setEmphasized([pair.a.id, pair.b.id])}
                  onMouseLeave={() => setEmphasized(null)}
                  onFocus={() => setEmphasized([pair.a.id, pair.b.id])}
                  onBlur={() => setEmphasized(null)}
                >
                  <i style={{ background: pair.a.color }} />
                  {pair.a.label}
                  <span className="explore-corr-x">×</span>
                  <i style={{ background: pair.b.color }} />
                  {pair.b.label}
                  <span className="explore-corr-r" data-neg={pair.r < 0 ? "true" : undefined}>
                    {pair.r.toFixed(2)}
                  </span>
                </button>
              ))
            )}
            {prevHidden !== null && (
              <button
                type="button"
                className="explore-corr-restore"
                title="Bring back the series that were visible before the solo"
                onClick={restorePrev}
              >
                ↩ restore selection
              </button>
            )}
          </div>
        </section>

        <aside className="explore-card explore-panel" aria-label="Series visibility">
          {groups.map(([group, title]) => (
            <div className="explore-group" key={group}>
              <h2>
                {title}
                <span className="explore-group-actions">
                  <button type="button" onClick={() => setGroup(group, true)}>
                    all
                  </button>
                  <button type="button" onClick={() => setGroup(group, false)}>
                    none
                  </button>
                </span>
              </h2>
              {allSeries
                .filter((entry) => entry.group === group)
                .map((entry) => {
                  const on = !hidden.has(entry.id);
                  return (
                    <button
                      key={entry.id}
                      type="button"
                      className="explore-series-row"
                      data-on={on ? "true" : "false"}
                      aria-pressed={on}
                      onClick={(event) => toggleSeries(entry.id, event.altKey)}
                      onMouseEnter={() => setEmphasized([entry.id])}
                      onMouseLeave={() => setEmphasized(null)}
                    >
                      <i className="explore-swatch" style={{ background: entry.color }} />
                      <span className="explore-series-label">{entry.label}</span>
                      <span className="explore-series-total">
                        {entry.total ?? "—"}
                      </span>
                    </button>
                  );
                })}
              {group === "tokens" && allSeries.every((entry) => entry.group !== "tokens") && (
                <p className="explore-group-empty">No token evidence in this range</p>
              )}
            </div>
          ))}
          <p className="explore-panel-hint">
            Click toggles · <kbd>⌥ click</kbd> solos · hover highlights ·
            click the chart to pin a day
          </p>
        </aside>
      </div>
    </div>
  );
}

export default ExploreWindowView;
