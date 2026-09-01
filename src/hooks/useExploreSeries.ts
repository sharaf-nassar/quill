// Data feed for the Usage Explorer: every metric on one shared daily grid.
//
// The explorer overlays token usage and the six readout metrics over weeks or
// months, so everything here is answered at *daily* resolution on one grid —
// the model overview's bucket starts (24h periods ending now). Sources that
// return raw points (token history, code stats history) are re-bucketed onto
// that grid client-side, the same way the widget's net-lines readout already
// buckets code history. Sessions and projects come from `get_activity_series`
// with one bucket per day on the identical window.
//
// Runtime is the one series without a daily source: `get_llm_runtime_stats`
// answers a fixed 7-bucket sparkline for any range. Each bucket is spread
// across its days as an average, and the series is labelled "avg/day" so a
// 30d/90d line never pretends to daily evidence it does not have.

import { useCallback } from "react";
import { invoke } from "@tauri-apps/api/core";
import { useCachedInvoke } from "./useCachedInvoke";
import type {
  ActivitySeriesResponse,
  CodeStatsHistoryPoint,
  LlmRuntimeStats,
  ModelActivity,
  ModelUsageOverviewResponse,
  TokenDataPoint,
} from "../types";

/** Ranges the explorer offers; each maps to daily buckets in the backend. */
export type ExploreRange = "7d" | "30d" | "90d";

export const EXPLORE_RANGE_DAYS: Record<ExploreRange, number> = {
  "7d": 7,
  "30d": 30,
  "90d": 90,
};

export interface ExploreMetricSeries {
  /** Seconds of active LLM runtime per day (bucket average — see header). */
  runtime: number[];
  /** Total tokens ÷ changed lines per day; 0 where no lines changed. */
  tokPerLoc: number[];
  /** Changed lines per wall-clock hour per day. */
  locPerHour: number[];
  /** Distinct sessions per day. */
  sessions: number[];
  /** Distinct projects per day. */
  projects: number[];
  /** lines_added − lines_removed per day. */
  netLines: number[];
}

export interface ExploreSeriesData {
  /** ISO bucket starts, oldest first — the shared x-axis. */
  grid: string[];
  /** Raw model activity for the token overlay; grouped by the view. */
  activity: ModelActivity | null;
  metrics: ExploreMetricSeries;
  /** Range aggregates for the series panel. */
  totals: {
    runtimeSecs: number | null;
    sessionCount: number;
    tokens: number;
    changedLines: number;
    netLines: number;
  };
}

export interface ExploreSeriesResult {
  data: ExploreSeriesData | null;
  loading: boolean;
  error: string | null;
}

/** Index of the day bucket a timestamp falls into, clamped onto the grid. */
function dayIndex(timestampMs: number, startMs: number, days: number): number {
  const index = Math.floor((timestampMs - startMs) / (24 * 60 * 60 * 1000));
  return Math.max(0, Math.min(days - 1, index));
}

function assemble(
  days: number,
  overview: ModelUsageOverviewResponse,
  activitySeries: ActivitySeriesResponse,
  runtime: LlmRuntimeStats,
  tokenHistory: TokenDataPoint[],
  codeHistory: CodeStatsHistoryPoint[],
): ExploreSeriesData {
  const grid = overview.activity.bucketStarts;
  const startMs = new Date(grid[0] ?? Date.now()).getTime();

  const tokensPerDay = new Array<number>(days).fill(0);
  for (const point of tokenHistory) {
    const ts = new Date(point.timestamp).getTime();
    if (!Number.isFinite(ts) || ts < startMs) continue;
    tokensPerDay[dayIndex(ts, startMs, days)] += point.total_tokens;
  }

  const locPerDay = new Array<number>(days).fill(0);
  const netPerDay = new Array<number>(days).fill(0);
  for (const point of codeHistory) {
    const ts = new Date(point.timestamp).getTime();
    if (!Number.isFinite(ts) || ts < startMs) continue;
    const index = dayIndex(ts, startMs, days);
    locPerDay[index] += point.total_changed;
    netPerDay[index] += point.lines_added - point.lines_removed;
  }

  // Spread each coarse runtime bucket over its days as a per-day average.
  // ponytail: runtime sparkline is a fixed 7-bucket backend grid; a per-day
  // backend series is the upgrade path if daily runtime evidence matters.
  const buckets = runtime.sparkline;
  const runtimePerDay = new Array<number>(days).fill(0);
  if (buckets.length > 0) {
    const daysPerBucket = days / buckets.length;
    for (let day = 0; day < days; day += 1) {
      const bucket = Math.min(
        buckets.length - 1,
        Math.floor((day * buckets.length) / days),
      );
      runtimePerDay[day] = (buckets[bucket] ?? 0) / daysPerBucket;
    }
  }

  const pad = (values: number[]) => {
    const out = values.slice(0, days);
    while (out.length < days) out.push(0);
    return out;
  };

  return {
    grid,
    activity: overview.activity,
    metrics: {
      runtime: runtimePerDay,
      tokPerLoc: tokensPerDay.map((tokens, index) =>
        locPerDay[index] > 0 ? Math.round(tokens / locPerDay[index]) : 0,
      ),
      locPerHour: locPerDay.map((loc) => Math.round(loc / 24)),
      sessions: pad(activitySeries.session_counts),
      projects: pad(activitySeries.project_counts),
      netLines: netPerDay,
    },
    totals: {
      runtimeSecs: runtime.turn_count > 0 ? runtime.total_runtime_secs : null,
      sessionCount: runtime.session_count,
      tokens: tokensPerDay.reduce((sum, value) => sum + value, 0),
      changedLines: locPerDay.reduce((sum, value) => sum + value, 0),
      netLines: netPerDay.reduce((sum, value) => sum + value, 0),
    },
  };
}

export function useExploreSeries(range: ExploreRange): ExploreSeriesResult {
  const request = useCallback(async (): Promise<ExploreSeriesData> => {
    const days = EXPLORE_RANGE_DAYS[range];
    const [overview, activitySeries, runtime, tokenHistory, codeHistory] =
      await Promise.all([
        invoke<ModelUsageOverviewResponse>("get_model_usage_overview", {
          range,
          provider: null,
        }),
        invoke<ActivitySeriesResponse>("get_activity_series", {
          range,
          buckets: days,
        }),
        invoke<LlmRuntimeStats>("get_llm_runtime_stats", { range }),
        invoke<TokenDataPoint[]>("get_token_history", {
          range,
          hostname: null,
          sessionId: null,
          cwd: null,
        }),
        invoke<CodeStatsHistoryPoint[]>("get_code_stats_history", { range }),
      ]);
    return assemble(
      days,
      overview,
      activitySeries,
      runtime,
      tokenHistory,
      codeHistory,
    );
  }, [range]);

  const { state } = useCachedInvoke({
    command: "explore_series",
    args: { range },
    request,
    normalizeError: String,
    onError: (error) => console.error("Explore series fetch error:", error),
    invalidationEvents: [
      "tokens-updated",
      "sessions-index-updated",
      "transcript-analytics-updated",
      "model-analytics-updated",
    ],
    pollMs: 60_000,
  });

  return {
    data: state.data,
    loading: state.initialLoading,
    error: state.error,
  };
}
