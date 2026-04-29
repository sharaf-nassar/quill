import React, { useEffect, useState, Suspense } from "react";
import { useAnalyticsData } from "../../hooks/useAnalyticsData";
import TabBar from "./TabBar";
import NowTab from "./NowTab";
import TrendsTab from "./TrendsTab";
import ContextSavingsTab from "./ContextSavingsTab";
import type { RangeType, AnalyticsTab, ContextPreservationStatus } from "../../types";

const ChartsTab = React.lazy(() => import("./ChartsTab"));

const TAB_KEY = "quill-analytics-tab";

interface AnalyticsViewProps {
	contextPreservation: ContextPreservationStatus;
}

function AnalyticsView({ contextPreservation }: AnalyticsViewProps) {
	const showContextTab =
		contextPreservation.enabled || contextPreservation.hasContextSavingsEvents;
	const [activeTab, setActiveTab] = useState<AnalyticsTab>(() => {
		try {
			const saved = localStorage.getItem(TAB_KEY);
			if (
				saved === "now" ||
				saved === "trends" ||
				saved === "charts" ||
				(saved === "context" && showContextTab)
			) return saved;
		} catch { /* ignore */ }
		return "now";
	});
	const [nowRange, setNowRange] = useState<RangeType>("1h");
	const [trendsRange, setTrendsRange] = useState<RangeType>("7d");
	const [contextRange, setContextRange] = useState<RangeType>("1h");
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
		setActiveTab(tab);
		try {
			localStorage.setItem(TAB_KEY, tab);
		} catch { /* ignore */ }
	};

	const { snapshotCount, loading } = useAnalyticsData(null, "24h");

	useEffect(() => {
		if (activeTab !== "context" || showContextTab) return;
		setActiveTab("now");
		try {
			localStorage.setItem(TAB_KEY, "now");
		} catch { /* ignore */ }
	}, [activeTab, showContextTab]);

	if (snapshotCount === 0 && !loading) {
		return (
			<div className="analytics-view">
				<TabBar
					activeTab={activeTab}
					onTabChange={handleTabChange}
					showContextTab={showContextTab}
				/>
				{activeTab === "context" && showContextTab ? (
				<ContextSavingsTab
					range={contextRange}
					onRangeChange={setContextRange}
				/>
			) : (
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
						Analytics will appear here once enough data has been recorded. Data
						is captured every 60 seconds.
					</div>
				</div>
			)}
			</div>
		);
	}

	return (
		<div className="analytics-view">
			<TabBar
				activeTab={activeTab}
				onTabChange={handleTabChange}
				showContextTab={showContextTab}
			/>
			{activeTab === "now" && (
				<NowTab
					range={nowRange}
					onRangeChange={setNowRange}
				/>
			)}
			{activeTab === "trends" && (
				<TrendsTab
					range={trendsRange}
					onRangeChange={setTrendsRange}
				/>
			)}
			{activeTab === "charts" && (
				<Suspense fallback={<div className="chart-skeleton" style={{ height: 200 }} />}>
					<ChartsTab
						range={chartsRange}
						onRangeChange={handleChartsRangeChange}
					/>
				</Suspense>
			)}
			{activeTab === "context" && showContextTab && (
				<ContextSavingsTab
					range={contextRange}
					onRangeChange={setContextRange}
				/>
			)}
		</div>
	);
}

export default AnalyticsView;
