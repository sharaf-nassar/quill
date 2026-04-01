import { useState, useEffect, useCallback, useRef } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import type {
  IntegrationProvider,
  RangeType,
  TokenDataPoint,
  TokenStats,
} from "../types";

const RANGE_DAYS: Record<RangeType, number> = {
  "1h": 1,
  "24h": 1,
  "7d": 7,
  "30d": 30,
};

const REFRESH_DEBOUNCE_MS = 1000;

export function useTokenData(
  range: RangeType,
  provider: IntegrationProvider | null,
  hostname: string | null,
  sessionId: string | null,
  cwd: string | null,
) {
  const [history, setHistory] = useState<TokenDataPoint[]>([]);
  const [stats, setStats] = useState<TokenStats | null>(null);
  const [hostnames, setHostnames] = useState<string[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);

  const initialLoadDone = useRef(false);

  const fetchData = useCallback(async () => {
    if (!initialLoadDone.current) {
      setLoading(true);
    }
    setError(null);

    try {
      const days = RANGE_DAYS[range] ?? 1;
      const providerArg = provider || null;
      const hostnameArg = hostname || null;
      const sessionIdArg = sessionId || null;
      const cwdArg = cwd || null;

      const [historyData, statsData, hostnameData] = await Promise.all([
        invoke<TokenDataPoint[]>("get_token_history", {
          range,
          provider: providerArg,
          hostname: hostnameArg,
          sessionId: sessionIdArg,
          cwd: cwdArg,
        }),
        invoke<TokenStats>("get_token_stats", {
          days,
          provider: providerArg,
          hostname: hostnameArg,
          sessionId: sessionIdArg,
          cwd: cwdArg,
        }),
        invoke<string[]>("get_token_hostnames"),
      ]);

      setHistory(historyData);
      setStats(statsData);
      setHostnames(hostnameData);
    } catch (e) {
      console.error("Token data fetch error:", e);
      setError(String(e));
    } finally {
      setLoading(false);
      initialLoadDone.current = true;
    }
  }, [range, provider, hostname, sessionId, cwd]);

  useEffect(() => {
    fetchData();
  }, [fetchData]);

  // Auto-refresh when new token data arrives via Tauri event
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

  // Periodic fallback refresh for idle periods when no token events fire
  useEffect(() => {
    const interval = setInterval(fetchData, 60_000);
    return () => clearInterval(interval);
  }, [fetchData]);

  return { history, stats, hostnames, loading, error, refresh: fetchData };
}
