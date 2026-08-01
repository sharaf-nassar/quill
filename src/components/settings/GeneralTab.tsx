import { useCallback, useEffect, useState } from "react";
import { getVersion } from "@tauri-apps/api/app";
import { WebviewWindow } from "@tauri-apps/api/webviewWindow";
import {
  RUNTIME_SETTINGS_DEFAULTS,
  type UseRuntimeSettingsResult,
} from "../../hooks/useRuntimeSettings";
import {
  LEARNING_SETTINGS_DEFAULTS,
  type UseLearningSettingsResult,
} from "../../hooks/useLearningSettings";
import { useToast } from "../../hooks/useToast";
import { useAppImageIntegration } from "../../hooks/useAppImageIntegration";
import SettingRow from "./SettingRow";
import Toggle from "./Toggle";

interface GeneralTabProps {
  runtime: UseRuntimeSettingsResult;
  learning: UseLearningSettingsResult;
}

function GeneralTab({ runtime, learning }: GeneralTabProps) {
  const { toast } = useToast();
  const appImage = useAppImageIntegration();
  const [version, setVersion] = useState("");

  useEffect(() => {
    let active = true;
    getVersion()
      .then((value) => {
        if (active) setVersion(value);
      })
      .catch((error: unknown) => {
        // Chrome, not data: the row stays usable, but keep the failure context.
        console.warn("Failed to read app version", error);
      });
    return () => {
      active = false;
    };
  }, []);

  const handleAlwaysOnTop = (next: boolean) => {
    void runtime.save({ ...runtime.settings, alwaysOnTop: next });
  };

  const summarize = () => {
    const live = runtime.settings.liveUsageEnabled
      ? `every ${runtime.settings.liveUsageIntervalSeconds}s`
      : "off";
    return `Live polling: ${live}`;
  };

  const handleResetAll = async () => {
    try {
      await runtime.save(RUNTIME_SETTINGS_DEFAULTS);
      await learning.save(LEARNING_SETTINGS_DEFAULTS);
      toast("info", "Settings reset to defaults");
    } catch (err) {
      toast("error", `Reset failed: ${String(err)}`);
    }
  };

  const handleOpenReleaseNotes = useCallback(async () => {
    const existing = await WebviewWindow.getByLabel("release-notes");
    if (existing) {
      await existing.show();
      await existing.setFocus();
      return;
    }
    new WebviewWindow("release-notes", {
      url: "/?view=release-notes",
      title: "Release Notes",
      width: 560,
      height: 600,
      minWidth: 380,
      minHeight: 360,
      decorations: false,
      transparent: true,
      resizable: true,
    });
  }, []);

  return (
    <div className="settings-panel">
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

      {appImage.isAppImage && (
        <SettingRow
          label="Install to applications menu"
          description="Add Quill to your desktop applications menu with an icon, and keep it auto-updating."
          control={
            appImage.integrated ? (
              <button type="button" className="settings-button" disabled>
                Installed ✓
              </button>
            ) : (
              <button
                type="button"
                className="settings-button settings-button--primary"
                onClick={() => void appImage.install()}
                disabled={appImage.installing}
              >
                {appImage.installing ? "Installing…" : "Install to applications menu"}
              </button>
            )
          }
        />
      )}

      <div className="settings-section-header">Advanced</div>
      <SettingRow
        label="Current configuration"
        description={summarize()}
        control={null}
      />
      <SettingRow
        label="Reset to defaults"
        description="Restore Quill's runtime and learning preferences to their initial values. Provider integrations, brevity blocks, learned rules, and analytics history are NOT touched."
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

      <div className="settings-section-header">Help improve Quill</div>
      <SettingRow
        label="Help improve Quill"
        description="Send anonymized crash reports. All session data, file paths, and prompt text are removed locally before transmission. Disable to send nothing."
        control={
          <Toggle
            tone={runtime.settings.crashReportingEnabled ? "on" : "off"}
            pressed={runtime.settings.crashReportingEnabled}
            disabled={runtime.saving}
            onClick={() =>
              void runtime.save({
                ...runtime.settings,
                crashReportingEnabled: !runtime.settings.crashReportingEnabled,
              })
            }
          />
        }
      />

      <div className="settings-section-header">About</div>
      <SettingRow
        label="Version"
        description={
          version
            ? `Quill v${version} — read the release notes for this and earlier builds.`
            : "Version unavailable — read the release notes for recent builds."
        }
        control={
          <button
            type="button"
            className="settings-button"
            onClick={() => void handleOpenReleaseNotes()}
            title="Open release notes"
          >
            {"What's new"}
          </button>
        }
      />
    </div>
  );
}

export default GeneralTab;
