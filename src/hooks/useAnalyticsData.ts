import { useState, useEffect, useCallback, useRef } from "react";
import { invoke } from "@tauri-apps/api/core";
import type { RangeType, DataPoint, BucketStats, UsageBucket } from "../types";
import { usageBucketRefKey } from "../types";

const REFRESH_INTERVAL_MS = 60_000; // Re-fetch every 60s to keep chart current

const RANGES: Record<RangeType, { label: string; days: number }> = {
  "1h": { label: "1 Hour", days: 1 },
  "24h": { label: "24 Hours", days: 1 },
  "7d": { label: "7 Days", days: 7 },
  "30d": { label: "30 Days", days: 30 },
};

export function useAnalyticsData(
  bucket: UsageBucket | null,
  range: RangeType,
) {
  const [history, setHistory] = useState<DataPoint[]>([]);
  const [stats, setStats] = useState<BucketStats | null>(null);
  const [snapshotCount, setSnapshotCount] = useState(0);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);

  const bucketRef = useRef(bucket);
  bucketRef.current = bucket;
  const bucketIdentity = bucket ? usageBucketRefKey(bucket) : "none";

  const initialLoadDone = useRef(false);
  const requestIdRef = useRef(0);

  const fetchData = useCallback(async () => {
    const requestId = requestIdRef.current + 1;
    requestIdRef.current = requestId;
    // Only show loading skeleton on initial fetch, not periodic refreshes
    if (!initialLoadDone.current) {
      setLoading(true);
    }
    setError(null);

    try {
      const days = RANGES[range]?.days ?? 1;
      const currentBucket = bucketRef.current;
      const hasBucket = bucketIdentity !== "none" && currentBucket !== null;

      const [historyData, countData, statsData] = await Promise.all([
        hasBucket
          ? invoke<DataPoint[]>("get_usage_history", {
              provider: currentBucket.provider,
              bucketKey: currentBucket.key,
              range,
            })
          : Promise.resolve([]),
        invoke<number>("get_snapshot_count"),
        hasBucket
          ? invoke<BucketStats>("get_usage_stats", {
              provider: currentBucket.provider,
              bucketKey: currentBucket.key,
              days,
            })
          : Promise.resolve(null),
      ]);

      if (requestId !== requestIdRef.current) {
        return;
      }

      setHistory(historyData);
      setSnapshotCount(countData);

      if (hasBucket && currentBucket && statsData) {
        setStats({ ...statsData, current: currentBucket.utilization });
      } else {
        setStats(null);
      }
    } catch (e) {
      if (requestId !== requestIdRef.current) {
        return;
      }
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

  // Periodic refresh so the chart stays current even during idle periods
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
