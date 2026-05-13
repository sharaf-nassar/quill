import { useState, useEffect, useCallback, useRef } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import type {
  BreakdownMode,
  HostBreakdown,
  IntegrationProvider,
  ProjectBreakdown,
  SessionBreakdown,
  SkillBreakdown,
} from "../types";

type BreakdownRow = HostBreakdown | ProjectBreakdown | SessionBreakdown | SkillBreakdown;
interface BreakdownOptions {
  skillAllTime?: boolean;
  skillProvider?: IntegrationProvider | null;
}

const REFRESH_DEBOUNCE_MS = 1000;
const SESSION_BREAKDOWN_LIMIT = 200;
const SKILL_BREAKDOWN_LIMIT = 100;

export function useBreakdownData(mode: BreakdownMode, days: number, options: BreakdownOptions = {}) {
  const [data, setData] = useState<BreakdownRow[]>([]);
  const [dataMode, setDataMode] = useState<BreakdownMode>(mode);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const currentMode = useRef(mode);
  const currentRequestKey = useRef("");
  const skillAllTime = options.skillAllTime ?? false;
  const skillProvider = options.skillProvider ?? null;
  const requestKey = `${mode}:${days}:${skillAllTime}:${skillProvider ?? "all"}`;

  useEffect(() => {
    currentMode.current = mode;
    currentRequestKey.current = requestKey;
  }, [mode, requestKey]);

  const fetchData = useCallback(async () => {
    const activeRequestKey = requestKey;
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
      } else if (mode === "skills") {
        result = await invoke<SkillBreakdown[]>("get_skill_breakdown", {
          days,
          provider: skillProvider,
          allTime: skillAllTime,
          limit: SKILL_BREAKDOWN_LIMIT,
        });
      } else {
        result = await invoke<SessionBreakdown[]>("get_session_breakdown", {
          days,
          hostname: null,
          limit: SESSION_BREAKDOWN_LIMIT,
        });
      }
      // Only apply if mode hasn't changed during the fetch
      if (currentMode.current === mode && currentRequestKey.current === activeRequestKey) {
        setData(result);
        setDataMode(mode);
      }
    } catch (e) {
      console.error("Breakdown data fetch error:", e);
      if (currentMode.current === mode && currentRequestKey.current === activeRequestKey) {
        setError(String(e));
      }
    } finally {
      if (currentMode.current === mode && currentRequestKey.current === activeRequestKey) {
        setLoading(false);
      }
    }
  }, [mode, days, requestKey, skillAllTime, skillProvider]);

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
