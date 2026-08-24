import { invoke } from "@tauri-apps/api/core";
import { useCallback, useEffect } from "react";
import { cachedInvokeStore } from "../hooks/cachedInvokeStore";
import { useCachedInvoke } from "../hooks/useCachedInvoke";
import type { CpaConnectionStatus, ProviderStatus, UsageData } from "../types";

const WEB_POLL_MS = 55_000;

export interface WebMonitorData {
  usageData: UsageData | null;
  statuses: ProviderStatus[];
  hasUsageSource: boolean;
  loading: boolean;
  sourcesUnavailable: boolean;
  usageUnavailable: boolean;
}

/** Cache-only monitor inputs plus browser visibility-aware revalidation. */
export function useWebMonitorData(): WebMonitorData {
  const requestUsage = useCallback(
    () => invoke<UsageData>("get_cached_usage_data"),
    [],
  );
  const requestStatuses = useCallback(
    () => invoke<ProviderStatus[]>("get_provider_statuses"),
    [],
  );
  const requestCpaStatus = useCallback(
    () => invoke<CpaConnectionStatus>("get_cpa_connection_status"),
    [],
  );

  const usage = useCachedInvoke({
    command: "get_cached_usage_data",
    args: {},
    request: requestUsage,
    normalizeError: String,
  });
  const providerStatuses = useCachedInvoke({
    command: "get_provider_statuses",
    args: {},
    request: requestStatuses,
    normalizeError: String,
  });
  const cpaStatus = useCachedInvoke({
    command: "get_cpa_connection_status",
    args: {},
    request: requestCpaStatus,
    normalizeError: String,
  });

  useEffect(() => {
    const refreshStale = () => {
      if (document.visibilityState !== "hidden") {
        cachedInvokeStore.refreshStaleSubscribers();
      }
    };

    const poll = window.setInterval(refreshStale, WEB_POLL_MS);
    window.addEventListener("focus", refreshStale);
    document.addEventListener("visibilitychange", refreshStale);
    return () => {
      window.clearInterval(poll);
      window.removeEventListener("focus", refreshStale);
      document.removeEventListener("visibilitychange", refreshStale);
    };
  }, []);

  const statuses = providerStatuses.state.data ?? [];
  const hasUsageSource =
    statuses.some((status) => status.enabled) || cpaStatus.state.data?.configured === true;

  return {
    usageData: usage.state.data,
    statuses,
    hasUsageSource,
    loading: providerStatuses.state.initialLoading || cpaStatus.state.initialLoading,
    sourcesUnavailable:
      providerStatuses.state.error !== null || cpaStatus.state.error !== null,
    usageUnavailable: usage.state.data === null && usage.state.error !== null,
  };
}
