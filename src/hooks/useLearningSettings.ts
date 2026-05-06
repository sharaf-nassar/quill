import { useCallback, useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import type { LearningSettings } from "../types";

export const LEARNING_SETTINGS_DEFAULTS: LearningSettings = {
  enabled: false,
  trigger_mode: "on-demand",
  periodic_minutes: 180,
  min_observations: 50,
  min_confidence: 0.95,
};

export interface UseLearningSettingsResult {
  settings: LearningSettings;
  loading: boolean;
  saving: boolean;
  error: string | null;
  save: (next: LearningSettings) => Promise<void>;
  refresh: () => Promise<void>;
}

export function useLearningSettings(): UseLearningSettingsResult {
  const [settings, setSettings] = useState<LearningSettings>(LEARNING_SETTINGS_DEFAULTS);
  const [loading, setLoading] = useState(true);
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const refresh = useCallback(async () => {
    setLoading(true);
    try {
      const next = await invoke<LearningSettings>("get_learning_settings");
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

  const save = useCallback(
    async (next: LearningSettings) => {
      setSaving(true);
      try {
        await invoke("set_learning_settings", { settings: next });
        setSettings(next);
        setError(null);
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

  return { settings, loading, saving, error, save, refresh };
}
