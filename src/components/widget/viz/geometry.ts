// Shared SVG geometry for the widget viz kit. Pure functions only — every
// helper returns a new array or string and never mutates its inputs.
//
// The curve is the same catmull-rom → cubic-bezier construction used by
// specs/018-widget-ui-redesign/mockup.tpl.html, so the shipped charts trace the
// mockup's exact silhouette.

export interface VizPoint {
  readonly x: number;
  readonly y: number;
}

export interface ScaleOptions {
  /** viewBox width the points are laid out across. */
  readonly width: number;
  /** y coordinate of the zero line (values grow upward from here). */
  readonly baseline: number;
  /** Pixels a full-scale value consumes above the baseline. */
  readonly plotHeight: number;
  /** Left inset before the first point. */
  readonly padStart?: number;
  /** Right inset after the last point. */
  readonly padEnd?: number;
  /** Bottom of the value scale; defaults to 0 so magnitude reads honestly. */
  readonly min?: number;
  /** Top of the value scale; defaults to the series maximum. */
  readonly max?: number;
}

/** Trim coordinate noise so the emitted path stays small and diffable. */
function round(n: number): number {
  return Math.round(n * 100) / 100;
}

/**
 * Map a value series onto viewBox coordinates.
 *
 * Returns an empty array for an empty series. A single value is centred on the
 * left inset so a one-point series still renders its endpoint marker.
 */
export function scalePoints(
  values: readonly number[],
  options: ScaleOptions,
): VizPoint[] {
  if (values.length === 0) return [];

  const padStart = options.padStart ?? 0;
  const padEnd = options.padEnd ?? 0;
  const span = Math.max(0, options.width - padStart - padEnd);
  const min = options.min ?? 0;
  const max = options.max ?? Math.max(...values);
  const range = max - min || 1;
  const steps = values.length - 1;

  return values.map((value, index) => {
    const x = steps === 0 ? padStart : padStart + (index * span) / steps;
    const normalized = (value - min) / range;
    return {
      x: round(x),
      y: round(options.baseline - normalized * options.plotHeight),
    };
  });
}

/**
 * Catmull-rom smoothed path through every point.
 *
 * Returns an empty string for an empty series so callers can render the
 * element unconditionally without emitting an invalid `d`.
 */
export function smoothPath(points: readonly VizPoint[]): string {
  if (points.length === 0) return "";
  if (points.length === 1) return `M${points[0].x},${points[0].y}`;

  const segments: string[] = [`M${points[0].x},${points[0].y}`];
  for (let i = 0; i < points.length - 1; i++) {
    const p0 = points[Math.max(0, i - 1)];
    const p1 = points[i];
    const p2 = points[i + 1];
    const p3 = points[Math.min(points.length - 1, i + 2)];
    const c1x = round(p1.x + (p2.x - p0.x) / 6);
    const c1y = round(p1.y + (p2.y - p0.y) / 6);
    const c2x = round(p2.x - (p3.x - p1.x) / 6);
    const c2y = round(p2.y - (p3.y - p1.y) / 6);
    segments.push(`C${c1x},${c1y} ${c2x},${c2y} ${p2.x},${p2.y}`);
  }
  return segments.join("");
}

/** Closed fill under a smoothed line, dropped to `baseline` at both ends. */
export function areaPath(
  points: readonly VizPoint[],
  baseline: number,
): string {
  if (points.length < 2) return "";
  const line = smoothPath(points);
  const last = points[points.length - 1];
  const first = points[0];
  return `${line}L${last.x},${baseline} L${first.x},${baseline} Z`;
}

/** Largest value across every series, floored at zero. */
export function seriesMax(series: readonly (readonly number[])[]): number {
  let max = 0;
  for (const values of series) {
    for (const value of values) {
      if (value > max) max = value;
    }
  }
  return max;
}

/** Sum of a series; used for the hover chip's per-series totals. */
export function seriesTotal(values: readonly number[]): number {
  return values.reduce((sum, value) => sum + value, 0);
}
