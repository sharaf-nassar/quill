import { useCallback, useEffect, useMemo, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import type { IntegrationProvider, ProviderStatus } from "../types";

const PROVIDER_ORDER: IntegrationProvider[] = ["claude", "codex", "mini_max"];

function sortStatuses(statuses: ProviderStatus[]): ProviderStatus[] {
  return [...statuses].sort(
    (a, b) => PROVIDER_ORDER.indexOf(a.provider) - PROVIDER_ORDER.indexOf(b.provider),
  );
}

function upsertStatus(
  statuses: ProviderStatus[],
  updated: ProviderStatus,
): ProviderStatus[] {
  const existing = statuses.find((status) => status.provider === updated.provider);
  if (!existing) {
    return sortStatuses([...statuses, updated]);
  }
  return statuses.map((status) =>
    status.provider === updated.provider ? updated : status,
  );
}

export interface UseIntegrationsResult {
  statuses: ProviderStatus[];
  loading: boolean;
  error: string | null;
  inFlightProviders: ReadonlySet<IntegrationProvider>;
  hasEnabledProvider: boolean;
  refresh: () => Promise<void>;
  enableProvider: (provider: IntegrationProvider, apiKey?: string) => Promise<ProviderStatus>;
  disableProvider: (provider: IntegrationProvider) => Promise<ProviderStatus>;
}

export function useIntegrations(): UseIntegrationsResult {
  const [statuses, setStatuses] = useState<ProviderStatus[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [inFlightProviders, setInFlightProviders] = useState<Set<IntegrationProvider>>(
    new Set(),
  );

  const refresh = useCallback(async () => {
    setLoading(true);
    try {
      const data = await invoke<ProviderStatus[]>("get_provider_statuses");
      setStatuses(sortStatuses(data));
      setError(null);
    } catch (e) {
      setError(String(e));
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    refresh();
  }, [refresh]);

  useEffect(() => {
    const unlisten = listen<ProviderStatus[]>("integrations-updated", (event) => {
      setStatuses(sortStatuses(event.payload));
      setLoading(false);
      setError(null);
    });
    return () => {
      unlisten.then((fn) => fn());
    };
  }, []);

  const runProviderCommand = useCallback(
    async (
      provider: IntegrationProvider,
      command: "confirm_enable_provider" | "confirm_disable_provider",
      extraArgs?: Record<string, unknown>,
    ) => {
      setInFlightProviders((prev) => new Set(prev).add(provider));
      try {
        const updated = await invoke<ProviderStatus>(command, { provider, ...extraArgs });
        setStatuses((prev) => upsertStatus(prev, updated));
        setError(null);
        return updated;
      } catch (e) {
        const message = String(e);
        setError(message);
        throw new Error(message);
      } finally {
        setInFlightProviders((prev) => {
          const next = new Set(prev);
          next.delete(provider);
          return next;
        });
      }
    },
    [],
  );

  const enableProvider = useCallback(
    async (provider: IntegrationProvider, apiKey?: string) => {
      return runProviderCommand(provider, "confirm_enable_provider", apiKey ? { apiKey } : undefined);
    },
    [runProviderCommand],
  );

  const disableProvider = useCallback(
    async (provider: IntegrationProvider) => {
      return runProviderCommand(provider, "confirm_disable_provider");
    },
    [runProviderCommand],
  );

  const hasEnabledProvider = useMemo(
    () => statuses.some((status) => status.enabled),
    [statuses],
  );

  return {
    statuses,
    loading,
    error,
    inFlightProviders,
    hasEnabledProvider,
    refresh,
    enableProvider,
    disableProvider,
  };
}
