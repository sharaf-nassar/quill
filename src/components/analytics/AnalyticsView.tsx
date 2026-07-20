import React, { useEffect, useState, Suspense } from "react";
import { useAnalyticsData } from "../../hooks/useAnalyticsData";
import TabBar, { analyticsPanelId, analyticsTabId } from "./TabBar";
import NowTab from "./NowTab";
import TrendsTab from "./TrendsTab";
import ContextSavingsTab from "./ContextSavingsTab";
import ModelsTab from "./ModelsTab";
import type {
	RangeType,
	AnalyticsTab,
	ContextPreservationStatus,
	ModelRange,
} from "../../types";

const ChartsTab = React.lazy(() => import("./ChartsTab"));

const TAB_KEY = "quill-analytics-tab";

interface AnalyticsViewProps {
	contextPreservation: ContextPreservationStatus;
	contextPreservationReady: boolean;
}

function SnapshotEmptyState() {
	return (
		<div className="analytics-empty-state">
			<svg
				className="analytics-empty-icon"
				width="32"
				height="32"
				viewBox="0 0 24 24"
				fill="none"
				stroke="currentColor"
				strokeWidth="1.5"
				strokeLinecap="round"
				strokeLinejoin="round"
				aria-hidden="true"
			>
				<circle cx="12" cy="12" r="10" />
				<polyline points="12 6 12 12 16 14" />
			</svg>
			<div className="analytics-empty-title">
				{"Collecting usage data\u2026"}
			</div>
			<div className="analytics-empty-desc">
				Analytics will appear here once enough data has been recorded. Data is
				captured every 60 seconds.
			</div>
		</div>
	);
}

interface SnapshotGateProps {
	children: React.ReactNode;
}

function SnapshotGate({ children }: SnapshotGateProps) {
	const { snapshotCount, snapshotCountReady, error } = useAnalyticsData(
		null,
		"24h",
	);
	const hasSuccessfulEmptySnapshot =
		snapshotCountReady && snapshotCount === 0 && error === null;

	return hasSuccessfulEmptySnapshot ? <SnapshotEmptyState /> : children;
}

function AnalyticsView({
	contextPreservation,
	contextPreservationReady,
}: AnalyticsViewProps) {
	const hasContextTab =
		contextPreservation.enabled || contextPreservation.hasContextSavingsEvents;
	const [activeTab, setActiveTab] = useState<AnalyticsTab>(() => {
		try {
			const saved = localStorage.getItem(TAB_KEY);
			if (
				saved === "now" ||
				saved === "trends" ||
				saved === "charts" ||
				saved === "models" ||
				(saved === "context" && (hasContextTab || !contextPreservationReady))
			) return saved;
		} catch { /* ignore */ }
		return "now";
	});
	const [hasVisitedModels, setHasVisitedModels] = useState(
		() => activeTab === "models",
	);
	const showContextTab =
		hasContextTab || (!contextPreservationReady && activeTab === "context");
	const effectiveActiveTab: AnalyticsTab =
		activeTab === "context" && contextPreservationReady && !hasContextTab
			? "now"
			: activeTab;
	const [nowRange, setNowRange] = useState<RangeType>("1h");
	const [trendsRange, setTrendsRange] = useState<RangeType>("7d");
	const [contextRange, setContextRange] = useState<RangeType>("1h");
	const [modelsRange, setModelsRange] = useState<ModelRange>("1h");
	const [chartsRange, setChartsRange] = useState<RangeType>(() => {
		try {
			const saved = localStorage.getItem("quill-charts-range");
			if (saved === "1h" || saved === "24h" || saved === "7d" || saved === "30d") return saved as RangeType;
		} catch { /* ignore */ }
		return "1h";
	});

	const handleChartsRangeChange = (r: RangeType) => {
		setChartsRange(r);
		try {
			localStorage.setItem("quill-charts-range", r);
		} catch { /* ignore */ }
	};

	const handleTabChange = (tab: AnalyticsTab) => {
		if (tab === "context" && !showContextTab) return;
		if (tab === "models") setHasVisitedModels(true);
		setActiveTab(tab);
		try {
			localStorage.setItem(TAB_KEY, tab);
		} catch { /* ignore */ }
	};

	useEffect(() => {
		if (
			!contextPreservationReady ||
			activeTab !== "context" ||
			hasContextTab
		) return;
		setActiveTab("now");
		try {
			localStorage.setItem(TAB_KEY, "now");
		} catch { /* ignore */ }
	}, [activeTab, contextPreservationReady, hasContextTab]);

	return (
		<div className="analytics-view">
			<TabBar
				activeTab={effectiveActiveTab}
				onTabChange={handleTabChange}
				showContextTab={showContextTab}
			/>
			<div
				className="analytics-tab-panel analytics-tab-panel--now"
				role="tabpanel"
				id={analyticsPanelId("now")}
				aria-labelledby={analyticsTabId("now")}
				hidden={effectiveActiveTab !== "now"}
				tabIndex={effectiveActiveTab === "now" ? 0 : -1}
			>
				{effectiveActiveTab === "now" ? (
					<SnapshotGate>
						<NowTab range={nowRange} onRangeChange={setNowRange} />
					</SnapshotGate>
				) : null}
			</div>
			<div
				className="analytics-tab-panel analytics-tab-panel--trends"
				role="tabpanel"
				id={analyticsPanelId("trends")}
				aria-labelledby={analyticsTabId("trends")}
				hidden={effectiveActiveTab !== "trends"}
				tabIndex={effectiveActiveTab === "trends" ? 0 : -1}
			>
				{effectiveActiveTab === "trends" ? (
					<SnapshotGate>
						<TrendsTab
							range={trendsRange}
							onRangeChange={setTrendsRange}
						/>
					</SnapshotGate>
				) : null}
			</div>
			<div
				className="analytics-tab-panel analytics-tab-panel--charts"
				role="tabpanel"
				id={analyticsPanelId("charts")}
				aria-labelledby={analyticsTabId("charts")}
				hidden={effectiveActiveTab !== "charts"}
				tabIndex={effectiveActiveTab === "charts" ? 0 : -1}
			>
				{effectiveActiveTab === "charts" ? (
					<SnapshotGate>
						<Suspense
							fallback={
								<div
									className="chart-skeleton"
									role="status"
									aria-busy="true"
									aria-label="Loading charts"
								/>
							}
						>
							<ChartsTab
								range={chartsRange}
								onRangeChange={handleChartsRangeChange}
							/>
						</Suspense>
					</SnapshotGate>
				) : null}
			</div>
			<div
				className="analytics-tab-panel analytics-tab-panel--models"
				role="tabpanel"
				id={analyticsPanelId("models")}
				aria-labelledby={analyticsTabId("models")}
				hidden={effectiveActiveTab !== "models"}
				tabIndex={effectiveActiveTab === "models" ? 0 : -1}
			>
				{hasVisitedModels ? (
					<ModelsTab
						range={modelsRange}
						onRangeChange={setModelsRange}
						active={effectiveActiveTab === "models"}
					/>
				) : null}
			</div>
			{showContextTab ? (
				<div
					className="analytics-tab-panel analytics-tab-panel--context"
					role="tabpanel"
					id={analyticsPanelId("context")}
					aria-labelledby={analyticsTabId("context")}
					hidden={effectiveActiveTab !== "context"}
					tabIndex={effectiveActiveTab === "context" ? 0 : -1}
				>
					{effectiveActiveTab === "context" ? (
						<ContextSavingsTab
							range={contextRange}
							onRangeChange={setContextRange}
						/>
					) : null}
				</div>
			) : null}
		</div>
	);
}

export default AnalyticsView;
