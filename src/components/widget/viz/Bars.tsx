// Bars — horizontal magnitude rows (Trends week pairs, Models session ranks,
// Hosts volume). Each track is a real `role="progressbar"`, so the value is
// announced rather than inferred from pixel width.

import { formatNumber } from "../../../utils/format";

const DEFAULT_LABEL_WIDTH = 76;

export interface VizBar {
  readonly id: string;
  readonly label: string;
  readonly value: number;
  /** Pre-formatted value text; defaults to a thousand-separated number. */
  readonly valueLabel?: string;
  /** Optional per-bar hue; defaults to the neutral --label fill. */
  readonly color?: string;
}

export interface BarsProps {
  bars: readonly VizBar[];
  /** Shared scale across every bar; defaults to the largest value present. */
  max?: number;
  /** Width of the label column in px, so tracks align into a table. */
  labelWidth?: number;
  formatValue?: (value: number) => string;
  /** Shown when there is nothing to rank. */
  emptyLabel?: string;
  className?: string;
}

function Bars({
  bars,
  max,
  labelWidth = DEFAULT_LABEL_WIDTH,
  formatValue = formatNumber,
  emptyLabel = "Nothing recorded in this range",
  className,
}: BarsProps) {
  const classes = className ? `viz-bars ${className}` : "viz-bars";

  if (bars.length === 0) {
    return (
      <div className="wg-state wg-state-empty">
        <span className="wg-state-lamp" />
        <span>{emptyLabel}</span>
      </div>
    );
  }

  const ceiling = max ?? bars.reduce((peak, bar) => Math.max(peak, bar.value), 0);
  const scale = ceiling > 0 ? ceiling : 1;

  return (
    <div className={classes}>
      {bars.map((bar) => {
        const clamped = Math.min(Math.max(bar.value, 0), scale);
        const percent = (clamped / scale) * 100;
        return (
          <div className="viz-bar-row" key={bar.id}>
            <span
              className="viz-bar-label"
              style={{ flex: `0 0 ${labelWidth}px`, width: `${labelWidth}px` }}
            >
              {bar.label}
            </span>
            <span
              className="viz-bar-track"
              role="progressbar"
              aria-label={bar.label}
              aria-valuenow={bar.value}
              aria-valuemin={0}
              aria-valuemax={scale}
              aria-valuetext={bar.valueLabel ?? formatValue(bar.value)}
            >
              <i
                className="viz-bar-fill"
                style={{ width: `${percent}%`, background: bar.color }}
              />
            </span>
            <span className="viz-bar-value">{bar.valueLabel ?? formatValue(bar.value)}</span>
          </div>
        );
      })}
    </div>
  );
}

export default Bars;
