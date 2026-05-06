import { useCallback, useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import type { RuntimeSettings } from "../types";

export const RUNTIME_SETTINGS_DEFAULTS: RuntimeSettings = {
  liveUsageEnabled: true,
  liveUsageIntervalSeconds: 180,
  pluginUpdatesEnabled: true,
  pluginUpdatesIntervalHours: 4,
  ruleWatcherEnabled: true,
  alwaysOnTop: false,
};

export interface UseRuntimeSettingsResult {
  settings: RuntimeSettings;
  loading: boolean;
  saving: boolean;
  error: string | null;
  save: (next: RuntimeSettings) => Promise<RuntimeSettings>;
  refresh: () => Promise<void>;
}

export function useRuntimeSettings(): UseRuntimeSettingsResult {
  const [settings, setSettings] = useState<RuntimeSettings>(RUNTIME_SETTINGS_DEFAULTS);
  const [loading, setLoading] = useState(true);
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const refresh = useCallback(async () => {
    setLoading(true);
    try {
      const next = await invoke<RuntimeSettings>("get_runtime_settings");
      setSettings(next);
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
    const unlisten = listen<RuntimeSettings>("runtime-settings-updated", (event) => {
      setSettings(event.payload);
    });
    return () => {
      unlisten.then((fn) => fn());
    };
  }, []);

  const save = useCallback(async (next: RuntimeSettings) => {
    setSaving(true);
    try {
      const resolved = await invoke<RuntimeSettings>("set_runtime_settings", {
        settings: next,
      });
      setSettings(resolved);
      setError(null);
      return resolved;
    } catch (e) {
      const message = String(e);
      setError(message);
      throw new Error(message);
    } finally {
      setSaving(false);
    }
  }, []);

  return { settings, loading, saving, error, save, refresh };
}
