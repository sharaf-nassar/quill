import type { AnalyticsTab } from "../../types";

interface TabBarProps {
	activeTab: AnalyticsTab;
	onTabChange: (tab: AnalyticsTab) => void;
}

const TABS: { key: AnalyticsTab; label: string; color: string }[] = [
	{ key: "now", label: "Now", color: "#58a6ff" },
	{ key: "trends", label: "Trends", color: "#a78bfa" },
	{ key: "charts", label: "Charts", color: "#34d399" },
];

function TabBar({ activeTab, onTabChange }: TabBarProps) {
	return (
		<div className="analytics-tab-bar">
			{TABS.map((tab) => (
				<button
					key={tab.key}
					className={`analytics-tab${activeTab === tab.key ? " active" : ""}`}
					style={
						activeTab === tab.key
							? { borderBottomColor: tab.color }
							: undefined
					}
					onClick={() => onTabChange(tab.key)}
					aria-pressed={activeTab === tab.key}
				>
					{tab.label}
				</button>
			))}
		</div>
	);
}

export default TabBar;
