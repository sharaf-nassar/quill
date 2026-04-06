import { useCallback, useEffect, useRef, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import { invoke } from "@tauri-apps/api/core";
import {
  sessionRefKey,
  type CodexLiveRange,
  type CodexLiveData,
  type CodexLiveSessionRow,
  type SessionBreakdown,
  type SessionCodeStats,
  type SparklinePoint,
  type TokenDataPoint,
} from "../types";

const REFRESH_INTERVAL_MS = 60_000;
const REFRESH_DEBOUNCE_MS = 1_000;
const HOUR_MS = 60 * 60 * 1000;
const SESSION_BREAKDOWN_LIMIT = 500;
const TOKEN_SPARKLINE_BUCKETS = 12;
const PULSE_BUCKETS = 6;
const LIVE_RANGE_HOURS: Record<CodexLiveRange, number> = {
  "1h": 1,
  "6h": 6,
  "12h": 12,
  "24h": 24,
};
const RECENCY_BUCKETS = 6;

const ZERO_SESSION_STATS: SessionCodeStats = {
  lines_added: 0,
  lines_removed: 0,
  net_change: 0,
};

function toTimestampMs(value: string | null | undefined): number | null {
  if (!value) return null;
  const ms = new Date(value).getTime();
  return Number.isNaN(ms) ? null : ms;
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

function sessionCodeStatsMap(
  raw: Record<string, SessionCodeStats>,
  sessionId: string,
): SessionCodeStats {
  return raw[sessionRefKey({ provider: "codex", session_id: sessionId })] ?? ZERO_SESSION_STATS;
}

function normalizeSessionRow(
  session: SessionBreakdown,
  tokenHistory1h: TokenDataPoint[],
  cutoffMs: number,
  codeStats: SessionCodeStats,
): CodexLiveSessionRow {
  const turnEstimate = tokenHistory1h.reduce((count, point) => {
    const timestampMs = toTimestampMs(point.timestamp);
    return timestampMs !== null && timestampMs >= cutoffMs ? count + 1 : count;
  }, 0);

  return {
    provider: "codex",
    sessionId: session.session_id,
    hostname: session.hostname,
    project: session.project,
    firstSeen: session.first_seen,
    lastActive: session.last_active,
    tokens: sumTokenHistory(tokenHistory1h, cutoffMs),
    turnEstimate,
    linesAdded: codeStats.lines_added,
    linesRemoved: codeStats.lines_removed,
    netChange: codeStats.net_change,
  };
}

export function useCodexLiveData(range: CodexLiveRange = "1h") {
  const [data, setData] = useState<CodexLiveData | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const initialLoadDone = useRef(false);
  const requestIdRef = useRef(0);

  const fetchData = useCallback(async () => {
    const requestId = ++requestIdRef.current;

    if (!initialLoadDone.current) {
      setLoading(true);
    }
    setError(null);

    try {
      const [sessionBreakdown, tokenHistory] = await Promise.all([
        invoke<SessionBreakdown[]>("get_session_breakdown", {
          days: 1,
          hostname: null,
          provider: "codex",
          limit: SESSION_BREAKDOWN_LIMIT,
        }),
        invoke<TokenDataPoint[]>("get_token_history", {
          range: "24h",
          provider: "codex",
          hostname: null,
          sessionId: null,
          cwd: null,
        }),
      ]);

      if (requestId !== requestIdRef.current) {
        return;
      }

      const nowMs = Date.now();
      const windowMs = LIVE_RANGE_HOURS[range] * HOUR_MS;
      const cutoffMs = nowMs - windowMs;
      const selectedTokenHistory = tokenHistory.filter((point) => {
        const timestampMs = toTimestampMs(point.timestamp);
        return timestampMs !== null && timestampMs >= cutoffMs;
      });
      const activeSessions = sessionBreakdown.filter((session) => {
        if (session.provider !== "codex") return false;
        const lastActiveMs = toTimestampMs(session.last_active);
        return lastActiveMs !== null && lastActiveMs >= cutoffMs;
      });

      const sessionRefs = activeSessions.map((session) => ({
        provider: "codex" as const,
        session_id: session.session_id,
      }));

      const perSessionHistoryPromises = activeSessions.map(async (session) => {
        const history = await invoke<TokenDataPoint[]>("get_token_history", {
          range: "24h",
          provider: "codex",
          hostname: null,
          sessionId: session.session_id,
          cwd: null,
        });

        return [session.session_id, history] as const;
      });

      const [sessionHistoryEntries, codeStats] = await Promise.all([
        Promise.all(perSessionHistoryPromises),
        sessionRefs.length > 0
          ? invoke<Record<string, SessionCodeStats>>("get_batch_session_code_stats", {
              sessionRefs,
            })
          : Promise.resolve({} as Record<string, SessionCodeStats>),
      ]);

      if (requestId !== requestIdRef.current) {
        return;
      }

      const sessionHistoryMap = new Map(sessionHistoryEntries);
      const normalizedSessions = activeSessions
        .map((session) =>
          normalizeSessionRow(
            session,
            sessionHistoryMap.get(session.session_id) ?? [],
            cutoffMs,
            sessionCodeStatsMap(codeStats, session.session_id),
          ),
        )
        .sort((a, b) => {
          if (b.tokens !== a.tokens) {
            return b.tokens - a.tokens;
          }
          return (toTimestampMs(b.lastActive) ?? 0) - (toTimestampMs(a.lastActive) ?? 0);
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

      const tokensValue = sumTokenHistory(tokenHistory, cutoffMs);
      const tokensSparkline = buildTokenSparkline(
        selectedTokenHistory,
        TOKEN_SPARKLINE_BUCKETS,
        nowMs,
        windowMs,
      );
      const activityPulse = buildTokenSparkline(
        selectedTokenHistory,
        PULSE_BUCKETS,
        nowMs,
        windowMs,
      );
      const activeSessionSparkline = buildRecencySeries(
        activeSessions.map((session) => session.last_active),
        RECENCY_BUCKETS,
        nowMs,
        windowMs,
      );
      const activeProjectSparkline = buildRecencySeries(
        [...latestProjectActivity.values()],
        RECENCY_BUCKETS,
        nowMs,
        windowMs,
      );

      setData({
        fetchedAt: new Date(nowMs).toISOString(),
        lastActivityAt: latestActivityAt,
        tokens: {
          value: tokensValue,
          sparkline: tokensSparkline,
          lastActivityAt: latestActivityAt,
        },
        activeSessions: {
          value: activeSessions.length,
          sparkline: activeSessionSparkline,
          lastActivityAt: maxTimestamp(activeSessions.map((session) => session.last_active)),
        },
        activeProjects: {
          value: latestProjectActivity.size,
          sparkline: activeProjectSparkline,
          lastActivityAt: maxTimestamp([...latestProjectActivity.values()]),
        },
        activityPulse,
        sessions: normalizedSessions,
      });
    } catch (e) {
      if (requestId !== requestIdRef.current) {
        return;
      }
      console.error("Codex live data fetch error:", e);
      setError(String(e));
    } finally {
      if (requestId === requestIdRef.current) {
        setLoading(false);
        initialLoadDone.current = true;
      }
    }
  }, [range]);

  useEffect(() => {
    fetchData();
  }, [fetchData]);

  useEffect(() => {
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
  }, [fetchData]);

  useEffect(() => {
    const interval = setInterval(fetchData, REFRESH_INTERVAL_MS);
    return () => clearInterval(interval);
  }, [fetchData]);

  return { data, loading, error, refresh: fetchData };
}
