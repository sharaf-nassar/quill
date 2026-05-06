export type SettingsTabId =
  | "general"
  | "integrations"
  | "context"
  | "learning"
  | "performance";

interface SettingsTabsProps {
  active: SettingsTabId;
  onChange: (id: SettingsTabId) => void;
}

const TABS: ReadonlyArray<{ id: SettingsTabId; label: string }> = [
  { id: "general", label: "General" },
  { id: "integrations", label: "Integrations" },
  { id: "context", label: "Context" },
  { id: "learning", label: "Learning" },
  { id: "performance", label: "Performance" },
];

export function SettingsTabs({ active, onChange }: SettingsTabsProps) {
  return (
    <nav className="settings-tabs" role="tablist" aria-label="Settings sections">
      {TABS.map((tab) => (
        <button
          key={tab.id}
          role="tab"
          type="button"
          aria-selected={active === tab.id}
          className={`settings-tab${active === tab.id ? " settings-tab--active" : ""}`}
          onClick={() => onChange(tab.id)}
        >
          {tab.label}
        </button>
      ))}
    </nav>
  );
}

export default SettingsTabs;
