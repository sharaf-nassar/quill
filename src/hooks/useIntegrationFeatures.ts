import { useCallback, useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import type { IntegrationFeatures } from "../types";

export const INTEGRATION_FEATURES_DEFAULTS: IntegrationFeatures = {
  contextPreservation: false,
  activityTracking: true,
  contextTelemetry: true,
  brevity: false,
};

export interface UseIntegrationFeaturesResult {
  features: IntegrationFeatures;
  loading: boolean;
  saving: boolean;
  error: string | null;
  setActivityTracking: (enabled: boolean) => Promise<IntegrationFeatures>;
  setContextTelemetry: (enabled: boolean) => Promise<IntegrationFeatures>;
  setBrevity: (enabled: boolean) => Promise<IntegrationFeatures>;
  refresh: () => Promise<void>;
}

export function useIntegrationFeatures(): UseIntegrationFeaturesResult {
  const [features, setFeatures] = useState<IntegrationFeatures>(
    INTEGRATION_FEATURES_DEFAULTS,
  );
  const [loading, setLoading] = useState(true);
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const refresh = useCallback(async () => {
    setLoading(true);
    try {
      const next = await invoke<IntegrationFeatures>("get_integration_features");
      setFeatures(next);
      setError(null);
    } catch (e) {
      setError(String(e));
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    void refresh();
  }, [refresh]);

  useEffect(() => {
    const unlisten = listen<IntegrationFeatures>(
      "integration-features-updated",
      (event) => {
        setFeatures(event.payload);
      },
    );
    return () => {
      unlisten.then((fn) => fn());
    };
  }, []);

  const callSetter = useCallback(
    async (
      command:
        | "set_activity_tracking_enabled"
        | "set_context_telemetry_enabled"
        | "set_brevity_enabled",
      enabled: boolean,
    ): Promise<IntegrationFeatures> => {
      setSaving(true);
      try {
        const next = await invoke<IntegrationFeatures>(command, { enabled });
        setFeatures(next);
        setError(null);
        return next;
      } catch (e) {
        const message = String(e);
        setError(message);
        throw new Error(message);
      } finally {
        setSaving(false);
      }
    },
    [],
  );

  const setActivityTracking = useCallback(
    (enabled: boolean) => callSetter("set_activity_tracking_enabled", enabled),
    [callSetter],
  );

  const setContextTelemetry = useCallback(
    (enabled: boolean) => callSetter("set_context_telemetry_enabled", enabled),
    [callSetter],
  );

  const setBrevity = useCallback(
    (enabled: boolean) => callSetter("set_brevity_enabled", enabled),
    [callSetter],
  );

  return {
    features,
    loading,
    saving,
    error,
    setActivityTracking,
    setContextTelemetry,
    setBrevity,
    refresh,
  };
}
