import { useCallback, useState } from "react";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { useIntegrations } from "../hooks/useIntegrations";
import { useIntegrationFeatures } from "../hooks/useIntegrationFeatures";
import { useRuntimeSettings } from "../hooks/useRuntimeSettings";
import { useLearningSettings } from "../hooks/useLearningSettings";
import SettingsTabs, {
  type SettingsTabId,
} from "../components/settings/SettingsTabs";
import GeneralTab from "../components/settings/GeneralTab";
import IntegrationsTab from "../components/settings/IntegrationsTab";
import ContextTab from "../components/settings/ContextTab";
import LearningTab from "../components/settings/LearningTab";
import PerformanceTab from "../components/settings/PerformanceTab";
import "../styles/settings.css";

function SettingsWindowView() {
  const integrations = useIntegrations();
  const features = useIntegrationFeatures();
  const runtime = useRuntimeSettings();
  const learning = useLearningSettings();

  const [active, setActive] = useState<SettingsTabId>("general");

  const handleClose = useCallback(async () => {
    await getCurrentWindow().close();
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
          <GeneralTab runtime={runtime} learning={learning} />
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
