import type { PluginsTab } from "../../types";

const TABS: { id: PluginsTab; label: string }[] = [
	{ id: "installed", label: "Installed" },
	{ id: "browse", label: "Browse" },
	{ id: "marketplaces", label: "Marketplaces" },
	{ id: "updates", label: "Updates" },
];

interface PluginsTabsProps {
	activeTab: PluginsTab;
	onTabChange: (tab: PluginsTab) => void;
	updateCount: number;
}

function PluginsTabs({ activeTab, onTabChange, updateCount }: PluginsTabsProps) {
	return (
		<div className="plugins-tab-bar">
			{TABS.map((tab) => (
				<button
					key={tab.id}
					className={`plugins-tab-bar__tab${activeTab === tab.id ? " plugins-tab-bar__tab--active" : ""}`}
					onClick={() => onTabChange(tab.id)}
				>
					{tab.label}
					{tab.id === "updates" && updateCount > 0 && (
						<span className="plugins-tab-bar__badge">{updateCount}</span>
					)}
				</button>
			))}
		</div>
	);
}

export default PluginsTabs;
