// Heat — a coarse density strip: one cell per bucket, intensity by opacity of
// a single hue. Used where a line would over-state precision (per-hour session
// activity, cache-hit density) but the shape still matters.

const DEFAULT_COLUMNS = 12;
/** Floor so an empty bucket still reads as a bucket, not a gap. */
const MIN_INTENSITY = 0.08;
const MAX_INTENSITY = 0.9;

export interface VizHeatCell {
  readonly id: string;
  /** Human description of the bucket, surfaced as the cell tooltip. */
  readonly label: string;
  readonly value: number;
}

export interface HeatProps {
  cells: readonly VizHeatCell[];
  /** Single hue for the whole strip — intensity carries the magnitude. */
  color: string;
  /** Shared scale; defaults to the largest cell value. */
  max?: number;
  columns?: number;
  /** Sentence describing what the strip shows, for assistive tech. */
  ariaLabel: string;
  emptyLabel?: string;
  className?: string;
}

function Heat({
  cells,
  color,
  max,
  columns = DEFAULT_COLUMNS,
  ariaLabel,
  emptyLabel = "No activity in this range",
  className,
}: HeatProps) {
  const classes = className ? `viz-heat ${className}` : "viz-heat";

  if (cells.length === 0) {
    return (
      <div className="wg-state wg-state-empty">
        <span className="wg-state-lamp" />
        <span>{emptyLabel}</span>
      </div>
    );
  }

  const ceiling = max ?? cells.reduce((peak, cell) => Math.max(peak, cell.value), 0);
  const scale = ceiling > 0 ? ceiling : 1;

  return (
    <div
      className={classes}
      role="img"
      aria-label={ariaLabel}
      style={{ gridTemplateColumns: `repeat(${columns}, 1fr)` }}
    >
      {cells.map((cell) => {
        const normalized = Math.min(Math.max(cell.value, 0), scale) / scale;
        const intensity = MIN_INTENSITY + normalized * (MAX_INTENSITY - MIN_INTENSITY);
        return (
          <span
            className="viz-heat-cell"
            key={cell.id}
            title={cell.label}
            style={{ background: color, opacity: intensity }}
          />
        );
      })}
    </div>
  );
}

export default Heat;
