import { useState, useMemo } from "react";
import { useAnalyticsData } from "../../hooks/useAnalyticsData";
import { useTokenData } from "../../hooks/useTokenData";
import { useCodeStats } from "../../hooks/useCodeStats";
import { useEfficiencyStats } from "../../hooks/useEfficiencyStats";
import { useVelocityStats } from "../../hooks/useVelocityStats";
import { useResponseTimeStats } from "../../hooks/useResponseTimeStats";
import { formatNumber, formatDurationSecs } from "../../utils/format";
import InsightCard from "./InsightCard";
import CompactStatsRow from "./CompactStatsRow";
import BreakdownPanel from "./BreakdownPanel";
import TokenSparkline from "./TokenSparkline";
import CodeSparkline from "./CodeSparkline";
import type {
	RangeType,
	UsageBucket,
	BreakdownSelection,
} from "../../types";

const RANGES: RangeType[] = ["1h", "24h", "7d", "30d"];
const RANGE_LABELS: Record<RangeType, string> = {
	"1h": "1H",
	"24h": "24H",
	"7d": "7D",
	"30d": "30D",
};
const RANGE_DAYS: Record<RangeType, number> = {
	"1h": 1,
	"24h": 1,
	"7d": 7,
	"30d": 30,
};
const DAYS_TO_RANGE: Record<number, RangeType> = {
	1: "24h",
	7: "7d",
	30: "30d",
};

const BREAKDOWN_COLLAPSED_KEY = "quill-breakdown-collapsed";


interface NowTabProps {
	range: RangeType;
	onRangeChange: (r: RangeType) => void;
	currentBuckets: UsageBucket[];
}

function NowTab({ range, onRangeChange, currentBuckets }: NowTabProps) {
	const defaultBucket = currentBuckets?.[0]?.label ?? "7 days";
	const [breakdownSelection, setBreakdownSelection] =
		useState<BreakdownSelection | null>(null);
	const [breakdownCollapsed, setBreakdownCollapsed] = useState(() => {
		try {
			return localStorage.getItem(BREAKDOWN_COLLAPSED_KEY) === "true";
		} catch {
			return false;
		}
	});
	const breakdownDays = RANGE_DAYS[range] ?? 1;
	const hasSelection = breakdownSelection !== null;
	const tokenRange: RangeType = hasSelection
		? (DAYS_TO_RANGE[breakdownDays] ?? "24h")
		: range;

	const bucketsKey = (currentBuckets ?? [])
		.map((b) => `${b.label}:${b.utilization}:${b.resets_at ?? ""}`)
		.join(",");
	// eslint-disable-next-line react-hooks/exhaustive-deps -- bucketsKey is an intentional stabilizer
	const stableBuckets = useMemo(() => currentBuckets, [bucketsKey]);

	const { loading, error } = useAnalyticsData(
		defaultBucket,
		range,
		stableBuckets,
	);

	const responseTimeStats = useResponseTimeStats(range);

	const tokenHostname =
		breakdownSelection?.type === "host" ? breakdownSelection.key : null;
	const tokenSessionId =
		breakdownSelection?.type === "session" ? breakdownSelection.key : null;
	const tokenCwd =
		breakdownSelection?.type === "project" ? breakdownSelection.key : null;
	const { history: tokenHistory, stats: tokenStats } = useTokenData(
		tokenRange,
		tokenHostname,
		tokenSessionId,
		tokenCwd,
	);

	const { stats: codeStats, history: codeHistory } = useCodeStats(range);

	const efficiencyStats = useEfficiencyStats(range);
	const velocityStats = useVelocityStats(range);

	return (
		<>
			<div className="analytics-controls">
				<div className={`range-tabs${hasSelection ? " dimmed" : ""}`}>
					{RANGES.map((r) => (
						<button
							key={r}
							className={`range-tab${range === r ? " active" : ""}`}
							aria-pressed={range === r}
							onClick={() => onRangeChange(r)}
						>
							{RANGE_LABELS[r]}
						</button>
					))}
				</div>
			</div>

			<TokenSparkline data={tokenHistory} range={tokenRange} />
			<CodeSparkline data={codeHistory} range={range} />

			{error && (
				<div className="analytics-error" role="alert">
					Failed to load analytics
				</div>
			)}

			{loading ? (
				<>
					<div className="chart-skeleton" />
					<div className="breakdown-skeleton">
						<div className="breakdown-skeleton-row" />
						<div className="breakdown-skeleton-row" />
						<div className="breakdown-skeleton-row" />
					</div>
				</>
			) : (
				<>
					{/* Insight cards row */}
					<div className="insight-cards-row">
						<InsightCard
							label="Efficiency"
							value={
								efficiencyStats.tokensPerLoc !== null
									? formatNumber(efficiencyStats.tokensPerLoc)
									: null
							}
							subtitle="tokens per line of code"
							trend={efficiencyStats.trend}
							sparkline={efficiencyStats.sparkline}
							accentColor="#58a6ff"
						/>
						<InsightCard
							label="Velocity"
							value={
								velocityStats.locPerHour !== null
									? formatNumber(velocityStats.locPerHour)
									: null
							}
							subtitle="lines changed per hour"
							trend={velocityStats.trend}
							sparkline={velocityStats.sparkline}
							accentColor="#a78bfa"
						/>
						<InsightCard
							label="Response Time"
							value={
								responseTimeStats.avgResponseSecs !== null
									? formatDurationSecs(responseTimeStats.avgResponseSecs)
									: null
							}
							subtitle={
								responseTimeStats.avgResponseSecs !== null
									? `peak ${formatDurationSecs(responseTimeStats.peakResponseSecs)} \u00b7 avg idle ${formatDurationSecs(responseTimeStats.avgIdleSecs)}`
									: "no data"
							}
							trend={null}
							sparkline={responseTimeStats.sparkline}
							accentColor={responseTimeStats.accentColor}
						/>
					</div>

					{/* Compact tokens + code row */}
					<CompactStatsRow tokenStats={tokenStats} codeStats={codeStats} />

					{/* Breakdown */}
					<div className="breakdown-collapsible">
						<button
							className="breakdown-collapse-toggle"
							onClick={() => {
								const next = !breakdownCollapsed;
								setBreakdownCollapsed(next);
								try {
									localStorage.setItem(BREAKDOWN_COLLAPSED_KEY, String(next));
								} catch { /* ignore */ }
							}}
							aria-expanded={!breakdownCollapsed}
							aria-label={breakdownCollapsed ? "Show breakdown" : "Hide breakdown"}
						>
							<span className="breakdown-collapse-chevron">
								{breakdownCollapsed ? "\u25B8" : "\u25BE"}
							</span>
							<span className="section-title" style={{ marginBottom: 0 }}>Breakdown</span>
						</button>
						{!breakdownCollapsed && (
							<BreakdownPanel
								days={RANGE_DAYS[range] ?? 1}
								selection={breakdownSelection}
								onSelect={setBreakdownSelection}
							/>
						)}
					</div>
				</>
			)}
		</>
	);
}

export default NowTab;
