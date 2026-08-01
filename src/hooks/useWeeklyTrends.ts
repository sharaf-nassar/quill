// Week-over-week figures behind the widget's Trends view.
//
// Three metrics, two windows each: the last seven days against the seven
// before them. The windows are fixed rather than range-driven because a
// "trend" the operator can re-scope to one hour is not a trend — Trends is the
// slow instrument next to Usage's live one.
//
// Every figure comes from a command the app already ships, on the formula the
// app already uses, so the numbers here agree with the surfaces that show them
// elsewhere: tokens and cache efficiency are the `get_token_history` sums
// behind `get_token_stats`, and velocity is
// [[src/hooks/useCodeInsights.ts#computeVelocity]] — the same LOC-per-active-
// hour definition the Usage readout prints (constitution #1: one source, one
// story).
//
// One approximation is inherited deliberately. `get_llm_runtime_stats` only
// accepts the fixed ranges, so it can measure "the last 7 days" exactly but
// never "the 7 days before that"; the prior week's active seconds are recovered
// by prorating the 30-day runtime sparkline, exactly as `useCodeInsights` does
// for its own comparison window.

import { useCallback, useEffect } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { useCachedInvoke } from "./useCachedInvoke";
import { activeSecsInWindow, computeVelocity } from "./useCodeInsights";
import { retentionSpanFor, type RetentionSpan } from "../utils/retention";
import type {
  CodeStatsHistoryPoint,
  InsightTrend,
  LlmRuntimeStats,
  TokenDataPoint,
} from "../types";

const WEEK_MS = 7 * 24 * 60 * 60 * 1000;
const HISTORY_MS = 30 * 24 * 60 * 60 * 1000;
/** The widest fixed range, so one fetch covers both compared weeks. */
const HISTORY_RANGE = "30d";
const REFRESH_DEBOUNCE_MS = 1000;
const REFRESH_INTERVAL_MS = 60_000;
/**
 * Percent moves below this read as noise rather than as a trend. Matches the
 * threshold [[src/hooks/useCodeInsights.ts#computeTrend]] applies to its own
 * period comparison, so the same week never reads "flat" on one surface and
 * "up 2%" on another.
 */
const FLAT_THRESHOLD_PCT = 3;

/** One metric over the two compared weeks. */
export interface WeeklyMetric {
  /** Last seven days; null when the metric has nothing to report. */
  readonly current: number | null;
  /** The seven days before that; null when absent or pruned. */
  readonly previous: number | null;
  /** Null whenever a side is missing — a delta needs both. */
  readonly trend: InsightTrend | null;
}

export interface WeeklyTrends {
  readonly tokens: WeeklyMetric;
  readonly velocity: WeeklyMetric;
  readonly cache: WeeklyMetric;
  /** ISO bounds of the compared windows, oldest first. */
  readonly previousStart: string;
  readonly currentStart: string;
  readonly end: string;
  /**
   * How each compared week sits against the retention watermark. Only velocity
   * degrades — it reads `tool_actions` and `session_events`, both of which
   * retention prunes, while token figures come from snapshots retention never
   * touches.
   */
  readonly velocitySpans: {
    readonly current: RetentionSpan;
    readonly previous: RetentionSpan;
  };
}

export interface WeeklyTrendsResult {
  readonly data: WeeklyTrends | null;
  readonly loading: boolean;
  readonly error: string | null;
}

/**
 * A percent change carrying its own meaning. `upIsGood` is null where rising is
 * neither good nor bad (token volume), which renders neutral instead of
 * guessing that more tokens is an achievement.
 */
function weeklyTrend(
  current: number | null,
  previous: number | null,
  upIsGood: boolean | null,
): InsightTrend | null {
  if (current === null || previous === null || previous === 0) return null;
  const percent = Math.round(((current - previous) / previous) * 100);
  if (Math.abs(percent) < FLAT_THRESHOLD_PCT) {
    return { direction: "flat", percentage: 0, upIsGood };
  }
  return {
    direction: percent > 0 ? "up" : "down",
    percentage: Math.abs(percent),
    upIsGood,
  };
}

function metric(
  current: number | null,
  previous: number | null,
  upIsGood: boolean | null,
): WeeklyMetric {
  return { current, previous, trend: weeklyTrend(current, previous, upIsGood) };
}

/**
 * Velocity for one week, or null when the week holds no evidence at all. A
 * quiet week has no lines and no runtime, and `0 LOC / hr` would state that we
 * measured a week of zero output rather than that we have nothing to divide.
 */
function weekVelocity(loc: number, activeSecs: number): number | null {
  if (loc === 0 && activeSecs <= 0) return null;
  return computeVelocity(loc, activeSecs, WEEK_MS);
}

/** Cache hit rate on the denominator the rest of the app uses. */
function hitRate(cacheRead: number, servedInput: number): number | null {
  if (servedInput <= 0) return null;
  return Math.round((cacheRead / servedInput) * 100);
}

async function loadWeeklyTrends(cutoff: string | null): Promise<WeeklyTrends> {
  const [tokenHistory, codeHistory, historyRuntime, weekRuntime] = await Promise.all([
    invoke<TokenDataPoint[]>("get_token_history", {
      range: HISTORY_RANGE,
      provider: null,
      hostname: null,
      sessionId: null,
      cwd: null,
    }),
    invoke<CodeStatsHistoryPoint[]>("get_code_stats_history", {
      range: HISTORY_RANGE,
    }),
    invoke<LlmRuntimeStats>("get_llm_runtime_stats", { range: HISTORY_RANGE }),
    // The current week's active seconds are measured, not prorated: this is the
    // same read the Usage view's runtime readout makes at 7D.
    invoke<LlmRuntimeStats>("get_llm_runtime_stats", { range: "7d" }),
  ]);

  const now = Date.now();
  const currentStart = now - WEEK_MS;
  const previousStart = currentStart - WEEK_MS;

  let currentTokens = 0;
  let previousTokens = 0;
  let currentCacheRead = 0;
  let previousCacheRead = 0;
  let currentServed = 0;
  let previousServed = 0;
  for (const point of tokenHistory) {
    const at = Date.parse(point.timestamp);
    if (!Number.isFinite(at) || at < previousStart) continue;
    // Cache efficiency is read tokens over everything that could have been
    // served from cache — the denominator `get_token_stats` consumers use.
    const served =
      point.input_tokens +
      point.cache_creation_input_tokens +
      point.cache_read_input_tokens;
    if (at >= currentStart) {
      currentTokens += point.total_tokens;
      currentCacheRead += point.cache_read_input_tokens;
      currentServed += served;
    } else {
      previousTokens += point.total_tokens;
      previousCacheRead += point.cache_read_input_tokens;
      previousServed += served;
    }
  }

  let currentLoc = 0;
  let previousLoc = 0;
  for (const point of codeHistory) {
    const at = Date.parse(point.timestamp);
    if (!Number.isFinite(at) || at < previousStart) continue;
    if (at >= currentStart) currentLoc += point.total_changed;
    else previousLoc += point.total_changed;
  }

  const previousActiveSecs = activeSecsInWindow(
    historyRuntime.sparkline,
    now - HISTORY_MS,
    HISTORY_MS,
    previousStart,
    currentStart,
  );

  const end = new Date(now).toISOString();
  const currentStartIso = new Date(currentStart).toISOString();
  const previousStartIso = new Date(previousStart).toISOString();
  const velocitySpans = {
    current: retentionSpanFor(currentStartIso, end, cutoff),
    previous: retentionSpanFor(previousStartIso, currentStartIso, cutoff),
  } as const;

  // A week that sits entirely below the watermark has had its code rows
  // deleted, so whatever survives is not a measurement of that week. Absent
  // beats a number that would read as a collapse in velocity.
  const currentVelocity =
    velocitySpans.current === "pruned"
      ? null
      : weekVelocity(currentLoc, weekRuntime.total_runtime_secs);
  const previousVelocity =
    velocitySpans.previous === "pruned"
      ? null
      : weekVelocity(previousLoc, previousActiveSecs);

  return {
    tokens: metric(
      currentTokens > 0 ? currentTokens : null,
      previousTokens > 0 ? previousTokens : null,
      null,
    ),
    velocity: metric(currentVelocity, previousVelocity, true),
    cache: metric(
      hitRate(currentCacheRead, currentServed),
      hitRate(previousCacheRead, previousServed),
      true,
    ),
    previousStart: previousStartIso,
    currentStart: currentStartIso,
    end,
    velocitySpans,
  };
}

/**
 * Week-over-week tokens, velocity and cache efficiency.
 *
 * `cutoff` is the retention watermark from
 * [[src/hooks/useRetentionCutoff.ts#useRetentionCutoff]]; it takes part in the
 * fetch identity so a maintenance run that moves the boundary re-derives the
 * figures instead of leaving a pruned week rendered as a real one.
 */
export function useWeeklyTrends(cutoff: string | null): WeeklyTrendsResult {
  const request = useCallback(() => loadWeeklyTrends(cutoff), [cutoff]);
  const { state, refresh } = useCachedInvoke({
    identity: `weekly-trends:${cutoff ?? "none"}`,
    request,
    normalizeError: String,
  });

  useEffect(() => {
    let timer: ReturnType<typeof setTimeout> | null = null;
    const schedule = () => {
      if (timer) clearTimeout(timer);
      timer = setTimeout(refresh, REFRESH_DEBOUNCE_MS);
    };
    const unlistens = [
      listen("tokens-updated", schedule),
      listen("transcript-analytics-updated", schedule),
    ];
    const interval = setInterval(refresh, REFRESH_INTERVAL_MS);
    return () => {
      if (timer) clearTimeout(timer);
      clearInterval(interval);
      for (const unlisten of unlistens) unlisten.then((stop) => stop());
    };
  }, [refresh]);

  return {
    data: state.data,
    loading: state.initialLoading,
    error: state.error,
  };
}
