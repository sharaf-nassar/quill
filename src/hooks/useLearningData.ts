import { useState, useEffect, useCallback } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { useToast } from "./useToast";
import type {
  IntegrationProvider,
  LearningSettings,
  LearnedRule,
  LearningRun,
  LearningLogEvent,
  ProviderFilter,
  ToolCount,
} from "../types";

function filterToProvider(
  providerFilter: ProviderFilter,
): IntegrationProvider | null {
  return providerFilter === "all" ? null : providerFilter;
}

export function useLearningData(providerFilter: ProviderFilter = "all") {
  const { toast } = useToast();
  const [settings, setSettings] = useState<LearningSettings>({
    enabled: false,
    trigger_mode: "on-demand",
    periodic_minutes: 180,
    min_observations: 50,
    min_confidence: 0.95,
  });
  const [rules, setRules] = useState<LearnedRule[]>([]);
  const [runs, setRuns] = useState<LearningRun[]>([]);
  const [observationCount, setObservationCount] = useState(0);
  const [unanalyzedCount, setUnanalyzedCount] = useState(0);
  const [topTools, setTopTools] = useState<ToolCount[]>([]);
  const [sparkline, setSparkline] = useState<number[]>([]);
  const [liveLogs, setLiveLogs] = useState<Record<number, string[]>>({});
  const [loading, setLoading] = useState(true);

  const refresh = useCallback(async () => {
    setLoading(true);
    try {
      const provider = filterToProvider(providerFilter);
      const [s, r, ru, oc, uc, tt, sp] = await Promise.all([
        invoke<LearningSettings>("get_learning_settings"),
        invoke<LearnedRule[]>("get_learned_rules", { provider }),
        invoke<LearningRun[]>("get_learning_runs", { limit: 10, provider }),
        invoke<number>("get_observation_count", { provider }),
        invoke<number>("get_unanalyzed_observation_count", { provider }),
        invoke<ToolCount[]>("get_top_tools", { limit: 5, days: 30, provider }),
        invoke<number[]>("get_observation_sparkline", { provider }),
      ]);
      setSettings(s);
      setRules(r);
      setRuns(ru);
      setObservationCount(oc);
      setUnanalyzedCount(uc);
      setTopTools(tt);
      setSparkline(sp);

      // Clean up liveLogs for runs that are no longer running
      const runningIds = new Set(ru.filter((r) => r.status === "running").map((r) => r.id));
      setLiveLogs((prev) => {
        const next: Record<number, string[]> = {};
        for (const [id, logs] of Object.entries(prev)) {
          if (runningIds.has(Number(id))) {
            next[Number(id)] = logs;
          }
        }
        return next;
      });
    } catch (e) {
      toast("error", `Failed to load learning data: ${e}`);
    } finally {
      setLoading(false);
    }
  }, [providerFilter, toast]);

  useEffect(() => {
    refresh();
    const interval = setInterval(refresh, 10_000);
    return () => clearInterval(interval);
  }, [refresh]);

  useEffect(() => {
    const unlisten = listen("learning-updated", () => {
      refresh();
    });
    return () => {
      unlisten.then((fn) => fn());
    };
  }, [refresh]);

  useEffect(() => {
    const unlisten = listen<LearningLogEvent>("learning-log", (event) => {
      const { run_id, message } = event.payload;
      setLiveLogs((prev) => ({
        ...prev,
        [run_id]: [...(prev[run_id] || []), message],
      }));
    });
    return () => {
      unlisten.then((fn) => fn());
    };
  }, []);

  const updateSettings = useCallback(
    async (next: LearningSettings) => {
      setSettings(next);
      try {
        await invoke("set_learning_settings", { settings: next });
      } catch (e) {
        toast("error", `Failed to save learning settings: ${e}`);
        refresh();
      }
    },
    [refresh, toast],
  );

  const triggerAnalysis = useCallback(async () => {
    try {
      await invoke("trigger_analysis", {
        provider: filterToProvider(providerFilter),
      });
      await refresh();
    } catch (e) {
      toast("warning", String(e));
    }
  }, [providerFilter, refresh, toast]);

  const deleteRule = useCallback(
    async (name: string) => {
      try {
        await invoke("delete_learned_rule", { name });
        await refresh();
      } catch (e) {
        toast("error", `Failed to delete rule: ${e}`);
      }
    },
    [refresh, toast],
  );

  const promoteRule = useCallback(
	async (name: string) => {
		try {
			await invoke("promote_learned_rule", { name });
			await refresh();
		} catch (e) {
			toast("error", `Failed to promote rule: ${e}`);
		}
	},
	[refresh, toast],
);

  // Derive analyzing state from runs data
  const analyzing = runs.some((r) => r.status === "running");

  return {
    settings,
    rules,
    runs,
    observationCount,
    unanalyzedCount,
    topTools,
    sparkline,
    analyzing,
    liveLogs,
    loading,
    updateSettings,
    triggerAnalysis,
    deleteRule,
    promoteRule,
    refresh,
  };
}
