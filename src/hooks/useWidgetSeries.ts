// Bucketed session and project counts behind the widget readout sparklines.

import { useCallback } from "react";
import { invoke } from "@tauri-apps/api/core";
import type { ActivitySeriesResponse, RangeType } from "../types";
import { useCachedInvoke } from "./useCachedInvoke";

/** Points the widget draws per series; mirrors the Rust default grid. */
export const WIDGET_SERIES_BUCKETS = 8;

export interface WidgetSeriesResult<T> {
  /** Last accepted response, retained across refreshes. */
  data: T | null;
  loading: boolean;
  error: string | null;
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
  const { state } = useCachedInvoke({
    command: "get_activity_series",
    args: { range, buckets },
    request,
    normalizeError: String,
    invalidationEvents: ["tokens-updated"],
    pollMs: 60_000,
  });

  return {
    data: state.data,
    loading: state.initialLoading,
    error: state.error,
  };
}
