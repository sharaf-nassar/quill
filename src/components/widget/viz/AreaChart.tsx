// AreaChart — the widget's hero chart: one smoothed area per provider, an
// overlay slot for the headline value, and a legend chip that stays hidden
// until the chart is hovered or focused (the providers are already named in
// the LIMITS section, so a permanent legend would be redundant ink).

import {
  useCallback,
  useEffect,
  useId,
  useRef,
  useState,
  type ReactNode,
} from "react";
import { formatTokenCount } from "../../../utils/tokens";
import {
  areaPath,
  bucketIndexAtPosition,
  legendPositionAtPosition,
  scalePoints,
  seriesMax,
  seriesTotal,
  smoothPath,
  type LegendPosition,
} from "./geometry";

const DEFAULT_WIDTH = 332;
const DEFAULT_HEIGHT = 118;
/** Space reserved below the plot for the x-axis labels. */
const AXIS_HEIGHT = 16;
/** Gap between the zero line and a zero-valued point, so flat series read. */
const BASELINE_GAP = 4;
/**
 * Fraction of the plot the tallest value may occupy. Keeping the series in the
 * lower ~60% is what lets the headline sit over the chart without collision.
 */
const DEFAULT_HEADROOM = 0.62;
const DEFAULT_GRIDLINES = [0.5, 0.78] as const;

/** Series ids are caller-supplied; strip anything a fragment reference rejects. */
function gradientId(prefix: string, seriesId: string): string {
  return `${prefix}-${seriesId.replace(/\W/g, "")}`;
}

export interface VizSeries {
  readonly id: string;
  /** Short uppercase name shown in the hover chip. */
  readonly label: string;
  /** Provider hue — a CSS colour or `var(--provider-*)` reference. */
  readonly color: string;
  readonly values: readonly number[];
  /** Peak opacity of the area fill; lines never change weight. */
  readonly fillOpacity?: number;
}

export interface AreaChartProps {
  series: readonly VizSeries[];
  /** Evenly spaced axis captions, e.g. hour marks. */
  xLabels?: readonly string[];
  height?: number;
  width?: number;
  headroom?: number;
  /** Gridline positions as a fraction of the baseline height. */
  gridlines?: readonly number[];
  /** Rendered over the chart's top-left with the surface fade treatment. */
  overlay?: ReactNode;
  /** Force the hover chip open — used by keyboard and screenshot passes. */
  legendVisible?: boolean;
  /** Formats the per-series totals in the hover chip. */
  formatTotal?: (value: number) => string;
  /** Sentence describing the chart and its totals for assistive tech. */
  ariaLabel: string;
  /** Shown in place of the chart when no series carries a value. */
  emptyLabel?: string;
  className?: string;
}

function AreaChart({
  series,
  xLabels,
  height = DEFAULT_HEIGHT,
  width = DEFAULT_WIDTH,
  headroom = DEFAULT_HEADROOM,
  gridlines = DEFAULT_GRIDLINES,
  overlay,
  legendVisible,
  formatTotal = formatTokenCount,
  ariaLabel,
  emptyLabel = "No activity in this range",
  className,
}: AreaChartProps) {
  // useId() emits punctuation that is illegal inside a `url(#…)` reference, so
  // the gradient ids are reduced to word characters before they reach the DOM.
  const gradientPrefix = `viz${useId().replace(/\W/g, "")}`;
  const classes = className ? `viz-chart ${className}` : "viz-chart";
  const baseline = height - AXIS_HEIGHT;
  const max = seriesMax(series.map((entry) => entry.values));
  const buckets = series.reduce((count, entry) => Math.max(count, entry.values.length), 0);
  const [scrub, setScrub] = useState<{
    bucket: number;
    legend: LegendPosition | null;
  } | null>(null);
  const legendRef = useRef<HTMLDivElement>(null);
  const helpId = `${gradientPrefix}-help`;
  const plotted = series.map((entry) => ({
    entry,
    points: scalePoints(entry.values, {
      width,
      baseline: baseline - BASELINE_GAP,
      plotHeight: baseline * headroom,
      max,
    }),
  }));

  useEffect(() => setScrub(null), [xLabels]);

  const handlePointer = useCallback(
    (event: React.PointerEvent<HTMLDivElement>) => {
      const rect = event.currentTarget.getBoundingClientRect();
      const legendRect = legendRef.current?.getBoundingClientRect();
      const x = event.clientX - rect.left;
      const bucket = bucketIndexAtPosition(x, rect.width, buckets);
      setScrub(
        bucket === null
          ? null
          : {
              bucket,
              legend: legendRect
                ? legendPositionAtPosition(
                    x,
                    event.clientY - rect.top,
                    rect.width,
                    rect.height,
                    legendRect.width,
                    legendRect.height,
                  )
                : null,
            },
      );
    },
    [buckets],
  );

  const handleKeyDown = useCallback(
    (event: React.KeyboardEvent<HTMLDivElement>) => {
      if (buckets === 0) return;
      const current = scrub?.bucket ?? buckets - 1;
      let next: number | null = null;
      if (event.key === "ArrowRight") {
        next = Math.min(current + 1, buckets - 1);
      } else if (event.key === "ArrowLeft") {
        next = Math.max(current - 1, 0);
      } else if (event.key === "Home") {
        next = 0;
      } else if (event.key === "End") {
        next = buckets - 1;
      } else if (event.key === "Escape") {
        setScrub(null);
      }

      if (next !== null) {
        event.preventDefault();
        const rect = event.currentTarget.getBoundingClientRect();
        const legendRect = legendRef.current?.getBoundingClientRect();
        const point = plotted.find(({ points }) => points[next])?.points[next];
        setScrub({
          bucket: next,
          legend:
            legendRect && point
              ? legendPositionAtPosition(
                  (point.x / width) * rect.width,
                  (point.y / height) * rect.height,
                  rect.width,
                  rect.height,
                  legendRect.width,
                  legendRect.height,
                )
              : null,
        });
      }
    },
    [buckets, height, plotted, scrub, width],
  );

  if (max <= 0) {
    return (
      <div className={classes}>
        <div className="wg-state wg-state-empty" style={{ minHeight: `${height}px` }}>
          <span className="wg-state-lamp" />
          <span>{emptyLabel}</span>
        </div>
      </div>
    );
  }

  const active = scrub !== null && scrub.bucket < buckets ? scrub.bucket : null;
  const crossX =
    active === null
      ? null
      : (plotted.find(({ points }) => points[active])?.points[active]?.x ?? 0);
  const legendPosition = active === null ? null : scrub?.legend;
  const announcement =
    active === null
      ? ""
      : `${xLabels?.[active] || `Bucket ${active + 1}`} — ${series
          .map((entry) => `${entry.label} ${formatTotal(entry.values[active] ?? 0)}`)
          .join(", ")}`;

  return (
    <div
      className={classes}
      role="group"
      aria-label={ariaLabel}
      aria-describedby={helpId}
      tabIndex={0}
      onPointerMove={handlePointer}
      onPointerLeave={() => setScrub(null)}
      onBlur={() => setScrub(null)}
      onKeyDown={handleKeyDown}
    >
      <p className="wg-sr" id={helpId}>
        Move the pointer across the chart, or use the left and right arrow keys, to read each
        time bucket. Escape clears the reading.
      </p>
      {overlay && (
        <div className="viz-chart-overlay">
          <div className="viz-chart-overlay-inner">{overlay}</div>
        </div>
      )}
      <div
        ref={legendRef}
        className="viz-chart-legend"
        data-side={legendPosition?.side}
        data-visible={legendVisible ? "true" : undefined}
        style={
          legendPosition
            ? { left: `${legendPosition.left}px`, top: `${legendPosition.top}px` }
            : undefined
        }
      >
        {series.map((entry) => (
          <span className="viz-chart-legend-row" key={entry.id}>
            <i className="viz-chart-legend-swatch" style={{ background: entry.color }} />
            {entry.label}
            <b className="viz-chart-legend-value">
              {formatTotal(active === null ? seriesTotal(entry.values) : (entry.values[active] ?? 0))}
            </b>
          </span>
        ))}
      </div>
      <svg
        className="viz-chart-svg"
        viewBox={`0 0 ${width} ${height}`}
        preserveAspectRatio="none"
        style={{ height: `${height}px` }}
        role="img"
        aria-label={ariaLabel}
      >
        <defs>
          {plotted.map(({ entry }) => (
            <linearGradient
              key={entry.id}
              id={gradientId(gradientPrefix, entry.id)}
              x1="0"
              y1="0"
              x2="0"
              y2="1"
            >
              <stop offset="0%" stopColor={entry.color} stopOpacity={entry.fillOpacity ?? 0.14} />
              <stop offset="100%" stopColor={entry.color} stopOpacity={0} />
            </linearGradient>
          ))}
        </defs>
        {gridlines.map((fraction) => (
          <line
            key={fraction}
            x1={0}
            y1={baseline * fraction}
            x2={width}
            y2={baseline * fraction}
            stroke="var(--line-soft)"
            strokeWidth={1}
          />
        ))}
        {xLabels?.map((label, index) => (
          <text
            key={`${label}-${index}`}
            x={((index + 1) * width) / (xLabels.length + 1)}
            y={height - 4}
            textAnchor="middle"
            fill={index === active ? "var(--text)" : "var(--faint)"}
            fontSize={7.5}
            fontWeight={index === active ? 700 : 500}
          >
            {label}
          </text>
        ))}
        {plotted.map(({ entry, points }) => (
          <path
            key={`${entry.id}-fill`}
            d={areaPath(points, baseline)}
            fill={`url(#${gradientId(gradientPrefix, entry.id)})`}
          />
        ))}
        {plotted.map(({ entry, points }) => (
          <path
            key={`${entry.id}-line`}
            d={smoothPath(points)}
            fill="none"
            stroke={entry.color}
            strokeWidth={1.6}
            strokeLinecap="round"
          />
        ))}
        {crossX !== null && (
          <line
            x1={crossX}
            y1={0}
            x2={crossX}
            y2={baseline}
            stroke="rgba(255,255,255,0.16)"
            strokeWidth={1}
          />
        )}
        {plotted.map(({ entry, points }) => {
          const point = active === null ? points[points.length - 1] : points[active];
          if (!point) return null;
          return (
            <circle
              key={`${entry.id}-${active === null ? "endpoint" : "active"}`}
              cx={active === null ? point.x - 1.5 : point.x}
              cy={point.y}
              r={2.4}
              fill={entry.color}
              stroke="var(--surface)"
              strokeWidth={1.5}
            />
          );
        })}
      </svg>
      <p className="wg-sr" role="status" aria-live="polite">
        {announcement}
      </p>
    </div>
  );
}

export default AreaChart;
