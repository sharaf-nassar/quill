// TrendsView — the widget's slow instrument: three week-over-week rows.
//
// Tokens, velocity and cache efficiency, each as a metric name, this week's
// value, the delta against last week, and the paired mini-bars that are the
// delta's evidence. Nothing here is re-scopable: the windows are the last seven
// days and the seven before them, because a trend you can narrow to one hour is
// a reading, not a trend. The view region hides its range strip while Trends is
// showing rather than leaving a control that would do nothing.
//
// The rules the file obeys:
//
//   - **Both sides or neither.** A delta renders only when both weeks have a
//     figure; a percentage against an absent week is invented movement.
//   - **Colour means something.** The delta is green or red by *meaning*
//     (`InsightTrend.upIsGood`), so rising velocity reads as a gain while a
//     token count — neither good nor bad — stays neutral. The bars carry no
//     hue at all: this week is bright, last week is dim.
//   - **Absent, never zero.** A week whose code rows fall below the retention
//     watermark renders as an em dash with the disclosure beneath, because a
//     pruned week drawn as a short bar would read as a collapse in output.
//
// See specs/018-widget-ui-redesign/plan.md#Affected Components.

import { Bars, type VizBar } from "../viz";
import { useRetentionCutoff } from "../../../hooks/useRetentionCutoff";
import {
  useWeeklyTrends,
  type WeeklyMetric,
  type WeeklyTrends,
} from "../../../hooks/useWeeklyTrends";
import { formatNumber } from "../../../utils/format";
import { PRUNED_PLACEHOLDER, formatRetentionCutoff } from "../../../utils/retention";
import { formatTokenCount } from "../../../utils/tokens";
import type { InsightTrend } from "../../../types";

/** Label column of the bar pair — wide enough for `This wk` at 10px. */
const BAR_LABEL_WIDTH = 46;

interface TrendRow {
  readonly id: string;
  readonly name: string;
  /** Dim qualifier after the name, e.g. the unit the value is counted in. */
  readonly unit: string | null;
  readonly metric: WeeklyMetric;
  readonly format: (value: number) => string;
  /** Why this row can go missing, shown on hover. */
  readonly title: string;
}

interface Delta {
  readonly text: string;
  readonly tone: "positive" | "negative" | "flat";
}

/**
 * A week-over-week move rendered by meaning rather than by arrow direction: a
 * trend whose goodness is unknown stays neutral instead of borrowing green.
 */
function delta(trend: InsightTrend | null): Delta | null {
  if (!trend) return null;
  if (trend.direction === "flat") return { text: "— 0%", tone: "flat" };
  const rising = trend.direction === "up";
  const tone =
    trend.upIsGood === null
      ? "flat"
      : trend.upIsGood === rising
        ? "positive"
        : "negative";
  return { text: `${rising ? "▲" : "▼"} ${trend.percentage}%`, tone };
}

/** `Jul 24 – Jul 31` for one of the compared windows. */
function windowLabel(fromIso: string, toIso: string): string {
  const day = (iso: string) => {
    const parsed = new Date(iso);
    if (Number.isNaN(parsed.getTime())) return "—";
    return parsed.toLocaleDateString(undefined, { month: "short", day: "numeric" });
  };
  return `${day(fromIso)} – ${day(toIso)}`;
}

/**
 * The bar pair. Both weeks share one scale so the lengths are comparable, and a
 * missing week keeps its track — an omitted row would hide that the comparison
 * has a side at all.
 */
function weekBars(metric: WeeklyMetric, format: (value: number) => string): VizBar[] {
  const toBar = (id: string, label: string, value: number | null): VizBar => ({
    id,
    label,
    value: value ?? 0,
    valueLabel: value === null ? PRUNED_PLACEHOLDER : format(value),
  });
  return [
    toBar("current", "This wk", metric.current),
    toBar("previous", "Last wk", metric.previous),
  ];
}

function TrendRowItem({ row }: { row: TrendRow }) {
  const change = delta(row.metric.trend);
  const hasFigure = row.metric.current !== null || row.metric.previous !== null;
  const scale = Math.max(row.metric.current ?? 0, row.metric.previous ?? 0);

  return (
    <li className="wg-trend">
      <div className="wg-trend-head">
        <span className="wg-trend-name">{row.name}</span>
        {row.unit && <span className="wg-trend-unit">{row.unit}</span>}
        <span className="wg-trend-value wg-num" title={row.title}>
          {row.metric.current === null ? PRUNED_PLACEHOLDER : row.format(row.metric.current)}
          {change && (
            <span
              className="wg-trend-delta"
              data-tone={change.tone}
              title="This week against the seven days before it"
            >
              {change.text}
            </span>
          )}
        </span>
      </div>
      {hasFigure && (
        <Bars
          bars={weekBars(row.metric, row.format)}
          max={scale > 0 ? scale : undefined}
          labelWidth={BAR_LABEL_WIDTH}
          formatValue={row.format}
          className="wg-trend-bars"
        />
      )}
    </li>
  );
}

function buildRows(data: WeeklyTrends): TrendRow[] {
  return [
    {
      id: "tokens",
      name: "Tokens",
      unit: null,
      metric: data.tokens,
      format: formatTokenCount,
      title: "Total tokens recorded in each week",
    },
    {
      id: "velocity",
      name: "Velocity",
      unit: "LOC / hr",
      metric: data.velocity,
      format: formatNumber,
      title: "Lines changed per hour of active LLM runtime",
    },
    {
      id: "cache",
      name: "Cache efficiency",
      unit: "hit rate",
      metric: data.cache,
      format: (value: number) => `${value}%`,
      title: "Cache reads as a share of everything that could be served from cache",
    },
  ];
}

function TrendsView() {
  const retention = useRetentionCutoff();
  const trends = useWeeklyTrends(retention.cutoff);
  const data = trends.data;

  if (trends.error && !data) {
    return (
      <div className="wg-trends">
        <div className="wg-state wg-state-error">
          <span className="wg-state-lamp" aria-hidden="true" />
          Trends unavailable
        </div>
      </div>
    );
  }

  if (!data) {
    return (
      <div className="wg-trends">
        {/* Row-shaped rather than line-shaped, so the band lands close to its
            loaded height and the view does not jump when the figures arrive. */}
        <div className="wg-trends-pending" aria-hidden="true">
          <div className="wg-skeleton wg-trend-skeleton" />
          <div className="wg-skeleton wg-trend-skeleton" />
          <div className="wg-skeleton wg-trend-skeleton" />
        </div>
      </div>
    );
  }

  const rows = buildRows(data);
  const empty = rows.every(
    (row) => row.metric.current === null && row.metric.previous === null,
  );
  // Only velocity reads pruned tables, so the disclosure appears exactly when
  // one of the compared weeks sits at or below the watermark.
  const degraded =
    retention.cutoff !== null &&
    (data.velocitySpans.current !== "retained" ||
      data.velocitySpans.previous !== "retained");

  return (
    <div className="wg-trends">
      <p
        className="wg-trends-caption wg-num"
        title="Rolling seven-day windows ending now"
      >
        {windowLabel(data.currentStart, data.end)} vs{" "}
        {windowLabel(data.previousStart, data.currentStart)}
      </p>

      {empty ? (
        <div className="wg-state">
          <span className="wg-state-lamp" aria-hidden="true" />
          No activity in the last two weeks
        </div>
      ) : (
        <ul className="wg-trend-rows">
          {rows.map((row) => (
            <TrendRowItem key={row.id} row={row} />
          ))}
        </ul>
      )}

      {degraded && retention.cutoff && (
        <p
          className="wg-trend-note"
          role="note"
          title="Retention deletes tool activity and session events past the watermark. Token figures are unaffected — they come from snapshots retention never prunes."
        >
          Retention · code activity before {formatRetentionCutoff(retention.cutoff)} was
          pruned, so velocity for the compared weeks may be incomplete
        </p>
      )}
    </div>
  );
}

export default TrendsView;
