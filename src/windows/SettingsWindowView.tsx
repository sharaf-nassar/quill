import { useCallback, useState } from "react";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { emit } from "@tauri-apps/api/event";
import { useIntegrations } from "../hooks/useIntegrations";
import { useIntegrationFeatures } from "../hooks/useIntegrationFeatures";
import { useRuntimeSettings } from "../hooks/useRuntimeSettings";
import { useLearningSettings } from "../hooks/useLearningSettings";
import { useUiPrefs, UI_PREFS_EVENT, type UiPrefs } from "../hooks/useUiPrefs";
import SettingsTabs, {
  type SettingsTabId,
} from "../components/settings/SettingsTabs";
import GeneralTab from "../components/settings/GeneralTab";
import IntegrationsTab from "../components/settings/IntegrationsTab";
import ContextTab from "../components/settings/ContextTab";
import LearningTab from "../components/settings/LearningTab";
import PerformanceTab from "../components/settings/PerformanceTab";
import "../styles/settings.css";

const DEFAULT_UI_PREFS: UiPrefs = {
  layoutMode: "stacked",
  timeMode: "marker",
  showLive: true,
  showAnalytics: false,
};

function SettingsWindowView() {
  const integrations = useIntegrations();
  const features = useIntegrationFeatures();
  const runtime = useRuntimeSettings();
  const learning = useLearningSettings();
  const { prefs, update: updatePrefs } = useUiPrefs();

  const [active, setActive] = useState<SettingsTabId>("general");

  const handleClose = useCallback(async () => {
    await getCurrentWindow().close();
  }, []);

  const resetUiPrefs = useCallback(async () => {
    try {
      localStorage.setItem("quill-layout-mode", DEFAULT_UI_PREFS.layoutMode);
      localStorage.setItem("quill-time-mode", DEFAULT_UI_PREFS.timeMode);
      localStorage.setItem("quill-show-live", String(DEFAULT_UI_PREFS.showLive));
      localStorage.setItem(
        "quill-show-analytics",
        String(DEFAULT_UI_PREFS.showAnalytics),
      );
    } catch {
      /* ignore */
    }
    await emit(UI_PREFS_EVENT, DEFAULT_UI_PREFS);
  }, []);

  return (
    <div className="settings-window">
      <div className="settings-window-titlebar" data-tauri-drag-region>
        <span className="settings-window-title" data-tauri-drag-region>
          Settings
        </span>
        <button
          type="button"
          className="settings-window-close"
          onClick={() => void handleClose()}
          aria-label="Close"
        >
          &times;
        </button>
      </div>
      <SettingsTabs active={active} onChange={setActive} />
      <div className="settings-content">
        {active === "general" && (
          <GeneralTab
            prefs={prefs}
            onUpdatePrefs={updatePrefs}
            runtime={runtime}
            learning={learning}
            onResetUiPrefs={resetUiPrefs}
          />
        )}
        {active === "integrations" && (
          <IntegrationsTab integrations={integrations} features={features} />
        )}
        {active === "context" && (
          <ContextTab integrations={integrations} features={features} />
        )}
        {active === "learning" && (
          <LearningTab learning={learning} runtime={runtime} />
        )}
        {active === "performance" && <PerformanceTab runtime={runtime} />}
      </div>
    </div>
  );
}

export default SettingsWindowView;
