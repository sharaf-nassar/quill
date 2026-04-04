import { useCallback, useEffect, useRef, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import { invoke } from "@tauri-apps/api/core";
import type {
  IntegrationProvider,
  SessionBreakdown,
  SparklinePoint,
  TokenDataPoint,
} from "../types";

const REFRESH_INTERVAL_MS = 60_000;
const REFRESH_DEBOUNCE_MS = 1_000;
const HOUR_MS = 60 * 60 * 1000;
const SESSION_BREAKDOWN_LIMIT = 500;
const TOKEN_SPARKLINE_BUCKETS = 12;
const RECENCY_BUCKETS = 6;

export type LiveSummaryRange = "1h" | "6h" | "12h" | "24h";

export interface LiveSummarySeries {
  value: number;
  sparkline: SparklinePoint[];
  lastActivityAt: string | null;
}

export interface LiveSummaryData {
  fetchedAt: string;
  lastActivityAt: string | null;
  tokens: LiveSummarySeries;
  activeSessions: LiveSummarySeries;
  activeProjects: LiveSummarySeries;
}

const LIVE_RANGE_HOURS: Record<LiveSummaryRange, number> = {
  "1h": 1,
  "6h": 6,
  "12h": 12,
  "24h": 24,
};

function toTimestampMs(value: string | null | undefined): number | null {
  if (!value) return null;
  const ms = new Date(value).getTime();
  return Number.isNaN(ms) ? null : ms;
}

function maxTimestamp(values: Array<string | null | undefined>): string | null {
  let latestMs = -Infinity;
  let latestValue: string | null = null;

  for (const value of values) {
    const timestampMs = toTimestampMs(value);
    if (timestampMs === null || timestampMs <= latestMs) continue;
    latestMs = timestampMs;
    latestValue = value ?? null;
  }

  return latestValue;
}

function sumTokenHistory(history: TokenDataPoint[], cutoffMs: number): number {
  return history.reduce((total, point) => {
    const timestampMs = toTimestampMs(point.timestamp);
    if (timestampMs === null || timestampMs < cutoffMs) {
      return total;
    }
    return total + point.total_tokens;
  }, 0);
}

function buildBucketSeries(
  entries: Array<{ timestamp: string; value: number }>,
  bucketCount: number,
  nowMs: number,
  windowMs: number,
): SparklinePoint[] {
  const values = new Array(bucketCount).fill(0);
  const windowStart = nowMs - windowMs;
  const bucketSize = windowMs / bucketCount;

  for (const entry of entries) {
    const timestampMs = toTimestampMs(entry.timestamp);
    if (timestampMs === null || timestampMs < windowStart) continue;
    const bucket = Math.min(
      bucketCount - 1,
      Math.floor((timestampMs - windowStart) / bucketSize),
    );
    values[bucket] += entry.value;
  }

  return values.map((value) => ({ value }));
}

function buildRecencySeries(
  timestamps: string[],
  bucketCount: number,
  nowMs: number,
  windowMs: number,
): SparklinePoint[] {
  return buildBucketSeries(
    timestamps.map((timestamp) => ({ timestamp, value: 1 })),
    bucketCount,
    nowMs,
    windowMs,
  );
}

function buildTokenSparkline(
  history: TokenDataPoint[],
  bucketCount: number,
  nowMs: number,
  windowMs: number,
): SparklinePoint[] {
  return buildBucketSeries(
    history.map((point) => ({
      timestamp: point.timestamp,
      value: point.total_tokens,
    })),
    bucketCount,
    nowMs,
    windowMs,
  );
}

export function useLiveSummaryData(
  range: LiveSummaryRange,
  enabledProviders: IntegrationProvider[],
) {
  const [data, setData] = useState<LiveSummaryData | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const initialLoadDone = useRef(false);
  const requestIdRef = useRef(0);
  const providerKey = enabledProviders.join("|");

  const fetchData = useCallback(async () => {
    const requestId = ++requestIdRef.current;

    if (!initialLoadDone.current) {
      setLoading(true);
    }
    setError(null);

    try {
      const providerFetches = await Promise.all(
        enabledProviders.map(async (provider) => {
          const [sessionBreakdown, tokenHistory] = await Promise.all([
            invoke<SessionBreakdown[]>("get_session_breakdown", {
              days: 1,
              hostname: null,
              provider,
              limit: SESSION_BREAKDOWN_LIMIT,
            }),
            invoke<TokenDataPoint[]>("get_token_history", {
              range: "24h",
              provider,
              hostname: null,
              sessionId: null,
              cwd: null,
            }),
          ]);

          return { provider, sessionBreakdown, tokenHistory };
        }),
      );

      if (requestId !== requestIdRef.current) {
        return;
      }

      const nowMs = Date.now();
      const windowMs = LIVE_RANGE_HOURS[range] * HOUR_MS;
      const cutoffMs = nowMs - windowMs;
      const allSessions = providerFetches.flatMap(({ sessionBreakdown }) => sessionBreakdown);
      const allTokenHistory = providerFetches.flatMap(({ tokenHistory }) => tokenHistory);
      const selectedTokenHistory = allTokenHistory.filter((point) => {
        const timestampMs = toTimestampMs(point.timestamp);
        return timestampMs !== null && timestampMs >= cutoffMs;
      });
      const activeSessions = allSessions.filter((session) => {
        const lastActiveMs = toTimestampMs(session.last_active);
        return lastActiveMs !== null && lastActiveMs >= cutoffMs;
      });

      const latestProjectActivity = new Map<string, string>();

      for (const session of activeSessions) {
        if (!session.project) continue;
        const currentProjectTimestamp = latestProjectActivity.get(session.project);
        const candidateMs = toTimestampMs(session.last_active);
        const currentMs = toTimestampMs(currentProjectTimestamp);
        if (candidateMs === null) continue;
        if (currentMs === null || candidateMs > currentMs) {
          latestProjectActivity.set(session.project, session.last_active);
        }
      }

      const latestActivityAt = maxTimestamp([
        ...selectedTokenHistory.map((point) => point.timestamp),
        ...activeSessions.map((session) => session.last_active),
      ]);

      setData({
        fetchedAt: new Date(nowMs).toISOString(),
        lastActivityAt: latestActivityAt,
        tokens: {
          value: sumTokenHistory(allTokenHistory, cutoffMs),
          sparkline: buildTokenSparkline(
            selectedTokenHistory,
            TOKEN_SPARKLINE_BUCKETS,
            nowMs,
            windowMs,
          ),
          lastActivityAt: maxTimestamp(selectedTokenHistory.map((point) => point.timestamp)),
        },
        activeSessions: {
          value: activeSessions.length,
          sparkline: buildRecencySeries(
            activeSessions.map((session) => session.last_active),
            RECENCY_BUCKETS,
            nowMs,
            windowMs,
          ),
          lastActivityAt: maxTimestamp(activeSessions.map((session) => session.last_active)),
        },
        activeProjects: {
          value: latestProjectActivity.size,
          sparkline: buildRecencySeries(
            [...latestProjectActivity.values()],
            RECENCY_BUCKETS,
            nowMs,
            windowMs,
          ),
          lastActivityAt: maxTimestamp([...latestProjectActivity.values()]),
        },
      });
    } catch (e) {
      if (requestId !== requestIdRef.current) {
        return;
      }
      console.error("Live summary data fetch error:", e);
      setError(String(e));
    } finally {
      if (requestId === requestIdRef.current) {
        setLoading(false);
        initialLoadDone.current = true;
      }
    }
  }, [enabledProviders, range]);

  useEffect(() => {
    if (enabledProviders.length === 0) {
      setData(null);
      setLoading(false);
      setError(null);
      return;
    }
    fetchData();
  }, [enabledProviders.length, fetchData, providerKey]);

  useEffect(() => {
    if (enabledProviders.length === 0) return;

    let mounted = true;
    let timer: ReturnType<typeof setTimeout> | null = null;
    const unlistenPromise = listen("tokens-updated", () => {
      if (!mounted) return;
      if (timer) clearTimeout(timer);
      timer = setTimeout(fetchData, REFRESH_DEBOUNCE_MS);
    });

    return () => {
      mounted = false;
      if (timer) clearTimeout(timer);
      unlistenPromise.then((fn) => fn());
    };
  }, [enabledProviders.length, fetchData]);

  useEffect(() => {
    if (enabledProviders.length === 0) return;
    const interval = setInterval(fetchData, REFRESH_INTERVAL_MS);
    return () => clearInterval(interval);
  }, [enabledProviders.length, fetchData]);

  return { data, loading, error, refresh: fetchData };
}
