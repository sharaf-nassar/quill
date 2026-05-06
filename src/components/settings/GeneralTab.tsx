import type { LayoutMode, TimeMode } from "../../types";
import type { UiPrefs } from "../../hooks/useUiPrefs";
import {
  RUNTIME_SETTINGS_DEFAULTS,
  type UseRuntimeSettingsResult,
} from "../../hooks/useRuntimeSettings";
import {
  LEARNING_SETTINGS_DEFAULTS,
  type UseLearningSettingsResult,
} from "../../hooks/useLearningSettings";
import { useToast } from "../../hooks/useToast";
import SettingRow from "./SettingRow";
import Toggle from "./Toggle";

interface GeneralTabProps {
  prefs: UiPrefs;
  onUpdatePrefs: (patch: Partial<UiPrefs>) => void;
  runtime: UseRuntimeSettingsResult;
  learning: UseLearningSettingsResult;
  onResetUiPrefs: () => Promise<void>;
}

export function GeneralTab({
  prefs,
  onUpdatePrefs,
  runtime,
  learning,
  onResetUiPrefs,
}: GeneralTabProps) {
  const { toast } = useToast();

  const handleAlwaysOnTop = (next: boolean) => {
    void runtime.save({ ...runtime.settings, alwaysOnTop: next });
  };

  const summarize = () => {
    const live = runtime.settings.liveUsageEnabled
      ? `every ${runtime.settings.liveUsageIntervalSeconds}s`
      : "off";
    const plugins = runtime.settings.pluginUpdatesEnabled
      ? `every ${runtime.settings.pluginUpdatesIntervalHours}h`
      : "off";
    return `Live polling: ${live} • Plugin updates: ${plugins} • Layout: ${prefs.layoutMode}`;
  };

  const handleResetAll = async () => {
    try {
      await runtime.save(RUNTIME_SETTINGS_DEFAULTS);
      await learning.save(LEARNING_SETTINGS_DEFAULTS);
      await onResetUiPrefs();
      toast("info", "Settings reset to defaults");
    } catch (err) {
      toast("error", `Reset failed: ${String(err)}`);
    }
  };

  return (
    <div className="settings-panel">
      <SettingRow
        label="Layout"
        description="Stacked places Live above Analytics; side-by-side puts them left/right."
        control={
          <div className="settings-segmented">
            {(["stacked", "side-by-side"] as LayoutMode[]).map((mode) => (
              <button
                key={mode}
                type="button"
                className={`settings-segment${prefs.layoutMode === mode ? " settings-segment--active" : ""}`}
                onClick={() => onUpdatePrefs({ layoutMode: mode })}
              >
                {mode === "stacked" ? "Stacked" : "Side-by-side"}
              </button>
            ))}
          </div>
        }
      />

      <SettingRow
        label="Time visualization"
        description="How Live Usage renders the bucket reset clock."
        control={
          <div className="settings-segmented">
            {(["marker", "dual", "background"] as TimeMode[]).map((mode) => (
              <button
                key={mode}
                type="button"
                className={`settings-segment${prefs.timeMode === mode ? " settings-segment--active" : ""}`}
                onClick={() => onUpdatePrefs({ timeMode: mode })}
              >
                {mode}
              </button>
            ))}
          </div>
        }
      />

      <SettingRow
        label="Live panel"
        description="Show the live-usage pane in the main window."
        control={
          <Toggle
            tone={prefs.showLive ? "on" : "off"}
            pressed={prefs.showLive}
            onClick={() => onUpdatePrefs({ showLive: !prefs.showLive })}
          />
        }
      />

      <SettingRow
        label="Analytics panel"
        description="Show the analytics dashboard pane in the main window."
        control={
          <Toggle
            tone={prefs.showAnalytics ? "on" : "off"}
            pressed={prefs.showAnalytics}
            onClick={() => onUpdatePrefs({ showAnalytics: !prefs.showAnalytics })}
          />
        }
      />

      <SettingRow
        label="Always on top"
        description="Pin the main window above other windows."
        control={
          <Toggle
            tone={runtime.settings.alwaysOnTop ? "on" : "off"}
            pressed={runtime.settings.alwaysOnTop}
            disabled={runtime.saving}
            onClick={() => handleAlwaysOnTop(!runtime.settings.alwaysOnTop)}
          />
        }
      />

      <div className="settings-section-header">Advanced</div>
      <SettingRow
        label="Current configuration"
        description={summarize()}
        control={null}
      />
      <SettingRow
        label="Reset to defaults"
        description="Restore Quill's runtime, learning, and UI preferences to their initial values. Provider integrations, brevity blocks, learned rules, and analytics history are NOT touched."
        control={
          <button
            type="button"
            className="settings-button settings-button--danger"
            onClick={() => void handleResetAll()}
          >
            Reset
          </button>
        }
      />
      <div className="settings-prose">
        <p>
          For deeper resets — re-running the integration installer, wiping analytics, or
          rebuilding the session search index — disable and re-enable the relevant provider
          from the Integrations tab. Storage cleanup commands are not exposed here.
        </p>
      </div>
    </div>
  );
}

export default GeneralTab;
