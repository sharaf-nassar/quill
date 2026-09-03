// Bucketed tool-call, turn, prompt, and reasoning series behind the widget
// readout sparklines. Sessions and projects lost their sparklines with the
// two-tier grid; `get_activity_series` now serves the Explorer alone.

import { useCallback } from "react";
import { invoke } from "@tauri-apps/api/core";
import type { RangeType, WidgetActivityStats } from "../types";
import { useCachedInvoke } from "./useCachedInvoke";

/** Points the widget draws per series; mirrors the Rust default grid. */
export const WIDGET_SERIES_BUCKETS = 8;

export interface WidgetSeriesResult<T> {
  /** Last accepted response, retained across refreshes. */
  data: T | null;
  loading: boolean;
  error: string | null;
}

/** Tool calls, turns, prompts, and reasoning tokens for `range` with series. */
export function useWidgetActivityStats(
  range: RangeType,
  buckets: number = WIDGET_SERIES_BUCKETS,
): WidgetSeriesResult<WidgetActivityStats> {
  const request = useCallback(
    () =>
      invoke<WidgetActivityStats>("get_widget_activity_stats", {
        range,
        buckets,
      }),
    [range, buckets],
  );
  const { state } = useCachedInvoke({
    command: "get_widget_activity_stats",
    args: { range, buckets },
    request,
    normalizeError: String,
    // Tool calls and prompts land with transcript analytics; reasoning with
    // model observations. Token reports arrive with each turn as well.
    invalidationEvents: [
      "tokens-updated",
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
