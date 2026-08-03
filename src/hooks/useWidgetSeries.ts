// Bucketed series behind the widget's hero chart and its sessions/projects
// sparklines. Both aggregates read `token_snapshots` on the same grid, so they
// are fetched with one bucket count and refreshed on the same signal — a chart
// and a sparkline drawn from different windows would be a quiet lie.

import { useCallback, useEffect } from "react";
import { invoke } from "@tauri-apps/api/core";
import type {
  ActivitySeriesResponse,
  ProviderTokenSeriesResponse,
  RangeType,
} from "../types";
import { useCachedInvoke } from "./useCachedInvoke";

/** Points the widget draws per series; mirrors the Rust default grid. */
export const WIDGET_SERIES_BUCKETS = 8;

const REFRESH_INTERVAL_MS = 60_000;

export interface WidgetSeriesResult<T> {
  /** Last accepted response, retained across refreshes. */
  data: T | null;
  loading: boolean;
  error: string | null;
}

/**
 * Refreshes on new token snapshots, debounced, plus a slow poll so a widget
 * left open still ages its window forward.
 */
function useSnapshotRefresh(refresh: () => void): void {
  useEffect(() => {
    const interval = setInterval(refresh, REFRESH_INTERVAL_MS);
    return () => clearInterval(interval);
  }, [refresh]);
}

/**
 * Per-provider token series for `range`.
 *
 * `total_tokens` on the response matches `get_token_stats` for the same range,
 * so the headline overlaid on the chart is the same number the areas add up
 * to.
 */
export function useProviderTokenSeries(
  range: RangeType,
  buckets: number = WIDGET_SERIES_BUCKETS,
): WidgetSeriesResult<ProviderTokenSeriesResponse> {
  const request = useCallback(
    () =>
      invoke<ProviderTokenSeriesResponse>("get_provider_token_series", {
        range,
        buckets,
      }),
    [range, buckets],
  );
  const { state, refresh } = useCachedInvoke({
    command: "get_provider_token_series",
    args: { range, buckets },
    request,
    normalizeError: String,
    invalidationEvents: ["tokens-updated"],
  });
  useSnapshotRefresh(refresh);

  return {
    data: state.data,
    loading: state.initialLoading,
    error: state.error,
  };
}

/**
 * Per-bucket distinct session and project counts for `range`, aligned to the
 * same grid as {@link useProviderTokenSeries}.
 */
export function useActivitySeries(
  range: RangeType,
  buckets: number = WIDGET_SERIES_BUCKETS,
): WidgetSeriesResult<ActivitySeriesResponse> {
  const request = useCallback(
    () =>
      invoke<ActivitySeriesResponse>("get_activity_series", {
        range,
        buckets,
      }),
    [range, buckets],
  );
  const { state, refresh } = useCachedInvoke({
    command: "get_activity_series",
    args: { range, buckets },
    request,
    normalizeError: String,
    invalidationEvents: ["tokens-updated"],
  });
  useSnapshotRefresh(refresh);

  return {
    data: state.data,
    loading: state.initialLoading,
    error: state.error,
  };
}
