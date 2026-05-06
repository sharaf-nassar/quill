import type { UseRuntimeSettingsResult } from "../../hooks/useRuntimeSettings";
import { useToast } from "../../hooks/useToast";
import SettingRow from "./SettingRow";
import Toggle from "./Toggle";
import { clampInt } from "./utils";

interface PerformanceTabProps {
  runtime: UseRuntimeSettingsResult;
}

export function PerformanceTab({ runtime }: PerformanceTabProps) {
  const { toast } = useToast();
  const { settings, saving } = runtime;

  const update = (patch: Partial<typeof settings>) => {
    void runtime.save({ ...settings, ...patch }).catch((err) => toast("error", String(err)));
  };

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
    </div>
  );
}

export default PerformanceTab;
