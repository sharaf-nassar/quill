// Bucketed session and project counts behind the widget readout sparklines.

import { useCallback, useEffect } from "react";
import { invoke } from "@tauri-apps/api/core";
import type { ActivitySeriesResponse, RangeType } from "../types";
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

/** Per-bucket distinct session and project counts for `range`. */
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
