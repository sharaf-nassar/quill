// Sparkline — the 13px trend line under each readout cell in the metric grid.
// Stroke, endpoint, and label swatch all carry the metric's fixed hue; the
// value itself stays --text-hi (DESIGN.md colour law).

import { scalePoints, smoothPath } from "./geometry";

const DEFAULT_WIDTH = 100;
const DEFAULT_HEIGHT = 13;
const PAD_START = 2;
const PAD_END = 4;

export interface SparklineProps {
  /** Ordered bucket values for the selected range. */
  values: readonly number[];
  /** Metric hue — a CSS colour or `var(--metric-*)` reference. */
  color: string;
  /** viewBox height in px; the element always fills its container's width. */
  height?: number;
  /** viewBox width in px. Only affects curve resolution, not layout. */
  width?: number;
  /**
   * Accessible label. Omit it when an adjacent value already states the
   * number — the sparkline is then decorative and hidden from the tree.
   */
  label?: string;
  className?: string;
}

/** A bare trend line: no axes, no grid, no ticks — shape only. */
function Sparkline({
  values,
  color,
  height = DEFAULT_HEIGHT,
  width = DEFAULT_WIDTH,
  label,
  className,
}: SparklineProps) {
  const classes = className ? `viz-sparkline ${className}` : "viz-sparkline";
  const labelProps = label
    ? { role: "img" as const, "aria-label": label }
    : { "aria-hidden": true as const };

  const min = values.length > 0 ? Math.min(...values) : 0;
  const max = values.length > 0 ? Math.max(...values) : 0;
  const points = scalePoints(values, {
    width,
    baseline: height - 2.5,
    plotHeight: height - 5,
    padStart: PAD_START,
    padEnd: PAD_END,
    min,
    max,
  });
  const endpoint = points.length > 0 ? points[points.length - 1] : null;

  return (
    <svg
      className={classes}
      viewBox={`0 0 ${width} ${height}`}
      preserveAspectRatio="none"
      style={{ height: `${height}px` }}
      {...labelProps}
    >
      {points.length > 1 && (
        <path
          d={smoothPath(points)}
          fill="none"
          stroke={color}
          strokeOpacity={0.6}
          strokeWidth={1.3}
          strokeLinecap="round"
        />
      )}
      {endpoint && <circle cx={endpoint.x} cy={endpoint.y} r={1.7} fill={color} />}
    </svg>
  );
}

export default Sparkline;
