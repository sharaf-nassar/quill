import type { LearningSettings, LearningTriggerMode } from "../../types";
import type { UseLearningSettingsResult } from "../../hooks/useLearningSettings";
import type { UseRuntimeSettingsResult } from "../../hooks/useRuntimeSettings";
import { useToast } from "../../hooks/useToast";
import SettingRow from "./SettingRow";
import Toggle from "./Toggle";
import { clampFloat, clampInt } from "./utils";

interface LearningTabProps {
  learning: UseLearningSettingsResult;
  runtime: UseRuntimeSettingsResult;
}

const TRIGGER_MODES: ReadonlyArray<{ id: LearningTriggerMode; label: string }> = [
  { id: "on-demand", label: "On-demand" },
  { id: "periodic", label: "Periodic" },
];

export function LearningTab({ learning, runtime }: LearningTabProps) {
  const { toast } = useToast();
  const { settings, saving } = learning;

  const update = (patch: Partial<LearningSettings>) => {
    const next: LearningSettings = { ...settings, ...patch };
    void learning.save(next).catch((err) => toast("error", String(err)));
  };

  const handleRuleWatcher = (next: boolean) => {
    void runtime
      .save({ ...runtime.settings, ruleWatcherEnabled: next })
      .catch((err) => toast("error", String(err)));
  };

  return (
    <div className="settings-panel">
      <SettingRow
        label="Trigger mode"
        description="On-demand runs analysis only when triggered. Periodic runs on a schedule when at least the minimum number of new observations exist."
        control={
          <div className="settings-segmented">
            {TRIGGER_MODES.map((mode) => (
              <button
                key={mode.id}
                type="button"
                className={`settings-segment${settings.trigger_mode === mode.id ? " settings-segment--active" : ""}`}
                onClick={() =>
                  update({
                    trigger_mode: mode.id,
                    enabled: mode.id === "periodic" ? settings.enabled : false,
                  })
                }
              >
                {mode.label}
              </button>
            ))}
          </div>
        }
      />

      {settings.trigger_mode === "periodic" && (
        <>
          <SettingRow
            label="Periodic enabled"
            description="Run analysis automatically on the interval below."
            control={
              <Toggle
                tone={settings.enabled ? "on" : "off"}
                pressed={settings.enabled}
                disabled={saving}
                onClick={() => update({ enabled: !settings.enabled })}
              />
            }
          />

          <SettingRow
            label="Interval (minutes)"
            description="Minimum gap between automatic analysis runs."
            control={
              <input
                type="number"
                className="settings-input settings-input--narrow"
                min={5}
                max={1440}
                value={settings.periodic_minutes}
                onChange={(e) =>
                  update({
                    periodic_minutes: clampInt(
                      parseInt(e.target.value, 10),
                      5,
                      1440,
                    ),
                  })
                }
              />
            }
          />
        </>
      )}

      <SettingRow
        label="Min observations"
        description="Minimum new observations required before analysis runs."
        control={
          <input
            type="number"
            className="settings-input settings-input--narrow"
            min={5}
            max={5000}
            value={settings.min_observations}
            onChange={(e) =>
              update({
                min_observations: clampInt(
                  parseInt(e.target.value, 10),
                  5,
                  5000,
                ),
              })
            }
          />
        }
      />

      <SettingRow
        label="Min confidence"
        description="Wilson-score floor for auto-promoting a discovered rule (0.5 – 1.0)."
        control={
          <input
            type="number"
            className="settings-input settings-input--narrow"
            min={0.5}
            max={1}
            step={0.01}
            value={settings.min_confidence}
            onChange={(e) =>
              update({
                min_confidence: clampFloat(parseFloat(e.target.value), 0.5, 1),
              })
            }
          />
        }
      />

      <div className="settings-section-header">Rule watcher</div>
      <SettingRow
        label="Watch learned-rule directories"
        description="Re-index rule files on filesystem changes. Disabling means manually edited rules will not be picked up until the next app launch. Takes effect after restart."
        control={
          <Toggle
            tone={runtime.settings.ruleWatcherEnabled ? "on" : "off"}
            pressed={runtime.settings.ruleWatcherEnabled}
            disabled={runtime.saving}
            onClick={() => handleRuleWatcher(!runtime.settings.ruleWatcherEnabled)}
          />
        }
      />
    </div>
  );
}

export default LearningTab;
