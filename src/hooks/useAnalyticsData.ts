import { useState, useEffect, useCallback, useRef } from "react";
import { invoke } from "@tauri-apps/api/core";
import type { RangeType, DataPoint, BucketStats, MergedBucket } from "../types";
import { usageBucketRefKey } from "../types";

const REFRESH_INTERVAL_MS = 60_000; // Re-fetch every 60s to keep chart current

const RANGES: Record<RangeType, { label: string; days: number }> = {
  "1h": { label: "1 Hour", days: 1 },
  "24h": { label: "24 Hours", days: 1 },
  "7d": { label: "7 Days", days: 7 },
  "30d": { label: "30 Days", days: 30 },
};

function roundToMinute(ts: string): string {
  const d = new Date(ts);
  d.setSeconds(0, 0);
  return d.toISOString();
}

function mergeHistories(arrays: DataPoint[][]): DataPoint[] {
  if (arrays.length === 1) return arrays[0];
  const byMinute = new Map<string, number[]>();
  for (const arr of arrays) {
    for (const point of arr) {
      const key = roundToMinute(point.timestamp);
      const existing = byMinute.get(key) ?? [];
      existing.push(point.utilization);
      byMinute.set(key, existing);
    }
  }
  return Array.from(byMinute.entries())
    .map(([timestamp, values]) => ({
      timestamp,
      utilization: values.reduce((a, b) => a + b, 0) / values.length,
    }))
    .sort((a, b) => a.timestamp.localeCompare(b.timestamp));
}

function mergeStats(all: BucketStats[], merged: MergedBucket): BucketStats {
  if (all.length === 1) return { ...all[0], current: merged.utilization };
  const n = all.length;
  return {
    provider: all[0].provider,
    key: all[0].key,
    label: merged.label,
    current: merged.utilization,
    avg: all.reduce((s, st) => s + st.avg, 0) / n,
    max: Math.max(...all.map((st) => st.max)),
    min: Math.min(...all.map((st) => st.min)),
    time_above_80: all.reduce((s, st) => s + st.time_above_80, 0) / n,
    trend: all.every((st) => st.trend === all[0].trend) ? all[0].trend : "flat",
    sample_count: all.reduce((s, st) => s + st.sample_count, 0),
  };
}

export function useAnalyticsData(
  bucket: MergedBucket | null,
  range: RangeType,
) {
  const [history, setHistory] = useState<DataPoint[]>([]);
  const [stats, setStats] = useState<BucketStats | null>(null);
  const [snapshotCount, setSnapshotCount] = useState(0);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);

  const bucketRef = useRef(bucket);
  bucketRef.current = bucket;
  const bucketIdentity = bucket
    ? bucket.sources.map((s) => usageBucketRefKey(s)).sort().join("+")
    : "none";

  const initialLoadDone = useRef(false);
  const requestIdRef = useRef(0);

  const fetchData = useCallback(async () => {
    const requestId = requestIdRef.current + 1;
    requestIdRef.current = requestId;
    if (!initialLoadDone.current) {
      setLoading(true);
    }
    setError(null);

    try {
      const days = RANGES[range]?.days ?? 1;
      const current = bucketRef.current;
      const hasBucket = bucketIdentity !== "none" && current !== null;

      const historyPromises = hasBucket
        ? current.sources.map((src) =>
            invoke<DataPoint[]>("get_usage_history", {
              provider: src.provider,
              bucketKey: src.key,
              range,
            }),
          )
        : [];

      const statsPromises = hasBucket
        ? current.sources.map((src) =>
            invoke<BucketStats>("get_usage_stats", {
              provider: src.provider,
              bucketKey: src.key,
              days,
            }),
          )
        : [];

      const [historyArrays, countData, statsArrays] = await Promise.all([
        Promise.all(historyPromises),
        invoke<number>("get_snapshot_count"),
        Promise.all(statsPromises),
      ]);

      if (requestId !== requestIdRef.current) return;

      setHistory(
        historyArrays.length > 0 ? mergeHistories(historyArrays) : [],
      );
      setSnapshotCount(countData);

      if (hasBucket && current && statsArrays.length > 0) {
        setStats(mergeStats(statsArrays, current));
      } else {
        setStats(null);
      }
    } catch (e) {
      if (requestId !== requestIdRef.current) return;
      console.error("Analytics fetch error:", e);
      setError(String(e));
    } finally {
      if (requestId === requestIdRef.current) {
        setLoading(false);
        initialLoadDone.current = true;
      }
    }
  }, [bucketIdentity, range]);

  useEffect(() => {
    fetchData();
  }, [fetchData]);

  useEffect(() => {
    const interval = setInterval(fetchData, REFRESH_INTERVAL_MS);
    return () => clearInterval(interval);
  }, [fetchData]);

  return {
    history,
    stats,
    snapshotCount,
    loading,
    error,
    refresh: fetchData,
  };
}
