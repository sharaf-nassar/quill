import { useState, useEffect, useCallback, useRef } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { useCachedInvoke } from "./useCachedInvoke";
import type {
  IntegrationProvider,
  RangeType,
  TokenDataPoint,
  TokenStats,
} from "../types";

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
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);

  const initialLoadDone = useRef(false);

  const fetchData = useCallback(async () => {
    if (!initialLoadDone.current) {
      setLoading(true);
    }
    setError(null);

    try {
      const providerArg = provider || null;
      const hostnameArg = hostname || null;
      const sessionIdArg = sessionId || null;
      const cwdArg = cwd || null;

      const [historyData, statsData] = await Promise.all([
        invoke<TokenDataPoint[]>("get_token_history", {
          range,
          provider: providerArg,
          hostname: hostnameArg,
          sessionId: sessionIdArg,
          cwd: cwdArg,
        }),
        invoke<TokenStats>("get_token_stats", {
          range,
          provider: providerArg,
          hostname: hostnameArg,
          sessionId: sessionIdArg,
          cwd: cwdArg,
        }),
      ]);

      setHistory(historyData);
      setStats(statsData);
    } catch (e) {
      console.error("Token data fetch error:", e);
      setError(String(e));
    } finally {
      setLoading(false);
      initialLoadDone.current = true;
    }
  }, [range, provider, hostname, sessionId, cwd]);

	// Hostnames are independent of the selected range and filters. Keep this
	// request outside the range-keyed data refresh so changing a range never
	// repeats it.
	const fetchHostnames = useCallback(
		() => invoke<string[]>("get_token_hostnames"),
		[],
	);
	const { state: hostnameState } = useCachedInvoke({
		identity: "token-hostnames",
		request: fetchHostnames,
		normalizeError: String,
	});

	useCachedInvoke({
		identity: `token-data:${range}:${provider ?? "all"}:${hostname ?? "all"}:${sessionId ?? "all"}:${cwd ?? "all"}`,
		request: fetchData,
		normalizeError: String,
	});

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

  return {
		history,
		stats,
		hostnames: hostnameState.data ?? [],
		loading,
		error: error ?? hostnameState.error,
		refresh: fetchData,
	};
}
