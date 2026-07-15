import { useEffect, useLayoutEffect, useRef, type KeyboardEvent } from "react";
import type { AnalyticsTab } from "../../types";

interface TabBarProps {
	activeTab: AnalyticsTab;
	onTabChange: (tab: AnalyticsTab) => void;
	showContextTab: boolean;
}

interface TabDefinition {
	key: AnalyticsTab;
	label: string;
}

const TABS: readonly TabDefinition[] = [
	{ key: "now", label: "Now" },
	{ key: "trends", label: "Trends" },
	{ key: "charts", label: "Charts" },
	{ key: "models", label: "Models" },
	{ key: "context", label: "Context" },
];

export function analyticsTabId(tab: AnalyticsTab): string {
	return `analytics-tab-${tab}`;
}

export function analyticsPanelId(tab: AnalyticsTab): string {
	return `analytics-panel-${tab}`;
}

function TabBar({ activeTab, onTabChange, showContextTab }: TabBarProps) {
	const tabListRef = useRef<HTMLDivElement | null>(null);
	const tabRefs = useRef<Array<HTMLButtonElement | null>>([]);
	const focusedTabRef = useRef<AnalyticsTab | null>(null);
	const previousShowContextTabRef = useRef(showContextTab);
	const tabs = showContextTab
		? TABS
		: TABS.filter((tab) => tab.key !== "context");
	const selectedTab = tabs.some((tab) => tab.key === activeTab)
		? activeTab
		: tabs[0].key;

	useEffect(() => {
		const clearFocusOwnershipOutsideTabList = (event: FocusEvent) => {
			const target = event.target;
			if (
				target instanceof Node &&
				tabListRef.current?.contains(target)
			) return;

			focusedTabRef.current = null;
		};

		document.addEventListener(
			"focusin",
			clearFocusOwnershipOutsideTabList,
			true,
		);
		return () => {
			document.removeEventListener(
				"focusin",
				clearFocusOwnershipOutsideTabList,
				true,
			);
		};
	}, []);

	useLayoutEffect(() => {
		const contextWasRemoved =
			previousShowContextTabRef.current && !showContextTab;
		previousShowContextTabRef.current = showContextTab;

		if (!contextWasRemoved || focusedTabRef.current !== "context") return;

		focusedTabRef.current = "now";
		tabRefs.current[0]?.focus();
	}, [showContextTab]);

	const activateTab = (index: number) => {
		const tab = tabs[index];
		if (!tab) return;

		onTabChange(tab.key);
		tabRefs.current[index]?.focus();
	};

	const handleKeyDown = (
		event: KeyboardEvent<HTMLButtonElement>,
		index: number,
	) => {
		if (event.altKey || event.ctrlKey || event.metaKey) return;

		let nextIndex: number | null = null;
		switch (event.key) {
			case "ArrowLeft":
				nextIndex = (index - 1 + tabs.length) % tabs.length;
				break;
			case "ArrowRight":
				nextIndex = (index + 1) % tabs.length;
				break;
			case "Home":
				nextIndex = 0;
				break;
			case "End":
				nextIndex = tabs.length - 1;
				break;
			default:
				return;
		}

		event.preventDefault();
		activateTab(nextIndex);
	};

	return (
		<div
			ref={tabListRef}
			className="analytics-tab-bar"
			role="tablist"
			aria-label="Analytics views"
			aria-orientation="horizontal"
		>
			{tabs.map((tab, index) => {
				const selected = selectedTab === tab.key;

				return (
					<button
						key={tab.key}
						ref={(element) => {
							tabRefs.current[index] = element;
						}}
						type="button"
						id={analyticsTabId(tab.key)}
						className={`analytics-tab${selected ? " active" : ""}`}
						role="tab"
						aria-controls={analyticsPanelId(tab.key)}
						aria-selected={selected}
						tabIndex={selected ? 0 : -1}
						onClick={() => onTabChange(tab.key)}
						onFocus={() => {
							focusedTabRef.current = tab.key;
						}}
						onKeyDown={(event) => handleKeyDown(event, index)}
					>
						{tab.label}
					</button>
				);
			})}
		</div>
	);
}

export default TabBar;
