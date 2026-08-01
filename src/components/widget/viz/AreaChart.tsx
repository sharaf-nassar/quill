// AreaChart — the widget's hero chart: one smoothed area per provider, an
// overlay slot for the headline value, and a legend chip that stays hidden
// until the chart is hovered or focused (the providers are already named in
// the LIMITS section, so a permanent legend would be redundant ink).

import { useId, type ReactNode } from "react";
import { formatTokenCount } from "../../../utils/tokens";
import { areaPath, scalePoints, seriesMax, seriesTotal, smoothPath } from "./geometry";

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

  const plotted = series.map((entry) => ({
    entry,
    points: scalePoints(entry.values, {
      width,
      baseline: baseline - BASELINE_GAP,
      plotHeight: baseline * headroom,
      max,
    }),
  }));

  return (
    <div className={classes}>
      {overlay && (
        <div className="viz-chart-overlay">
          <div className="viz-chart-overlay-inner">{overlay}</div>
        </div>
      )}
      <div className="viz-chart-legend" data-visible={legendVisible ? "true" : undefined}>
        {series.map((entry) => (
          <span className="viz-chart-legend-row" key={entry.id}>
            <i className="viz-chart-legend-swatch" style={{ background: entry.color }} />
            {entry.label}
            <b className="viz-chart-legend-value">{formatTotal(seriesTotal(entry.values))}</b>
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
            fill="var(--faint)"
            fontSize={7.5}
            fontWeight={500}
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
        {plotted.map(({ entry, points }) => {
          const endpoint = points.length > 0 ? points[points.length - 1] : null;
          if (!endpoint) return null;
          return (
            <circle
              key={`${entry.id}-endpoint`}
              cx={endpoint.x - 1.5}
              cy={endpoint.y}
              r={2.4}
              fill={entry.color}
              stroke="var(--surface)"
              strokeWidth={1.5}
            />
          );
        })}
      </svg>
    </div>
  );
}

export default AreaChart;
