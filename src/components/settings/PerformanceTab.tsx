import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { useCallback, useEffect, useState } from "react";
import type { UseRuntimeSettingsResult } from "../../hooks/useRuntimeSettings";
import { useToast } from "../../hooks/useToast";
import SettingRow from "./SettingRow";
import Toggle from "./Toggle";
import { clampInt } from "./utils";

interface PerformanceTabProps {
  runtime: UseRuntimeSettingsResult;
}

type CompactDatabaseProgress = {
  phase: string;
  pct?: number;
};

type CompactDatabaseResult = {
  status: "completed" | "skipped";
  reason?: string;
  bytes_before: number;
  bytes_after: number;
};

function formatBytes(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`;
  const units = ["KB", "MB", "GB", "TB"];
  const index = Math.min(Math.floor(Math.log(bytes) / Math.log(1024)), units.length);
  const value = bytes / 1024 ** index;
  return `${value.toFixed(value >= 10 || index === 0 ? 0 : 1)} ${units[index - 1]}`;
}

export function PerformanceTab({ runtime }: PerformanceTabProps) {
  const { toast } = useToast();
  const { settings, saving } = runtime;
  const [compactionProgress, setCompactionProgress] =
    useState<CompactDatabaseProgress | null>(null);
  const [compactionResult, setCompactionResult] =
    useState<CompactDatabaseResult | null>(null);

  const finishCompaction = useCallback((result: CompactDatabaseResult) => {
    setCompactionProgress(null);
    setCompactionResult(result);
  }, []);

  useEffect(() => {
    const unlistenProgress = listen<CompactDatabaseProgress>(
      "compact-database-progress",
      ({ payload }) => {
        setCompactionResult(null);
        setCompactionProgress(payload);
      },
    );
    const unlistenFinished = listen<CompactDatabaseResult>(
      "compact-database-finished",
      ({ payload }) => finishCompaction(payload),
    );

    return () => {
      void Promise.all([unlistenProgress, unlistenFinished]).then((unlisteners) => {
        unlisteners.forEach((unlisten) => unlisten());
      });
    };
  }, [finishCompaction]);

  const update = (patch: Partial<typeof settings>) => {
    void runtime.save({ ...settings, ...patch }).catch((err) => toast("error", String(err)));
  };

  const compactDatabase = useCallback(async () => {
    setCompactionResult(null);
    setCompactionProgress({ phase: "Starting" });
    try {
      finishCompaction(await invoke<CompactDatabaseResult>("compact_database"));
    } catch (err) {
      setCompactionProgress(null);
      toast("error", String(err));
    }
  }, [finishCompaction, toast]);

  const compactionDescription = compactionProgress
    ? `Compacting: ${compactionProgress.phase}${
        compactionProgress.pct == null ? "" : ` · ${compactionProgress.pct}%`
      }`
    : compactionResult?.status === "skipped"
      ? `Skipped: ${compactionResult.reason ?? "the database is not ready to compact."}`
      : compactionResult?.status === "completed"
        ? `Complete: ${formatBytes(compactionResult.bytes_before)} → ${formatBytes(
            compactionResult.bytes_after,
          )}`
        : "Reclaim unused SQLite pages. Quill pauses ingest while maintenance runs.";

  return (
    <div className="settings-panel">
      <div className="settings-section-header">Live usage polling</div>
      <SettingRow
        label="Background refresh"
        description="When ON, Quill refreshes Live Usage in the background even when the main window is hidden, so the tray indicator stays current."
        control={
          <Toggle
            tone={settings.liveUsageEnabled ? "on" : "off"}
            pressed={settings.liveUsageEnabled}
            disabled={saving}
            onClick={() => update({ liveUsageEnabled: !settings.liveUsageEnabled })}
          />
        }
      />
      <SettingRow
        label="Refresh interval (seconds)"
        description="Range 60–600. Lower values feel more responsive but consume more CPU and provider quota."
        control={
          <input
            type="number"
            className="settings-input settings-input--narrow"
            min={60}
            max={600}
            step={30}
            value={settings.liveUsageIntervalSeconds}
            onChange={(e) =>
              update({
                liveUsageIntervalSeconds: clampInt(
                  parseInt(e.target.value, 10),
                  60,
                  600,
                ),
              })
            }
            disabled={!settings.liveUsageEnabled || saving}
          />
        }
      />

      <div className="settings-section-header">Plugin update checker</div>
      <SettingRow
        label="Check for plugin updates"
        description="When ON, Quill polls Claude marketplaces in the background and surfaces an update badge on the plugins button."
        control={
          <Toggle
            tone={settings.pluginUpdatesEnabled ? "on" : "off"}
            pressed={settings.pluginUpdatesEnabled}
            disabled={saving}
            onClick={() =>
              update({ pluginUpdatesEnabled: !settings.pluginUpdatesEnabled })
            }
          />
        }
      />
      <SettingRow
        label="Check interval (hours)"
        description="Range 1–24."
        control={
          <input
            type="number"
            className="settings-input settings-input--narrow"
            min={1}
            max={24}
            value={settings.pluginUpdatesIntervalHours}
            onChange={(e) =>
              update({
                pluginUpdatesIntervalHours: clampInt(
                  parseInt(e.target.value, 10),
                  1,
                  24,
                ),
              })
            }
            disabled={!settings.pluginUpdatesEnabled || saving}
          />
        }
      />

      <div className="settings-section-header">Database maintenance</div>
      <SettingRow
        label="Compact database"
        description={compactionDescription}
        control={
          <button
            type="button"
            className="settings-button settings-button--compact"
            disabled={compactionProgress != null}
            onClick={() => void compactDatabase()}
          >
            {compactionProgress ? "Compacting…" : "Compact"}
          </button>
        }
      />
    </div>
  );
}

export default PerformanceTab;
