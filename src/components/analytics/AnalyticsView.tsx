import React, { useState, useMemo, Suspense, useEffect, useRef } from "react";
import { useAnalyticsData } from "../../hooks/useAnalyticsData";
import TabBar from "./TabBar";
import NowTab from "./NowTab";
import TrendsTab from "./TrendsTab";
import type { RangeType, UsageBucket, AnalyticsTab } from "../../types";
import { mergeBucketsByLabel } from "../../types";

const ChartsTab = React.lazy(() => import("./ChartsTab"));

const TAB_KEY = "quill-analytics-tab";

interface AnalyticsViewProps {
	currentBuckets: UsageBucket[];
}

function AnalyticsView({ currentBuckets }: AnalyticsViewProps) {
	const [activeTab, setActiveTab] = useState<AnalyticsTab>(() => {
		try {
			const saved = localStorage.getItem(TAB_KEY);
			if (saved === "now" || saved === "trends" || saved === "charts") return saved;
		} catch { /* ignore */ }
		return "now";
	});
	const [nowRange, setNowRange] = useState<RangeType>("24h");
	const [trendsRange, setTrendsRange] = useState<RangeType>("7d");
	const [chartsRange, setChartsRange] = useState<RangeType>(() => {
		try {
			const saved = localStorage.getItem("quill-charts-range");
			if (saved === "1h" || saved === "24h" || saved === "7d" || saved === "30d") return saved as RangeType;
		} catch { /* ignore */ }
		return "24h";
	});
	const [selectedBucketKey, setSelectedBucketKey] = useState<string | null>(null);
	const [bucketMenuOpen, setBucketMenuOpen] = useState(false);
	const bucketMenuRef = useRef<HTMLDivElement>(null);

	const handleChartsRangeChange = (r: RangeType) => {
		setChartsRange(r);
		try {
			localStorage.setItem("quill-charts-range", r);
		} catch { /* ignore */ }
	};

	const handleTabChange = (tab: AnalyticsTab) => {
		setActiveTab(tab);
		try {
			localStorage.setItem(TAB_KEY, tab);
		} catch { /* ignore */ }
	};

	const mergedBuckets = useMemo(
		() => mergeBucketsByLabel(currentBuckets),
		[currentBuckets],
	);

	useEffect(() => {
		if (mergedBuckets.length === 0) {
			setSelectedBucketKey(null);
			return;
		}

		if (
			selectedBucketKey &&
			mergedBuckets.some((mb) => mb.label === selectedBucketKey)
		) {
			return;
		}

		setSelectedBucketKey(mergedBuckets[0].label);
	}, [selectedBucketKey, mergedBuckets]);

	useEffect(() => {
		if (!bucketMenuOpen) return;
		const handler = (event: MouseEvent) => {
			if (
				bucketMenuRef.current &&
				!bucketMenuRef.current.contains(event.target as Node)
			) {
				setBucketMenuOpen(false);
			}
		};
		document.addEventListener("mousedown", handler);
		return () => document.removeEventListener("mousedown", handler);
	}, [bucketMenuOpen]);

	const selectedBucket = mergedBuckets.find(
		(mb) => mb.label === selectedBucketKey,
	) ?? null;

	const { snapshotCount, loading } = useAnalyticsData(selectedBucket, "24h");

	if (snapshotCount === 0 && !loading) {
		return (
			<div className="analytics-view">
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
			</div>
		);
	}

	const selectedBucketLabel = selectedBucket
		? selectedBucket.label
		: "No live metric";

	return (
		<div className="analytics-view">
			<TabBar activeTab={activeTab} onTabChange={handleTabChange} />
			{mergedBuckets.length > 0 && (
				<div className="analytics-controls analytics-controls--metric">
					<div className="bucket-dropdown-wrap" ref={bucketMenuRef}>
						<button
							className="bucket-dropdown-trigger"
							onClick={() => setBucketMenuOpen((open) => !open)}
							aria-haspopup="true"
							aria-expanded={bucketMenuOpen}
						>
							<span>{selectedBucketLabel}</span>
							<span className="bucket-dropdown-arrow">
								{bucketMenuOpen ? "\u25B4" : "\u25BE"}
							</span>
						</button>
						{bucketMenuOpen && (
							<div className="bucket-dropdown-menu" role="menu">
								{mergedBuckets.map((mb) => (
									<button
										key={mb.label}
										className={`bucket-dropdown-item${mb.label === selectedBucketKey ? " active" : ""}`}
										role="menuitemradio"
										aria-checked={mb.label === selectedBucketKey}
										onClick={() => {
											setSelectedBucketKey(mb.label);
											setBucketMenuOpen(false);
										}}
									>
										{mb.label}
									</button>
								))}
							</div>
						)}
					</div>
				</div>
			)}
			{activeTab === "now" && (
				<NowTab
					range={nowRange}
					onRangeChange={setNowRange}
					currentBucket={selectedBucket}
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
						currentBucket={selectedBucket}
					/>
				</Suspense>
			)}
		</div>
	);
}

export default AnalyticsView;
