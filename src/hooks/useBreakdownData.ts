import { useState, useEffect, useCallback, useRef } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import type {
  BreakdownMode,
  HostBreakdown,
  ProjectBreakdown,
  SessionBreakdown,
} from "../types";

type BreakdownRow = HostBreakdown | ProjectBreakdown | SessionBreakdown;

const REFRESH_DEBOUNCE_MS = 1000;
const SESSION_BREAKDOWN_LIMIT = 200;

export function useBreakdownData(mode: BreakdownMode, days: number) {
  const [data, setData] = useState<BreakdownRow[]>([]);
  const [dataMode, setDataMode] = useState<BreakdownMode>(mode);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const currentMode = useRef(mode);

  useEffect(() => {
    currentMode.current = mode;
  }, [mode]);

  const fetchData = useCallback(async () => {
    setLoading(true);
    setError(null);

    try {
      let result: BreakdownRow[];
      if (mode === "hosts") {
        result = await invoke<HostBreakdown[]>("get_host_breakdown", { days });
      } else if (mode === "projects") {
        result = await invoke<ProjectBreakdown[]>("get_project_breakdown", {
          days,
        });
      } else {
        result = await invoke<SessionBreakdown[]>("get_session_breakdown", {
          days,
          hostname: null,
          limit: SESSION_BREAKDOWN_LIMIT,
        });
      }
      // Only apply if mode hasn't changed during the fetch
      if (currentMode.current === mode) {
        setData(result);
        setDataMode(mode);
      }
    } catch (e) {
      console.error("Breakdown data fetch error:", e);
      if (currentMode.current === mode) {
        setError(String(e));
      }
    } finally {
      if (currentMode.current === mode) {
        setLoading(false);
      }
    }
  }, [mode, days]);

  useEffect(() => {
    fetchData();
  }, [fetchData]);

  // Auto-refresh when new token data arrives via Tauri event
  useEffect(() => {
    let mounted = true;
    let timer: ReturnType<typeof setTimeout> | null = null;
    const scheduleRefresh = () => {
      if (!mounted) return;
      if (timer) clearTimeout(timer);
      timer = setTimeout(fetchData, REFRESH_DEBOUNCE_MS);
    };
    const unlistenPromises = [
      listen("tokens-updated", scheduleRefresh),
      listen("sessions-index-updated", scheduleRefresh),
    ];
    return () => {
      mounted = false;
      if (timer) clearTimeout(timer);
      for (const unlistenPromise of unlistenPromises) {
        unlistenPromise.then((fn) => fn());
      }
    };
  }, [fetchData]);

  // Return loading when mode and dataMode are out of sync
  const stale = mode !== dataMode;

  return {
    data: stale ? [] : data,
    loading: loading || stale,
    error,
    refresh: fetchData,
  };
}
